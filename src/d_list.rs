use alloc::sync::{Arc, Weak};
use rcu_cell::{RcuCell, RcuWeak};

use core::ops::Deref;
use core::{cmp, fmt};

use crate::version_lock::{TryLockErr, VersionLock};

#[derive(Debug)]
#[repr(align(64))]
struct Node<T> {
    version: VersionLock,
    next: RcuCell<Node<T>>,
    prev: RcuWeak<Node<T>>,
    // only the head node has None data
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
    fn try_lock(&self) -> Result<usize, TryLockErr> {
        self.version.try_lock()
    }

    // lock the current node and return it's next node
    #[inline]
    fn lock(self: &Arc<Self>) -> Result<Arc<Node<T>>, TryLockErr> {
        self.version.lock()?;
        let next_node = self.next_node();
        assert!(next_node.prev_eq(self));
        Ok(next_node)
    }

    #[inline]
    fn unlock(&self) {
        self.version.unlock();
    }

    #[inline]
    fn unlock_remove(&self) {
        self.version.unlock_remove();
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
    fn clear_prev_node(&self) -> Weak<Node<T>> {
        self.prev.write(Weak::new())
    }

    #[inline]
    fn next_node(&self) -> Arc<Node<T>> {
        // Safety: the next node is always valid except for the tail node
        self.next.read().unwrap()
    }

    fn lock_prev_node(self: &Arc<Self>) -> Result<Arc<Node<T>>, ()> {
        loop {
            let prev_node = match self.prev_node() {
                // something wrong, like the prev node is dropped,
                // or the current node is removed
                None => {
                    if self.is_removed() {
                        return Err(());
                    }
                    core::hint::spin_loop();
                    continue;
                }

                // the prev can change due to prev insert/remove
                // we will do more check later
                Some(prev) => prev,
            };

            // if the prev node is removed, try again
            if prev_node.lock().is_err() {
                continue;
            }

            // check current node is not removed
            if self.is_removed() {
                prev_node.unlock();
                return Err(());
            }

            // if the prev node is changed, try again
            if !prev_node.next.arc_eq(self) {
                prev_node.unlock();
                core::hint::spin_loop();
                continue;
            }

            assert!(self.prev_eq(&prev_node));

            // successfully lock the prev node
            return Ok(prev_node);
        }
    }
}

/// An entry in a `LinkedList`.
pub struct Entry<'a, T> {
    list: &'a LinkedList<T>,
    node: Arc<Node<T>>,
}

impl<T> Entry<'_, T> {
    /// Remove the entry from the list.
    pub fn remove(self) {
        EntryImpl::new(self.list, &self.node).remove()
    }

    /// insert an element after the entry.
    /// if the entry was removed, the element will be returned in Err()
    pub fn insert_after(&self, elt: T) -> Result<Entry<T>, T> {
        EntryImpl::new(self.list, &self.node).insert_after(elt)
    }

    /// insert an element ahead the entry.
    /// if the entry was removed, the element will be returned in Err()
    pub fn insert_ahead(&self, elt: T) -> Result<Entry<T>, T> {
        EntryImpl::new(self.list, &self.node).insert_ahead(elt)
    }

    /// Remove the entry after this entry.
    pub fn remove_after(&self) -> Option<Entry<T>> {
        EntryImpl::new(self.list, &self.node).remove_after()
    }

    /// Remove the entry ahead this entry.
    pub fn remove_ahead(&self) -> Option<Entry<T>> {
        EntryImpl::new(self.list, &self.node).remove_ahead()
    }

    /// Returns true if the entry is removed.
    pub fn is_removed(&self) -> bool {
        self.node.is_removed()
    }

    /// Returns the next entry in the list.
    /// Returns `None` if the entry is removed.
    pub fn next(&self) -> Option<Entry<T>> {
        if self.is_removed() {
            return None;
        }

        let next = self.node.next.read()?;
        if Arc::ptr_eq(&self.node, &self.list.tail) {
            // we will not return the tail node as an entry
            return None;
        }
        Some(Entry {
            list: self.list,
            node: next,
        })
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
        self.node.data == other.node.data
    }
}

impl<T> AsRef<T> for Entry<'_, T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: PartialOrd> PartialOrd for Entry<'_, T> {
    fn partial_cmp(&self, other: &Entry<T>) -> Option<cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }

    fn lt(&self, other: &Entry<T>) -> bool {
        *(*self) < *(*other)
    }

    fn le(&self, other: &Entry<T>) -> bool {
        *(*self) <= *(*other)
    }

    fn gt(&self, other: &Entry<T>) -> bool {
        *(*self) > *(*other)
    }

    fn ge(&self, other: &Entry<T>) -> bool {
        *(*self) >= *(*other)
    }
}

impl<T: Ord> Ord for Entry<'_, T> {
    fn cmp(&self, other: &Entry<T>) -> cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: Eq> Eq for Entry<'_, T> {}

/// A concurrent doubly linked list.
/// Internally it use fine-grained double locks to ensure thread safety.
/// The readers like `iter`, `front` and `back` don't need to get locks.
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

impl<T> Drop for LinkedList<T> {
    fn drop(&mut self) {
        // avoid stack overflow
        while self.pop_front().is_some() {}
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
    pub fn front(&self) -> Option<Entry<T>> {
        // head.next is always non empty
        let node = self.head.next_node();
        // only the tail has None data
        node.data.is_some().then(|| Entry { list: self, node })
    }

    /// Returns an Entry to the back element, or `None` if the list is empty.
    #[inline]
    pub fn back(&self) -> Option<Entry<T>> {
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
    pub fn push_front(&self, elt: T) -> Entry<T> {
        match EntryImpl::new(self, &self.head).insert_after(elt) {
            Ok(entry) => entry,
            Err(_) => unreachable!("push_front should always success"),
        }
    }

    /// Pops the front element of the list, returns `None` if the list is empty.
    pub fn pop_front(&self) -> Option<Entry<T>> {
        EntryImpl::new(self, &self.head).remove_after()
    }

    /// Pushes an element to the back of the list, and returns an Entry to it.
    pub fn push_back(&self, elt: T) -> Entry<T> {
        match EntryImpl::new(self, &self.tail).insert_ahead(elt) {
            Ok(entry) => entry,
            Err(_) => unreachable!("push_back should always success"),
        }
    }

    /// Pops the back element of the list, returns `None` if the list is empty.
    pub fn pop_back(&self) -> Option<Entry<T>> {
        EntryImpl::new(self, &self.tail).remove_ahead()
    }

    /// Returns an iterator over the elements of the list.
    #[inline]
    pub fn iter(&self) -> Iter<T> {
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
        let next = self.curr.next_node();
        self.curr = next.clone();
        if Arc::ptr_eq(&self.curr, &self.list.tail) {
            return None;
        }
        Some(Entry {
            list: self.list,
            node: next,
        })
    }
}

struct EntryImpl<'a, 'b, T> {
    list: &'a LinkedList<T>,
    node: &'b Arc<Node<T>>,
}

impl<'a, 'b, T> EntryImpl<'a, 'b, T> {
    #[inline]
    fn new(list: &'a LinkedList<T>, node: &'b Arc<Node<T>>) -> Self {
        Self { list, node }
    }

    /// Remove the entry from the list.
    fn remove(self) {
        let curr_node = self.node;
        let prev_node = match curr_node.lock_prev_node() {
            Ok(node) => node,
            // the current node is already removed
            Err(_) => return,
        };
        {
            // unwrap safety: the prev node is locked
            let next_node = curr_node.lock().unwrap();
            {
                next_node.set_prev_node(&prev_node);
                prev_node.next.write(next_node);
            }
            curr_node.unlock_remove();
            curr_node.clear_prev_node();
        }
        prev_node.unlock();
    }

    /// insert an element after the entry.
    /// if the entry was removed, the element will be returned in Err()
    fn insert_after(&self, elt: T) -> Result<Entry<'a, T>, T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.set_prev_node(self.node);

        // move the drop out of locks
        let old_next_prev;
        let old_head_next;

        let next_node = match self.node.lock() {
            Ok(node) => node,
            Err(_) => {
                // current entry removed, can't insert
                let n = Arc::into_inner(new_node).unwrap();
                return Err(n.data.unwrap());
            }
        };
        {
            new_node.try_lock().unwrap();
            {
                old_next_prev = next_node.set_prev_node(&new_node);
                new_node.next.write(next_node.clone());
                old_head_next = self.node.next.write(new_node.clone());
            }
            new_node.unlock();
        }
        self.node.unlock();

        drop(old_next_prev);
        drop(old_head_next);

        Ok(Entry {
            list: self.list,
            node: new_node,
        })
    }

    /// remove element after this entry
    fn remove_after(&self) -> Option<Entry<'a, T>> {
        // move the drop out of locks
        let old_next_prev;
        let old_head_next;

        // unwrap safety: head is never revmoed
        let curr_node = self.node.lock().unwrap();
        {
            // there is no element after entry
            if Arc::ptr_eq(&curr_node, &self.list.tail) {
                self.node.unlock();
                return None;
            }

            // unwrap safety: next must be valid since it's still in the list
            let next_node = curr_node.lock().unwrap();
            {
                old_next_prev = next_node.set_prev_node(self.node);
                old_head_next = self.node.next.write(next_node.clone());
            }
            curr_node.unlock_remove();
            curr_node.clear_prev_node();
        }
        self.node.unlock();

        // don't clear the next field, could break the iterator
        // curr_node.next.take();
        drop(old_next_prev);
        drop(old_head_next);

        Some(Entry {
            list: self.list,
            node: curr_node,
        })
    }

    /// Insert an element ahead of the entry, and returns the new Entry to it.
    pub fn insert_ahead(&self, elt: T) -> Result<Entry<'a, T>, T> {
        let new_node = Arc::new(Node::new(elt));

        // move the drop out of locks
        let old_node_prev;
        let old_prev_next;

        // should always success since the tail is never removed
        let prev_node = match self.node.lock_prev_node() {
            Ok(node) => node,
            Err(_) => {
                let node = Arc::into_inner(new_node).unwrap();
                return Err(node.data.unwrap());
            }
        };
        {
            new_node.set_prev_node(&prev_node);

            new_node.try_lock().unwrap();
            {
                old_node_prev = self.node.set_prev_node(&new_node);
                new_node.next.write(self.node.clone());
                old_prev_next = prev_node.next.write(new_node.clone());
            }
            new_node.unlock();
        }
        prev_node.unlock();

        drop(old_node_prev);
        drop(old_prev_next);

        Ok(Entry {
            list: self.list,
            node: new_node,
        })
    }

    /// Pops the back element of the list, returns `None` if the list is empty.
    fn remove_ahead(&self) -> Option<Entry<'a, T>> {
        loop {
            // move the drop out of locks
            let old_tail_prev;
            let old_prev_next;

            let curr_node = match self.node.prev_node() {
                Some(node) => node,
                None => continue,
            };

            // the list is empty
            if Arc::ptr_eq(&curr_node, &self.list.head) {
                return None;
            }

            // try to lock the tail.prev.prev node
            let prev_node = match curr_node.lock_prev_node() {
                Ok(node) => node,
                Err(_) => continue,
            };

            {
                // lock the curr node
                let next_node = curr_node.lock().unwrap();
                {
                    // after lock curr_node some thing changed, try again
                    if !Arc::ptr_eq(&next_node, self.node) {
                        curr_node.unlock();
                        prev_node.unlock();
                        continue;
                    }

                    old_tail_prev = self.node.set_prev_node(&prev_node);
                    old_prev_next = prev_node.next.write(next_node).unwrap();
                }
                curr_node.unlock_remove();
                curr_node.clear_prev_node();
            }
            prev_node.unlock();

            // since we are pop from back, the next could be released
            curr_node.next.take();

            drop(old_tail_prev);
            drop(old_prev_next);

            return Some(Entry {
                list: self.list,
                node: curr_node,
            });
        }
    }
}

#[cfg(test)]
mod tests {
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
}
