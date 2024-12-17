use alloc::sync::{Arc, Weak};
use rcu_cell::{RcuCell, RcuWeak};

use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{cmp, fmt};

#[derive(Debug)]
enum NodeTryLockErr {
    Removed,
    Retry,
}

#[derive(Debug)]
#[repr(align(64))]
struct Node<T> {
    version: AtomicUsize,
    next: RcuCell<Node<T>>,
    prev: RcuWeak<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            version: AtomicUsize::new(1 << 63),
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
            version: AtomicUsize::new(0),
            prev: RcuWeak::new(),
            next: RcuCell::none(),
            data: Some(data),
        }
    }

    #[inline]
    fn version_generation(&self, version: usize) -> usize {
        // first bit means node is removed
        // second bit means node is locked
        // valid generations are 0, 4, 8, 12...
        if version & 2 == 0 {
            // not locked, use current generation
            version
        } else {
            // locked, use next generation to try
            version + 2
        }
    }

    #[inline]
    fn try_lock(&self) -> Result<usize, NodeTryLockErr> {
        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(NodeTryLockErr::Removed);
        }

        let version = self.version_generation(version);
        match self.version.compare_exchange(
            version,
            version + 2,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(v) => {
                core::sync::atomic::fence(Ordering::Acquire);
                Ok(v)
            }

            Err(v) => {
                if v & 1 == 1 {
                    Err(NodeTryLockErr::Removed)
                } else {
                    Err(NodeTryLockErr::Retry)
                }
            }
        }
    }

    // lock the current node and return it's next node
    #[inline]
    fn lock(self: &Arc<Self>) -> Result<Arc<Node<T>>, NodeTryLockErr> {
        let backoff = crossbeam_utils::Backoff::new();

        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(NodeTryLockErr::Removed);
        }

        let mut version = self.version_generation(version);
        while let Err(v) = self.version.compare_exchange_weak(
            version,
            version + 2,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            if v & 1 == 1 {
                return Err(NodeTryLockErr::Removed);
            }
            version = self.version_generation(v);
            backoff.snooze();
        }

        core::sync::atomic::fence(Ordering::Acquire);
        let next_node = self.next_node();
        assert!(next_node.prev_eq(self));
        Ok(next_node)
    }

    #[inline]
    fn unlock(&self) {
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version + 2, Ordering::Release);
    }

    #[inline]
    fn unlock_remove(&self) {
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version + 3, Ordering::Release);
    }

    #[inline]
    fn is_removed(&self) -> bool {
        self.version.load(Ordering::Relaxed) & 1 == 1
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
pub struct Entry<T>(Arc<Node<T>>);

impl<T> Entry<T> {
    /// Remove the entry from the list.
    pub fn remove(self) {
        let curr_node = &self.0;
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

    /// Returns true if the entry is removed.
    pub fn is_removed(&self) -> bool {
        self.0.is_removed()
    }
}

impl<T> Deref for Entry<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.data.as_ref().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for Entry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entry({:?})", self.0.data.as_ref().unwrap())
    }
}

impl<T: PartialEq> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.data == other.0.data
    }
}

impl<T> AsRef<T> for Entry<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: PartialOrd> PartialOrd for Entry<T> {
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

impl<T: Ord> Ord for Entry<T> {
    fn cmp(&self, other: &Entry<T>) -> cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: Eq> Eq for Entry<T> {}

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
        node.data.is_some().then(|| Entry(node))
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
        node.data.is_some().then(|| Entry(node))
    }

    /// Pushes an element to the front of the list, and returns an Entry to it.
    pub fn push_front(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.set_prev_node(&self.head);
        // unwrap safety: head is never removed
        let next_node = self.head.lock().unwrap();
        {
            new_node.try_lock().unwrap();
            {
                next_node.set_prev_node(&new_node);
                new_node.next.write(next_node.clone());
                self.head.next.write(new_node.clone());
            }
            new_node.unlock();
        }
        self.head.unlock();
        Entry(new_node)
    }

    /// Pops the front element of the list, returns `None` if the list is empty.
    pub fn pop_front(&self) -> Option<Entry<T>> {
        // unwrap safety: head is never revmoed
        let curr_node = self.head.lock().unwrap();
        {
            // the list is empty
            if Arc::ptr_eq(&curr_node, &self.tail) {
                self.head.unlock();
                return None;
            }

            // unwrap safety: next must be valid since it's still in the list
            let next_node = curr_node.lock().unwrap();
            {
                next_node.set_prev_node(&self.head);
                self.head.next.write(next_node.clone());
            }
            curr_node.unlock_remove();
            curr_node.clear_prev_node();
        }
        self.head.unlock();

        // don't clear the next field, could break the iterator
        // curr_node.next.take();

        Some(Entry(curr_node))
    }

    /// Pushes an element to the back of the list, and returns an Entry to it.
    pub fn push_back(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node::new(elt));

        // should always success since the tail is never removed
        let prev_node = self.tail.lock_prev_node().unwrap();
        {
            new_node.set_prev_node(&prev_node);

            new_node.try_lock().unwrap();
            {
                self.tail.set_prev_node(&new_node);
                new_node.next.write(self.tail.clone());
                prev_node.next.write(new_node.clone());
            }
            new_node.unlock();
        }
        prev_node.unlock();

        Entry(new_node)
    }

    /// Pops the back element of the list, returns `None` if the list is empty.
    pub fn pop_back(&self) -> Option<Entry<T>> {
        loop {
            let curr_node = match self.tail.prev_node() {
                Some(node) => node,
                None => continue,
            };

            // the list is empty
            if Arc::ptr_eq(&curr_node, &self.head) {
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
                    if !Arc::ptr_eq(&next_node, &self.tail) {
                        curr_node.unlock();
                        prev_node.unlock();
                        continue;
                    }

                    self.tail.set_prev_node(&prev_node);
                    prev_node.next.write(next_node).unwrap();
                }
                curr_node.unlock_remove();
                curr_node.clear_prev_node();
            }
            prev_node.unlock();

            // since we are pop from back, the next could be released
            curr_node.next.take();

            return Some(Entry(curr_node));
        }
    }

    /// Returns an iterator over the elements of the list.
    #[inline]
    pub fn iter(&self) -> Iter<T> {
        Iter {
            tail: &self.tail,
            curr: self.head.clone(),
        }
    }
}

/// An iterator over the elements of a `LinkedList`.
///
/// This `struct` is created by [`LinkedList::iter()`]. See its
/// documentation for more.
pub struct Iter<'a, T: 'a> {
    tail: &'a Arc<Node<T>>,
    curr: Arc<Node<T>>,
}

impl<T> Iterator for Iter<'_, T> {
    type Item = Entry<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.curr.next_node();
        self.curr = next.clone();
        if Arc::ptr_eq(&self.curr, self.tail) {
            return None;
        }
        Some(Entry(next))
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
    fn test_push_back_pop_front() {
        let queue = super::LinkedList::new();
        for i in 0..1000 {
            queue.push_back(i);
            assert!(queue.pop_front().is_some());
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn test_push_front_pop_back() {
        let queue = super::LinkedList::new();
        for i in 0..1 {
            queue.push_front(i);
            assert!(queue.pop_back().is_some());
        }
        assert!(queue.is_empty());
    }
}
