use alloc::sync::{Arc, Weak};
// use rcu_cell::arc::{Arc, Weak};
use rcu_cell::RcuCell;

use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use core::{cmp, fmt};

#[derive(Debug)]
#[repr(align(64))]
struct Node<T> {
    version: AtomicUsize,
    next: RcuCell<Node<T>>,
    // this is actually Weak<Node<T>>, but we can't use Weak in atomic
    // we use Weak to avoid reference cycles
    prev: AtomicPtr<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            version: AtomicUsize::new(1 << 63),
            prev: AtomicPtr::new(Weak::<Node<T>>::new().into_raw() as *mut Node<T>),
            next: RcuCell::none(),
            data: None,
        }
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        let dummy_ptr = Weak::<Node<T>>::new().into_raw() as *mut Node<T>;
        let prev = self.prev.swap(dummy_ptr, Ordering::Relaxed);
        let _ = self._weak_from_ptr(prev);

        // // check normal node is removed
        // let version = self.version.load(Ordering::SeqCst);
        // if version & (1 << 63) == 0 {
        //     assert!(version & 1 == 1);
        //     self.version.store(0, Ordering::Relaxed);
        // }

        // if we hold the node for a long time
        // the next nodes are still referenced
        // and when drop it could cause stack overflow!
    }
}

#[derive(Debug)]
enum NodeTryLockErr {
    Removed,
    Locked,
}

impl<T> Node<T> {
    #[inline]
    fn new(data: T) -> Self {
        Node {
            version: AtomicUsize::new(0),
            prev: AtomicPtr::new(Weak::<Node<T>>::new().into_raw() as *mut Node<T>),
            next: RcuCell::none(),
            data: Some(data),
        }
    }

    #[inline]
    fn try_lock(&self) -> Result<usize, NodeTryLockErr> {
        let version = self.version.load(Ordering::Relaxed);
        // first bit means node is removed
        if version & 1 == 1 {
            // only when node is removed we return Error
            // otherwise we should check the return value
            return Err(NodeTryLockErr::Removed);
        }

        // second bit means node is locked
        if version & 2 == 2 {
            return Err(NodeTryLockErr::Locked);
        }

        match self.version.compare_exchange(
            version,
            version + 2,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                core::sync::atomic::fence(Ordering::Acquire);
                Ok(version)
            }
            Err(v) if v & 1 == 1 => Err(NodeTryLockErr::Removed),
            Err(_) => Err(NodeTryLockErr::Locked),
        }
    }

    // lock the current node and return the it's next node
    #[inline]
    fn lock(self: &Arc<Self>) -> Result<Arc<Node<T>>, NodeTryLockErr> {
        let backoff = crossbeam_utils::Backoff::new();
        loop {
            match self.try_lock() {
                Ok(_) => {
                    let next_node = self.next_node();
                    assert!(next_node.prev_eq(self));
                    // assert!(!Arc::ptr_eq(self, &next_node));
                    // while !next_node.prev_eq(self) {
                    //     println!("curr_node: {:p}", Arc::as_ptr(self));
                    //     println!("next_node: {:p}", Arc::as_ptr(&next_node));
                    //     println!("prev_node: {:p}", next_node.prev);
                    //     backoff.snooze();
                    // }
                    return Ok(next_node);
                }
                Err(NodeTryLockErr::Locked) => backoff.snooze(),
                Err(NodeTryLockErr::Removed) => return Err(NodeTryLockErr::Removed),
            }
        }
    }

    #[inline]
    fn unlock(&self) {
        core::sync::atomic::fence(Ordering::Release);
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version + 2, Ordering::Release);
    }

    #[inline]
    fn unlock_remove(&self) {
        core::sync::atomic::fence(Ordering::Release);
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version + 3, Ordering::Release);
    }

    #[inline]
    fn is_removed(&self) -> bool {
        self.version.load(Ordering::Acquire) & 1 == 1
    }

    #[inline]
    fn _weak_from_ptr(&self, ptr: *const Node<T>) -> Weak<Node<T>> {
        core::sync::atomic::fence(Ordering::SeqCst);
        // Safety: internal function, make sure ptr load from self.prev
        unsafe { Weak::from_raw(ptr) }
    }

    #[inline]
    fn prev_node(&self) -> Option<Arc<Node<T>>> {
        let prev_ptr = self.prev.load(Ordering::Acquire);
        // it may fail to upgrade if the prev node is removed
        let prev = ManuallyDrop::new(self._weak_from_ptr(prev_ptr));
        prev.upgrade()
    }

    #[inline]
    fn prev_eq(&self, prev: &Arc<Node<T>>) -> bool {
        self.prev.load(Ordering::Acquire) == Arc::as_ptr(prev) as *mut _
    }

    #[inline]
    fn set_prev_node(self: &Arc<Self>, prev: &Arc<Node<T>>) {
        core::sync::atomic::fence(Ordering::Release);
        let prev_ptr = Weak::into_raw(Arc::downgrade(prev)) as *mut Node<T>;
        let _old_prev_ptr = self.prev.swap(prev_ptr, Ordering::Release);
        // FIXME: don't know why
        let _ = self._weak_from_ptr(_old_prev_ptr);
    }

    #[inline]
    fn clear_prev_node(&self) {
        core::sync::atomic::fence(Ordering::Release);
        let prev_ptr = Weak::into_raw(Weak::<Node<T>>::new()) as *mut Node<T>;
        let _old_prev_ptr = self.prev.swap(prev_ptr, Ordering::Release);
        let _ = self._weak_from_ptr(_old_prev_ptr);
    }

    #[inline]
    fn next_node(&self) -> Arc<Node<T>> {
        // Safety: the next node is always valid except for the tail node
        self.next.read().unwrap()
    }

    fn lock_prev_node(self: &Arc<Self>) -> Result<Arc<Node<T>>, ()> {
        loop {
            let prev_node = match self.prev_node() {
                // something wrong, like the prev node is deleted, or the current node is deleted
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
            // CAUTION: if we use try_lock here, it will
            // run Weak::upgrade more frequently, and
            // cause memory issue, still investigating
            if prev_node.lock().is_err() {
                core::hint::spin_loop();
                continue;
            }
            // core::sync::atomic::fence(Ordering::Acquire);

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

            // should wait until the prev is setup
            // if !self.prev_eq(&prev_node) {
            //     println!("wait for prev node to be setup, try again");
            //     core::hint::spin_loop();
            // }

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
            Err(_) => return,
        };
        // unwrap safety: the prev node is locked
        let next_node = curr_node.lock().unwrap();

        next_node.set_prev_node(&prev_node);
        prev_node.next.write(next_node);

        curr_node.unlock_remove();
        curr_node.clear_prev_node();

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
    pub fn is_empty(&self) -> bool {
        self.head.next.arc_eq(&self.tail)
    }

    /// Returns an Entry to the front element, or `None` if the list is empty.
    pub fn front(&self) -> Option<Entry<T>> {
        // head.next is always non empty
        let node = self.head.next_node();
        // only the tail has None data
        node.data.is_some().then(|| Entry(node))
    }

    /// Returns an Entry to the back element, or `None` if the list is empty.
    pub fn back(&self) -> Option<Entry<T>> {
        // tail.prev is always non empty
        let node = loop {
            match self.tail.prev_node() {
                Some(node) => break node,
                // the prev node is dropped, try again
                None => continue,
            }
        };
        // only the head has None data
        node.data.is_some().then(|| Entry(node))
    }

    /// Pushes an element to the front of the list, and returns an Entry to it.
    pub fn push_front(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node::new(elt));
        // unwrap safety: head is never revmoed
        let next_node = self.head.lock().unwrap();
        {
            new_node.try_lock().unwrap();

            new_node.set_prev_node(&self.head);
            next_node.set_prev_node(&new_node);
            new_node.next.write(next_node.clone());
            self.head.next.write(new_node.clone());

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

            next_node.set_prev_node(&self.head);
            self.head.next.write(next_node.clone());

            curr_node.unlock_remove();
            curr_node.clear_prev_node();
        }
        self.head.unlock();
        Some(Entry(curr_node))
    }

    /// Pushes an element to the back of the list, and returns an Entry to it.
    pub fn push_back(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node::new(elt));

        // should always success since the tail is never removed
        let prev_node = self.tail.lock_prev_node().unwrap();
        {
            new_node.try_lock().unwrap();

            // assert!(prev_node.next.arc_eq(&self.tail));
            // assert!(self.tail.prev_eq(&prev_node));
            // while !self.tail.prev_eq(&prev_node) {
            //     println!("wait for prev node to be setup again=====");
            //     core::hint::spin_loop();
            // }

            // once we set it, next push back would lock a different node
            new_node.set_prev_node(&prev_node);
            self.tail.set_prev_node(&new_node);
            new_node.next.write(self.tail.clone());
            prev_node.next.write(new_node.clone());

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

            // lock the curr node
            let next_node = curr_node.lock().unwrap();

            // after lock curr_node some thing changed, try again
            if !Arc::ptr_eq(&next_node, &self.tail) {
                curr_node.unlock();
                prev_node.unlock();
                continue;
            }

            // assert!(prev_node.next.arc_eq(&curr_node));
            // assert!(curr_node.prev_eq(&prev_node));
            // assert!(curr_node.next.arc_eq(&self.tail));
            // assert!(self.tail.prev_eq(&curr_node));

            self.tail.set_prev_node(&prev_node);
            prev_node.next.write(next_node);

            // mark the curr node as removed
            // so it won't be locked by other threads
            curr_node.unlock_remove();
            curr_node.clear_prev_node();
            curr_node.next.take();

            prev_node.unlock();

            return Some(Entry(curr_node));
        }
    }

    /// Returns an iterator over the elements of the list.
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
    fn test_push_pop() {
        let queue = super::LinkedList::new();
        for i in 0..1000 {
            queue.push_back(i);
            assert!(queue.pop_front().is_some());
        }
    }

    #[test]
    fn test_push_front_pop_back() {
        let queue = super::LinkedList::new();
        for i in 0..1 {
            queue.push_front(i);
            assert!(queue.pop_back().is_some());
        }
    }
}
