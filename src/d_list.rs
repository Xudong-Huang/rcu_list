use alloc::sync::{Arc, Weak};
use rcu_cell::{RcuCell, RcuWeak};

use core::ops::Deref;
use core::{cmp, fmt};

use crate::version_lock::{LockErr, VersionLock, VersionLockGuard};

#[derive(Debug)]
struct Node<T> {
    version: VersionLock,
    next: RcuCell<Node<T>>,
    prev: RcuWeak<Node<T>>,
    // only the head node and tail node has None data
    data: Option<T>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            version: VersionLock::new(),
            prev: RcuWeak::new(),
            next: RcuCell::none(),
            data: None,
        }
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        // A removed node keeps its `next` pointer so live iterators can still
        // walk past it. That means one held `Arc<Node>` can own a whole chain
        // of removed nodes, and dropping it recursively would overflow the
        // stack. Take ownership of each link and walk the chain iteratively.
        let mut next = core::mem::replace(&mut self.next, RcuCell::none()).into_arc();
        while let Some(node) = next {
            match Arc::into_inner(node) {
                // the next node is exclusively ours, detach its `next` and
                // continue; the node itself is dropped with an empty `next`
                Some(mut node) => {
                    next = core::mem::replace(&mut node.next, RcuCell::none()).into_arc();
                }
                // the next node is still referenced elsewhere, stop here
                None => break,
            }
        }
    }
}

/// RAII guard for a locked `Node` that the caller owns. It is used when the
/// locked node has to be returned from the locking function, where a borrowing
/// guard cannot be used. Owning the node avoids cloning its `Arc`.
struct OwnedNodeGuard<T> {
    node: Arc<Node<T>>,
}

impl<T> OwnedNodeGuard<T> {
    /// lock the node and take ownership of it
    fn lock(node: Arc<Node<T>>) -> Result<Self, LockErr> {
        node.version.lock()?;
        // the guard owns the node before anything can panic below
        let guard = OwnedNodeGuard { node };
        // the invariant check reads `next` and upgrades `prev`, which costs a
        // few atomic operations on every lock, so only check in debug builds
        debug_assert!(guard.node.next_node().prev_eq(&guard.node));
        Ok(guard)
    }

    #[inline]
    fn node(&self) -> &Arc<Node<T>> {
        &self.node
    }
}

impl<T> Drop for OwnedNodeGuard<T> {
    fn drop(&mut self) {
        self.node.version.unlock();
    }
}

impl<T> Node<T> {
    #[inline]
    fn new(data: T) -> Self {
        Node {
            version: VersionLock::new(),
            prev: RcuWeak::new(),
            next: RcuCell::none(),
            data: Some(data),
        }
    }

    #[inline]
    fn try_lock(&self) -> Result<VersionLockGuard<'_>, LockErr> {
        self.version.try_lock()?;
        Ok(VersionLockGuard::new(&self.version))
    }

    // lock the current node and return it's next node with the lock guard
    #[inline]
    fn lock(self: &Arc<Self>) -> Result<(Arc<Node<T>>, VersionLockGuard<'_>), LockErr> {
        self.version.lock()?;
        // the guard is created before anything can panic, so the node is
        // unlocked even if the assertion below fails
        let guard = VersionLockGuard::new(&self.version);
        let next_node = self.next_node();
        // `prev_eq` upgrades the weak pointer, only pay for it in debug builds
        debug_assert!(next_node.prev_eq(self));
        Ok((next_node, guard))
    }

    #[inline]
    fn is_removed(&self) -> bool {
        self.version.is_removed()
    }

    #[inline]
    fn prev_node(&self) -> Option<Arc<Node<T>>> {
        self.prev.upgrade()
    }

    #[inline]
    fn prev_eq(&self, prev: &Arc<Node<T>>) -> bool {
        self.prev.arc_eq(prev)
    }

    #[inline]
    fn set_prev_node(&self, prev: &Arc<Node<T>>) -> Weak<Node<T>> {
        self.prev.write_arc(prev)
    }

    #[inline]
    fn clear_prev_node(&self) {
        self.prev.take();
    }

    #[inline]
    fn next_node(&self) -> Arc<Node<T>> {
        // Safety: the next node is always valid except for the tail node
        self.next.read().unwrap()
    }

    fn lock_prev_node(self: &Arc<Self>) -> Result<OwnedNodeGuard<T>, LockErr> {
        let backoff = crossbeam_utils::Backoff::new();
        loop {
            let prev_node = match self.prev_node() {
                // something wrong, like the prev node is dropped,
                // or the current node is removed
                None => {
                    if self.is_removed() {
                        return Err(LockErr::Removed);
                    }
                    core::hint::spin_loop();
                    continue;
                }

                // the prev can change due to prev insert/remove
                // we will do more check later
                Some(prev) => prev,
            };

            // if the prev node is removed, try again
            // the guard owns the prev node and unlocks it on drop
            let guard = match OwnedNodeGuard::lock(prev_node) {
                Ok(guard) => guard,
                Err(_) => {
                    backoff.spin();
                    continue;
                }
            };

            // check current node is not removed, the guard unlocks the prev
            // node on the early return
            if self.is_removed() {
                return Err(LockErr::Removed);
            }

            // if the prev node is changed, try again
            if !guard.node().next.arc_eq(self) {
                backoff.reset();
                continue;
            }

            debug_assert!(self.prev_eq(guard.node()));

            // successfully lock the prev node
            return Ok(guard);
        }
    }
}

/// An owned entry in a `LinkedList`.
///
/// Unlike [`Entry`], a `StaticEntry` owns its node and does not borrow the
/// list, so it can be stored independently. Since it is not tied to a list, it
/// does not provide `next`, `remove_after` or `remove_ahead`.
pub struct StaticEntry<T> {
    node: Arc<Node<T>>,
}

impl<T> StaticEntry<T> {
    fn new(node: Arc<Node<T>>) -> Self {
        StaticEntry { node }
    }
    /// Returns true if the entry is removed.
    pub fn is_removed(&self) -> bool {
        self.node.is_removed()
    }

    /// Remove the entry from the list.
    pub fn remove(&self) {
        EntryImpl::new(&self.node).remove()
    }

    /// Replace the entry with a new value and return an entry for the new
    /// value. The old entry is marked as removed.
    /// If the node is already removed, return the passed in value in `Err()`.
    /// Internally we create a new node to replace the old entry.
    pub fn replace(&self, elt: T) -> Result<StaticEntry<T>, T> {
        EntryImpl::new(&self.node)
            .replace(elt)
            .map(StaticEntry::new)
    }

    /// Insert an element after the entry.
    /// If the entry was removed, the element will be returned in `Err()`.
    pub fn insert_after(&self, elt: T) -> Result<StaticEntry<T>, T> {
        EntryImpl::new(&self.node)
            .insert_after(elt)
            .map(StaticEntry::new)
    }

    /// Insert an element ahead of the entry.
    /// If the entry was removed, the element will be returned in `Err()`.
    pub fn insert_ahead(&self, elt: T) -> Result<StaticEntry<T>, T> {
        EntryImpl::new(&self.node)
            .insert_ahead(elt)
            .map(StaticEntry::new)
    }
}

impl<T> Clone for StaticEntry<T> {
    fn clone(&self) -> Self {
        StaticEntry {
            node: self.node.clone(),
        }
    }
}

impl<T> Deref for StaticEntry<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.node.data.as_ref().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for StaticEntry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StaticEntry({:?})", self.node.data.as_ref().unwrap())
    }
}

impl<T: PartialEq> PartialEq for StaticEntry<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.node, &other.node) || self.node.data == other.node.data
    }
}

impl<T> AsRef<T> for StaticEntry<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: PartialOrd> PartialOrd for StaticEntry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }

    fn lt(&self, other: &Self) -> bool {
        *(*self) < *(*other)
    }

    fn le(&self, other: &Self) -> bool {
        *(*self) <= *(*other)
    }

    fn gt(&self, other: &Self) -> bool {
        *(*self) > *(*other)
    }

    fn ge(&self, other: &Self) -> bool {
        *(*self) >= *(*other)
    }
}

impl<T> From<Entry<'_, T>> for StaticEntry<T> {
    fn from(entry: Entry<'_, T>) -> Self {
        entry.into_static()
    }
}

impl<T: Ord> Ord for StaticEntry<T> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        if Arc::ptr_eq(&self.node, &other.node) {
            return cmp::Ordering::Equal;
        }
        (**self).cmp(&**other)
    }
}

impl<T: Eq> Eq for StaticEntry<T> {}

/// An entry in a `LinkedList`.
pub struct Entry<'a, T> {
    list: &'a LinkedList<T>,
    node: Arc<Node<T>>,
}

impl<'a, T> Entry<'a, T> {
    /// Replace the entry with a new value and return an entry for the new
    /// value. The old entry is marked as removed.
    /// If the node is already removed, return the passed in value in `Err()`.
    /// Internally we create a new node to replace the old entry.
    pub fn replace(&self, elt: T) -> Result<Entry<'a, T>, T> {
        EntryImpl::new(&self.node).replace(elt).map(|node| Entry {
            list: self.list,
            node,
        })
    }

    /// Remove the entry from the list.
    pub fn remove(&self) {
        EntryImpl::new(&self.node).remove()
    }

    /// Insert an element after the entry.
    /// If the entry was removed, the element will be returned in `Err()`.
    pub fn insert_after(&self, elt: T) -> Result<Entry<'a, T>, T> {
        EntryImpl::new(&self.node)
            .insert_after(elt)
            .map(|node| Entry {
                list: self.list,
                node,
            })
    }

    /// Insert an element ahead of the entry.
    /// If the entry was removed, the element will be returned in `Err()`.
    pub fn insert_ahead(&self, elt: T) -> Result<Entry<'a, T>, T> {
        EntryImpl::new(&self.node)
            .insert_ahead(elt)
            .map(|node| Entry {
                list: self.list,
                node,
            })
    }

    /// Remove the entry after this entry.
    pub fn remove_after(&self) -> Option<Entry<'a, T>> {
        EntryImpl::new(&self.node)
            .remove_after(self.list)
            .map(|node| Entry {
                list: self.list,
                node,
            })
    }

    /// Remove the entry ahead this entry.
    pub fn remove_ahead(&self) -> Option<Entry<'a, T>> {
        EntryImpl::new(&self.node)
            .remove_ahead(self.list)
            .map(|node| Entry {
                list: self.list,
                node,
            })
    }

    /// Returns true if the entry is removed.
    pub fn is_removed(&self) -> bool {
        self.node.is_removed()
    }

    /// Returns the next entry in the list.
    /// Returns `None` if the entry is removed or is the last entry.
    pub fn next(&self) -> Option<Entry<'a, T>> {
        if self.is_removed() {
            return None;
        }

        let next = self.node.next.read()?;
        // only the tail node has None data, never return it as an entry
        next.data.is_some().then(|| Entry {
            list: self.list,
            node: next,
        })
    }

    /// Convert to `StaticEntry`.
    pub fn into_static(self) -> StaticEntry<T> {
        StaticEntry { node: self.node }
    }
}

impl<T> Clone for Entry<'_, T> {
    fn clone(&self) -> Self {
        Entry {
            list: self.list,
            node: self.node.clone(),
        }
    }
}

impl<T> Deref for Entry<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.node.data.as_ref().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for Entry<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entry({:?})", self.node.data.as_ref().unwrap())
    }
}

impl<T: PartialEq> PartialEq for Entry<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.node, &other.node) || self.node.data == other.node.data
    }
}

impl<T> AsRef<T> for Entry<'_, T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: PartialOrd> PartialOrd for Entry<'_, T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }

    fn lt(&self, other: &Self) -> bool {
        *(*self) < *(*other)
    }

    fn le(&self, other: &Self) -> bool {
        *(*self) <= *(*other)
    }

    fn gt(&self, other: &Self) -> bool {
        *(*self) > *(*other)
    }

    fn ge(&self, other: &Self) -> bool {
        *(*self) >= *(*other)
    }
}

impl<T: Ord> Ord for Entry<'_, T> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        if Arc::ptr_eq(&self.node, &other.node) {
            return cmp::Ordering::Equal;
        }
        (**self).cmp(&**other)
    }
}

impl<T: Eq> Eq for Entry<'_, T> {}

/// A concurrent doubly linked list.
///
/// Internally it uses fine-grained per-node locks to ensure thread safety.
/// Readers such as `iter`, `front` and `back` do not need to acquire locks.
#[derive(Debug)]
pub struct LinkedList<T> {
    head: Arc<Node<T>>,
    tail: Arc<Node<T>>,
}

impl<T> Default for LinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LinkedList<T> {
    /// Creates a new empty `LinkedList`.
    pub fn new() -> Self {
        // this is only used for list head, should never deref it's data
        let head = Arc::new(Node::default());
        let tail = Arc::new(Node::default());

        tail.set_prev_node(&head);
        head.next.write(tail.clone());

        Self { head, tail }
    }

    /// Returns true if the list is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.next.arc_eq(&self.tail)
    }

    /// Returns an Entry to the front element, or `None` if the list is empty.
    #[inline]
    pub fn front(&self) -> Option<Entry<'_, T>> {
        // head.next is always non empty
        let node = self.head.next_node();
        // only the tail has None data
        node.data.is_some().then(|| Entry { list: self, node })
    }

    /// Returns an Entry to the back element, or `None` if the list is empty.
    #[inline]
    pub fn back(&self) -> Option<Entry<'_, T>> {
        // tail.prev is always non empty
        let node = loop {
            match self.tail.prev_node() {
                Some(node) => break node,
                // the prev node is dropped
                // try again for a valid prev node
                None => continue,
            }
        };
        // only the head has None data
        node.data.is_some().then(|| Entry { list: self, node })
    }

    /// Pushes an element to the front of the list, and returns an Entry to it.
    pub fn push_front(&self, elt: T) -> Entry<'_, T> {
        match EntryImpl::new(&self.head).insert_after(elt) {
            Ok(node) => Entry { list: self, node },
            Err(_) => unreachable!("push_front should always success"),
        }
    }

    /// Pops the front element of the list, returns `None` if the list is empty.
    pub fn pop_front(&self) -> Option<Entry<'_, T>> {
        EntryImpl::new(&self.head)
            .remove_after(self)
            .map(|node| Entry { list: self, node })
    }

    /// Pushes an element to the back of the list, and returns an Entry to it.
    pub fn push_back(&self, elt: T) -> Entry<'_, T> {
        match EntryImpl::new(&self.tail).insert_ahead(elt) {
            Ok(node) => Entry { list: self, node },
            Err(_) => unreachable!("push_back should always success"),
        }
    }

    /// Pops the back element of the list, returns `None` if the list is empty.
    pub fn pop_back(&self) -> Option<Entry<'_, T>> {
        EntryImpl::new(&self.tail)
            .remove_ahead(self)
            .map(|node| Entry { list: self, node })
    }

    /// Returns an iterator over the elements of the list.
    #[inline]
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            list: self,
            curr: self.head.clone(),
        }
    }
}

/// An iterator over the elements of a `LinkedList`.
///
/// This `struct` is created by [`LinkedList::iter()`]. See its
/// documentation for more.
pub struct Iter<'a, T: 'a> {
    list: &'a LinkedList<T>,
    curr: Arc<Node<T>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = Entry<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let next_node = self.curr.next.read()?;
        self.curr = next_node.clone();
        next_node.data.is_some().then(|| Entry {
            list: self.list,
            node: next_node,
        })
    }
}

struct EntryImpl<'a, T> {
    node: &'a Arc<Node<T>>,
}

impl<'a, T> EntryImpl<'a, T> {
    #[inline]
    fn new(node: &'a Arc<Node<T>>) -> Self {
        Self { node }
    }

    /// Remove the entry from the list.
    fn remove(self) {
        let curr_node = self.node;
        let prev_guard = match curr_node.lock_prev_node() {
            Ok(guard) => guard,
            // the current node is already removed
            Err(_) => return,
        };

        // move the drop out of locks
        let old_node_prev;
        let old_prev_next;

        {
            // unwrap safety: the prev node is locked
            let (next_node, curr_guard) = curr_node.lock().unwrap();
            {
                old_node_prev = next_node.set_prev_node(prev_guard.node());
                old_prev_next = prev_guard.node().next.write(next_node);
            }
            curr_guard.remove();
            curr_node.clear_prev_node();
        }
        drop(prev_guard);

        drop(old_node_prev);
        drop(old_prev_next);
    }

    /// Replace the entry with new value,
    fn replace(&self, elt: T) -> Result<Arc<Node<T>>, T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.next.write(self.node.clone());

        // move the drop out of locks
        let old_node_prev;
        let old_prev_next;

        let prev_guard = match self.node.lock_prev_node() {
            Ok(guard) => guard,
            Err(_) => {
                let mut node = Arc::into_inner(new_node).unwrap();
                return Err(node.data.take().unwrap());
            }
        };
        {
            new_node.set_prev_node(prev_guard.node());
            let new_guard = new_node.try_lock().unwrap();
            let (_next_node, old_guard) = self.node.lock().unwrap();
            {
                let next_node = self.node.next_node();
                old_node_prev = next_node.set_prev_node(&new_node);
                new_node.next.write(next_node.clone());
                old_prev_next = prev_guard.node().next.write(new_node.clone());
            }
            old_guard.remove();
            drop(new_guard);
            self.node.clear_prev_node();
        }
        drop(prev_guard);

        drop(old_node_prev);
        drop(old_prev_next);

        Ok(new_node)
    }

    /// insert an element after the entry.
    /// if the entry was removed, the element will be returned in Err()
    fn insert_after(&self, elt: T) -> Result<Arc<Node<T>>, T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.set_prev_node(self.node);

        // move the drop out of locks
        let old_next_prev;
        let old_head_next;

        let (next_node, guard) = match self.node.lock() {
            Ok(node) => node,
            Err(_) => {
                // current entry removed, can't insert
                let mut n = Arc::into_inner(new_node).unwrap();
                return Err(n.data.take().unwrap());
            }
        };
        {
            // new_node.try_lock().unwrap();
            {
                new_node.next.write(next_node.clone());
                old_next_prev = next_node.set_prev_node(&new_node);
                old_head_next = self.node.next.write(new_node.clone());
            }
            // new_node.unlock();
        }
        drop(guard);

        drop(old_next_prev);
        drop(old_head_next);

        Ok(new_node)
    }

    /// remove element after this entry
    fn remove_after(&self, list: &LinkedList<T>) -> Option<Arc<Node<T>>> {
        // move the drop out of locks
        let old_next_prev;
        let old_head_next;

        let (curr_node, guard) = match self.node.lock() {
            Ok(node) => node,
            Err(_) => return None,
        };
        {
            // there is no element after entry
            if Arc::ptr_eq(&curr_node, &list.tail) {
                return None;
            }

            // unwrap safety: next must be valid since it's still in the list
            let (next_node, curr_guard) = curr_node.lock().unwrap();
            {
                old_next_prev = next_node.set_prev_node(self.node);
                old_head_next = self.node.next.write(next_node.clone());
            }
            curr_guard.remove();
            curr_node.clear_prev_node();
        }
        drop(guard);

        // don't clear the next field, could break the iterator
        // curr_node.next.take();
        drop(old_next_prev);
        drop(old_head_next);

        Some(curr_node)
    }

    /// Insert an element ahead of the entry, and returns the new Entry to it.
    pub fn insert_ahead(&self, elt: T) -> Result<Arc<Node<T>>, T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.next.write(self.node.clone());

        // move the drop out of locks
        let old_node_prev;
        let old_prev_next;

        let prev_guard = match self.node.lock_prev_node() {
            Ok(guard) => guard,
            Err(_) => {
                let mut node = Arc::into_inner(new_node).unwrap();
                return Err(node.data.take().unwrap());
            }
        };
        {
            new_node.set_prev_node(prev_guard.node());
            // new_node.try_lock().unwrap();
            {
                old_node_prev = self.node.set_prev_node(&new_node);
                old_prev_next = prev_guard.node().next.write(new_node.clone());
            }
            // new_node.unlock();
        }
        drop(prev_guard);

        drop(old_node_prev);
        drop(old_prev_next);

        Ok(new_node)
    }

    /// Remove the element ahead of the entry, returns `None` if the list is empty.
    fn remove_ahead(&self, list: &LinkedList<T>) -> Option<Arc<Node<T>>> {
        loop {
            // move the drop out of locks
            let old_node_prev;
            let old_prev_next;

            let curr_node = match self.node.prev_node() {
                Some(node) => node,
                None => {
                    if self.node.is_removed() {
                        return None;
                    }
                    core::hint::spin_loop();
                    continue;
                }
            };

            // the list is empty
            if Arc::ptr_eq(&curr_node, &list.head) {
                return None;
            }

            // try to lock the node.prev.prev node
            let prev_guard = match curr_node.lock_prev_node() {
                Ok(guard) => guard,
                Err(_) => continue,
            };

            {
                // lock the curr node, curr node is not removed
                let (next_node, curr_guard) = curr_node.lock().unwrap();
                {
                    // after lock curr_node some thing changed, try again
                    // the guards unlock both nodes on continue
                    if !Arc::ptr_eq(&next_node, self.node) {
                        continue;
                    }

                    old_node_prev = self.node.set_prev_node(prev_guard.node());
                    old_prev_next = prev_guard.node().next.write(next_node);
                }
                curr_guard.remove();
                curr_node.clear_prev_node();
            }
            drop(prev_guard);

            drop(old_node_prev);
            drop(old_prev_next);

            return Some(curr_node);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    #[test]
    fn test_list() {
        let list = super::LinkedList::new();
        assert!(list.is_empty());

        list.push_back(1);
        assert!(!list.is_empty());
        assert_eq!(*list.front().unwrap(), 1);
        assert_eq!(*list.back().unwrap(), 1);

        list.push_back(2);
        assert_eq!(*list.front().unwrap(), 1);
        assert_eq!(*list.back().unwrap(), 2);

        list.push_front(0);
        assert_eq!(*list.front().unwrap(), 0);
        assert_eq!(*list.back().unwrap(), 2);

        assert_eq!(*list.pop_front().unwrap(), 0);
        assert_eq!(*list.pop_front().unwrap(), 1);
        assert_eq!(*list.pop_front().unwrap(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn test_list_1() {
        let list = super::LinkedList::new();
        assert!(list.is_empty());

        list.push_front(1);
        assert!(!list.is_empty());
        assert_eq!(*list.front().unwrap(), 1);
        assert_eq!(*list.back().unwrap(), 1);

        list.push_front(2);
        assert_eq!(*list.front().unwrap(), 2);
        assert_eq!(*list.back().unwrap(), 1);

        list.push_back(0);
        assert_eq!(*list.front().unwrap(), 2);
        assert_eq!(*list.back().unwrap(), 0);

        assert_eq!(*list.pop_back().unwrap(), 0);
        assert_eq!(*list.pop_back().unwrap(), 1);
        assert_eq!(*list.pop_back().unwrap(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn test_remove_entry() {
        let list = super::LinkedList::new();
        let entry = list.push_back(1);
        assert!(!entry.is_removed());
        assert!(*entry == 1);
        entry.remove();
        assert!(list.is_empty());
        assert!(list.front().is_none());
        assert!(list.back().is_none());
    }

    #[test]
    fn test_iter() {
        let list = super::LinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_remove() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        entry.remove();

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_insert_after() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        entry.insert_after(100).unwrap();

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 100);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_insert_after_remove() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        assert_eq!(*entry.insert_after(100).unwrap(), 100);

        let mut iter = list.iter();
        let find_entry = iter.find(|e| **e == 2).unwrap();
        find_entry.remove();

        assert!(entry.is_removed());
        assert_eq!(entry.insert_after(101), Err(101));
    }

    #[test]
    fn simple_drop() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static REF: AtomicUsize = AtomicUsize::new(0);
        struct Foo(usize);
        impl Foo {
            fn new(data: usize) -> Self {
                Foo(data)
            }
        }
        impl Drop for Foo {
            fn drop(&mut self) {
                REF.fetch_add(self.0, Ordering::Relaxed);
            }
        }
        let list = super::LinkedList::new();

        for i in 0..100 {
            list.push_back(Foo::new(i));
        }

        drop(list);
        assert_eq!(REF.load(Ordering::Relaxed), (0..100).sum());
    }

    #[test]
    fn drop_long_removed_chain_is_iterative() {
        let list = super::LinkedList::new();
        const N: usize = 200_000;

        for i in 0..N {
            list.push_back(i);
        }

        // The held entry keeps the first removed node alive, and its `next`
        // chain keeps every other removed node alive. Dropping it used to
        // recurse once per element and overflow the stack.
        let first = list.pop_front().unwrap();
        while list.pop_front().is_some() {}

        drop(first);
    }

    #[test]
    fn entry_replace() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        let new_entry = entry.replace(100).unwrap();
        assert!(entry.is_removed());
        assert_eq!(*new_entry, 100);

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 100);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_next() {
        use alloc::vec;
        use alloc::vec::Vec;

        let list = super::LinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        // Walk through `Entry::next`; the last element must return `None`
        // instead of exposing the tail sentinel.
        let mut values = Vec::new();
        let mut entry = list.front();
        while let Some(e) = entry {
            values.push(*e);
            entry = e.next();
        }
        assert_eq!(values, vec![1, 2, 3]);
    }

    #[test]
    fn entry_next_last_and_removed() {
        let list = super::LinkedList::new();
        let entry = list.push_back(1);
        assert!(entry.next().is_none());

        entry.remove();
        assert!(entry.next().is_none());
        assert!(list.is_empty());
    }

    #[test]
    fn entry_insert_ahead() {
        use alloc::vec;
        use alloc::vec::Vec;

        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        entry.insert_ahead(100).unwrap();

        let values: Vec<_> = list.iter().map(|e| *e).collect();
        assert_eq!(values, vec![1, 100, 2, 3]);
    }

    #[test]
    fn entry_remove_after() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        let removed = entry.remove_after().unwrap();
        assert_eq!(*removed, 3);
        assert!(removed.is_removed());
        assert!(list.iter().map(|e| *e).eq([1, 2]));

        // there is nothing after the last entry
        assert!(entry.remove_after().is_none());
    }

    #[test]
    fn entry_remove_ahead() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(3);

        let removed = entry.remove_ahead().unwrap();
        assert_eq!(*removed, 1);
        assert!(removed.is_removed());
        assert!(list.iter().map(|e| *e).eq([3]));

        // there is nothing ahead of the first entry
        assert!(entry.remove_ahead().is_none());
    }

    #[test]
    fn static_entry_ops() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2).into_static();

        assert_eq!(*entry, 2);

        // insert on both sides
        assert_eq!(*entry.insert_after(3).unwrap(), 3);
        assert_eq!(*entry.insert_ahead(0).unwrap(), 0);
        assert!(list.iter().map(|e| *e).eq([1, 0, 2, 3]));

        // replace marks the old entry removed and returns the new one
        let replaced = entry.replace(20).unwrap();
        assert!(entry.is_removed());
        assert_eq!(*replaced, 20);
        assert!(list.iter().map(|e| *e).eq([1, 0, 20, 3]));

        // write operations on a removed entry fail
        assert_eq!(entry.insert_after(4), Err(4));
        assert_eq!(entry.insert_ahead(4), Err(4));

        // removing a removed entry is a no-op, removing a live one works
        entry.remove();
        assert!(list.iter().map(|e| *e).eq([1, 0, 20, 3]));
        replaced.remove();
        assert!(replaced.is_removed());
        assert!(list.iter().map(|e| *e).eq([1, 0, 3]));
    }

    #[test]
    fn guard_remove_marks_removed() {
        let node = alloc::sync::Arc::new(super::Node::new(1));
        {
            let guard = node.try_lock().unwrap();
            guard.remove();
        }
        assert!(node.is_removed());
        assert_eq!(node.try_lock().err(), Some(super::LockErr::Removed));
    }

    #[test]
    fn panic_while_holding_locks_unlocks_nodes() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let list = super::LinkedList::new();
        let entry = list.push_back(1);
        // mark the only element as removed without unlinking it, so
        // pop_front takes the head lock and then panics on lock().unwrap()
        {
            let guard = entry.node.try_lock().unwrap();
            guard.remove();
        }

        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = list.pop_front();
        }));
        assert!(result.is_err());

        // the head node must have been unlocked while unwinding, or this
        // would hang forever
        assert!(list.head.try_lock().is_ok());
    }

    #[test]
    fn iter_walks_past_removed_node() {
        let list = super::LinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);
        // move the iterator onto node 2, then remove that node
        assert_eq!(*iter.next().unwrap(), 2);
        let removed = list.front().unwrap().remove_after().unwrap();
        assert_eq!(*removed, 2);
        assert!(removed.is_removed());

        // the removed node keeps its `next`, so the iterator can continue
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());

        // a fresh iterator only sees the live nodes
        assert!(list.iter().map(|e| *e).eq([1, 3]));
    }

    #[test]
    fn replace_removed_entry_fails() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        entry.remove();
        assert_eq!(entry.replace(10), Err(10));

        let static_entry = list.push_back(3).into_static();
        static_entry.remove();
        assert_eq!(static_entry.replace(11), Err(11));
    }

    #[test]
    fn remove_around_removed_entry() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);
        entry.remove();

        assert!(entry.remove_after().is_none());
        assert!(entry.remove_ahead().is_none());
    }

    #[test]
    fn entry_traits() {
        use alloc::format;
        use core::cmp::Ordering;

        let list = super::LinkedList::new();
        list.push_back(10);
        let a = list.front().unwrap();
        let b = list.front().unwrap();

        assert_eq!(format!("{:?}", a), "Entry(10)");
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(*a.as_ref(), 10);
        assert_eq!(*a.clone(), 10);

        list.push_back(20);
        let d = list.back().unwrap();

        assert!(a < d);
        assert!(a <= d);
        assert!(d > a);
        assert!(d >= a);
        assert_ne!(a, d);
        assert_eq!(a.partial_cmp(&d), Some(Ordering::Less));
        assert_eq!(a.cmp(&d), Ordering::Less);
    }

    #[test]
    fn static_entry_traits() {
        use alloc::format;
        use core::cmp::Ordering;

        let list = super::LinkedList::new();
        list.push_back(10);
        let a = list.push_back(20).into_static();
        let b = a.clone();

        assert_eq!(format!("{:?}", a), "StaticEntry(20)");
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(*a.as_ref(), 20);
        assert_eq!(*b, 20);

        let c = list.push_back(30).into_static();
        assert_ne!(a, c);
        assert!(a < c);
        assert!(a <= c);
        assert!(c > a);
        assert!(c >= a);
        assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
        assert_eq!(c.cmp(&a), Ordering::Greater);

        // From<Entry>
        let from_entry = super::StaticEntry::from(list.back().unwrap());
        assert_eq!(*from_entry, 30);
        assert_eq!(from_entry, c);
    }

    #[test]
    fn default_is_empty() {
        let list: super::LinkedList<i32> = Default::default();
        assert!(list.is_empty());
        assert!(list.iter().next().is_none());
    }
}
