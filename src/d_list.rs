use alloc::sync::{Arc, Weak};
use rcu_cell::RcuCell;

use core::fmt;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

pub struct Node<T> {
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
            version: AtomicUsize::new(0),
            prev: AtomicPtr::new(Weak::<Node<T>>::new().into_raw() as *mut Node<T>),
            next: RcuCell::none(),
            data: None,
        }
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        let prev_ptr = Weak::into_raw(Weak::<Node<T>>::new()) as *mut Node<T>;
        let old_prev = self.prev.swap(prev_ptr, Ordering::Relaxed);
        let _ = unsafe { Weak::from_raw(old_prev) };
    }
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
    fn try_lock(&self) -> Result<usize, ()> {
        let version = self.version.load(Ordering::Relaxed);
        // first bit means node is removed
        if version & 1 == 1 {
            // only when node is removed we return Error
            // otherwise we should check the return value
            return Err(());
        }

        // second bit means node is locked
        let version = version & !2;

        match self.version.compare_exchange_weak(
            version,
            version + 2,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(version),
            // return MAX means a failed lock here
            Err(_) => Ok(usize::MAX),
        }
    }

    #[inline]
    fn lock(&self) -> Result<usize, ()> {
        let mut version;
        loop {
            version = self.try_lock()?;
            if version != usize::MAX {
                break;
            }
            core::hint::spin_loop();
        }
        Ok(version)
    }

    #[inline]
    fn unlock(&self) {
        self.version.fetch_add(2, Ordering::Relaxed);
    }

    #[inline]
    fn unlock_remove(&self) {
        self.version.fetch_add(3, Ordering::Relaxed);
    }

    #[inline]
    fn is_removed(&self) -> bool {
        self.version.load(Ordering::Relaxed) & 1 == 1
    }

    #[inline]
    fn prev(&self) -> Option<Arc<Node<T>>> {
        let prev_ptr = self.prev.load(Ordering::Acquire);
        // it may fail to upgrade if the prev node is removed
        let prev = unsafe { Weak::from_raw(prev_ptr) };
        ManuallyDrop::new(prev).upgrade()
    }

    #[inline]
    fn set_prev(&self, prev: &Arc<Node<T>>) {
        let weak = Arc::downgrade(prev);
        let prev_ptr = Weak::into_raw(weak) as *mut Node<T>;
        let old_prev = self.prev.swap(prev_ptr, Ordering::Release);
        let _ = unsafe { Weak::from_raw(old_prev) };
    }

    #[inline]
    fn next(&self) -> Arc<Node<T>> {
        // Safety: the next node is always valid except for the tail node
        self.next.read().unwrap()
    }
}

pub struct Entry<T>(Arc<Node<T>>);

impl<T> Deref for Entry<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.data.as_ref().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for Entry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntryRef({:?})", self.0.data.as_ref().unwrap())
    }
}

impl<T: PartialEq> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.data == other.0.data
    }
}

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
    pub fn new() -> Self {
        // this is only used for list head, should never deref it's data
        let head = Arc::new(Node::default());
        let tail = Arc::new(Node::default());

        tail.set_prev(&head);
        head.next.write(tail.clone());

        Self { head, tail }
    }

    pub fn is_empty(&self) -> bool {
        self.head.next.arc_eq(&self.tail)
    }

    pub fn front(&self) -> Option<Entry<T>> {
        // head.next is always non empty
        let node = self.head.next();
        // only the tail has None data
        node.data.is_some().then(|| Entry(node))
    }

    pub fn back(&self) -> Option<Entry<T>> {
        // tail.prev is always non empty
        let node = self.tail.prev().unwrap();
        // only the head has None data
        node.data.is_some().then(|| Entry(node))
    }

    pub fn push_front(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.set_prev(&self.head);
        // unwrap safety: head is never revmoed
        self.head.lock().unwrap();
        let next_node = self.head.next();
        // we don't need to lock the new node, till now no one will see it

        new_node.next.write(next_node.clone());
        self.head.next.write(new_node.clone());

        next_node.set_prev(&new_node);

        self.head.unlock();
        Entry(new_node)
    }

    pub fn pop_front(&self) -> Option<Entry<T>> {
        // unwrap safety: head is never revmoed
        self.head.lock().unwrap();
        if self.is_empty() {
            self.head.unlock();
            return None;
        }

        let next = self.head.next();
        // unwrap safety: next must be valid since it's still in the list
        next.lock().unwrap();
        // we are sure next is not the tail
        let next_next = next.next();
        self.head.next.write(next_next);
        next.unlock_remove();
        self.head.unlock();
        Some(Entry(next))
    }

    fn lock_prev_node(&self, curr_node: &Arc<Node<T>>) -> Result<Arc<Node<T>>, ()> {
        if curr_node.is_removed() {
            return Err(());
        }
        loop {
            let prev_node = match curr_node.prev() {
                // something wrong, like the prev node is deleted, or the current node is deleted
                None => return Err(()),

                // the prev can change due to prev insert/remove
                // we will do more check later
                Some(prev) => prev,
            };
            if prev_node.lock().is_err() {
                // the prev node is removed, try again
                continue;
            }
            if !prev_node.next.arc_eq(curr_node) {
                // the prev node is changed, try again
                prev_node.unlock();
                continue;
            }
            // successfully lock the prev node
            return Ok(prev_node);
        }
    }

    pub fn push_back(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node::new(elt));
        new_node.next.write(self.tail.clone());
        // should always success since the tail is never removed
        let prev_node = self.lock_prev_node(&self.tail).unwrap();
        new_node.set_prev(&prev_node);
        prev_node.next.write(new_node.clone());
        self.tail.set_prev(&new_node);

        prev_node.unlock();
        Entry(new_node)
    }

    // pub fn pop_back(&self) -> Option<Entry<T>> {
    //     loop {
    //         let tail_node = self.tail.read().unwrap();
    //         if Arc::ptr_eq(&tail_node, &self.head) {
    //             return None;
    //         }
    //         let prev_node = match self.lock_prev_node(&tail_node) {
    //             Ok(prev_node) => prev_node,
    //             // the tail is removed, try again
    //             Err(_) => continue,
    //         };
    //         // since the prev node is lock, the curr node should not be removed
    //         tail_node.lock().expect("tail node removed!");

    //         match tail_node.next.read() {
    //             None => {
    //                 // tail node is really the tail
    //                 prev_node.next.take();
    //             }
    //             Some(next_node) => {
    //                 // the tail is advanced
    //                 prev_node.next.write(next_node.clone());
    //                 // next_node.prev = prev_node.prev.clone();
    //             }
    //         }
    //         tail_node.unlock_remove();
    //         prev_node.unlock();
    //         return Some(Entry(tail_node));
    //     }
    // }

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
        let next = self.curr.next.read().unwrap();
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
}
