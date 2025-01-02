use crossbeam_epoch::{Atomic, Guard, Shared};

use alloc::boxed::Box;
use core::ops::Deref;
use core::sync::atomic::Ordering;
use core::{cmp, fmt};

use crate::version_lock::{LockErr, VersionLock};

/// A node ptr that actually stores `Arc<T>` in it.
/// each doubly linked list node has more than one stuctual references
/// so we use `Arc` to manage the memory.
#[derive(Debug)]
struct EpochPtr<T> {
    ptr: Atomic<T>,
}

impl<T> EpochPtr<T> {
    const fn null() -> Self {
        Self {
            ptr: Atomic::null(),
        }
    }

    fn read<'g>(&self, guard: &'g Guard) -> Option<&'g T> {
        let shared = self.ptr.load(Ordering::Acquire, guard);
        unsafe { shared.as_ref() }
    }

    fn write(&self, data: &T) {
        let shared_ptr = Shared::from(data as *const T);
        self.ptr.store(shared_ptr, Ordering::Release);
    }

    fn ptr_eq(&self, data: &T, guard: &Guard) -> bool {
        self.ptr.load(Ordering::Relaxed, guard).as_raw() == data
    }
}

#[derive(Debug)]
#[repr(align(64))]
struct Node<T> {
    version: VersionLock,
    next: EpochPtr<Node<T>>,
    prev: EpochPtr<Node<T>>,
    // only the head node and tail node has None data
    data: Option<T>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Node {
            version: VersionLock::new(),
            prev: EpochPtr::null(),
            next: EpochPtr::null(),
            data: None,
        }
    }
}

impl<T> Node<T> {
    #[inline]
    fn new(data: T) -> Self {
        Node {
            version: VersionLock::new(),
            prev: EpochPtr::null(),
            next: EpochPtr::null(),
            data: Some(data),
        }
    }

    #[inline]
    fn try_lock(&self) -> Result<usize, LockErr> {
        self.version.try_lock()
    }

    // lock the current node and return it's next node
    #[inline]
    fn lock<'g>(&self, guard: &'g Guard) -> Result<&'g Node<T>, LockErr> {
        self.version.lock()?;
        let next_node = self.next_node(guard);
        assert!(next_node.prev_eq(self, guard));
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
    fn prev_node<'g>(&self, guard: &'g Guard) -> &'g Node<T> {
        self.prev.read(guard).unwrap()
    }

    #[inline]
    fn prev_eq(&self, prev: &Node<T>, guard: &Guard) -> bool {
        self.prev.ptr_eq(prev, guard)
    }

    #[inline]
    fn set_prev_node(&self, prev: &Node<T>) {
        self.prev.write(prev)
    }

    #[inline]
    fn next_node<'g>(&self, guard: &'g Guard) -> &'g Node<T> {
        // Safety: the next node is always valid except for the tail node
        self.next.read(guard).unwrap()
    }

    fn lock_prev_node<'g>(&self, guard: &'g Guard) -> Result<&'g Node<T>, LockErr> {
        let backoff = crossbeam_utils::Backoff::new();
        loop {
            if self.is_removed() {
                return Err(LockErr::Removed);
            }

            let prev_node = self.prev_node(guard);

            // if the prev node is removed, try again
            if prev_node.lock(guard).is_err() {
                backoff.spin();
                continue;
            }

            // check current node is not removed
            if self.is_removed() {
                prev_node.unlock();
                return Err(LockErr::Removed);
            }

            // if the prev node is changed, try again
            if !prev_node.next.ptr_eq(self, guard) {
                prev_node.unlock();
                backoff.reset();
                continue;
            }

            assert!(self.prev_eq(prev_node, guard));

            // successfully lock the prev node
            return Ok(prev_node);
        }
    }
}

/// An entry in a `LinkedList`.
#[derive(Clone, Copy)]
pub struct Entry<'g, T> {
    list: &'g LinkedList<T>,
    guard: &'g Guard,
    node: &'g Node<T>,
}

impl<'g, T> Entry<'g, T> {
    /// Replace the entry with new value,
    /// and return the old Entry which is marked as removed.
    /// If the node is alredy removed, return the passed in value in Err().
    /// Internally we create a new node to replace the old entry
    pub fn replace(&self, elt: T) -> Result<Entry<'g, T>, T> {
        EntryImpl::new(self.list, self.node, self.guard).replace(elt)
    }

    /// Remove the entry from the list.
    pub fn remove(&self) {
        EntryImpl::new(self.list, self.node, self.guard).remove()
    }

    /// insert an element after the entry.
    /// if the entry was removed, the element will be returned in Err()
    pub fn insert_after(&self, elt: T) -> Result<Entry<'g, T>, T> {
        EntryImpl::new(self.list, self.node, self.guard).insert_after(elt)
    }

    /// insert an element ahead the entry.
    /// if the entry was removed, the element will be returned in Err()
    pub fn insert_ahead(&self, elt: T) -> Result<Entry<'g, T>, T> {
        EntryImpl::new(self.list, self.node, self.guard).insert_ahead(elt)
    }

    /// Remove the entry after this entry.
    pub fn remove_after(&self) -> Option<Entry<'g, T>> {
        EntryImpl::new(self.list, self.node, self.guard).remove_after()
    }

    /// Remove the entry ahead this entry.
    pub fn remove_ahead(&self) -> Option<Entry<'g, T>> {
        EntryImpl::new(self.list, self.node, self.guard).remove_ahead()
    }

    /// Returns true if the entry is removed.
    pub fn is_removed(&self) -> bool {
        self.node.is_removed()
    }

    /// Returns the next entry in the list.
    /// Returns `None` if the entry is removed.
    pub fn next(&self) -> Option<Entry<'g, T>> {
        if self.is_removed() {
            return None;
        }

        let next = self.node.next.read(self.guard)?;
        if core::ptr::addr_eq(self.node, self.list.tail.as_ref()) {
            // we will not return the tail node as an entry
            return None;
        }
        Some(Entry {
            list: self.list,
            guard: self.guard,
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
    head: Box<Node<T>>,
    tail: Box<Node<T>>,
}

impl<T> Default for LinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for LinkedList<T> {
    fn drop(&mut self) {
        let guard = unsafe { crossbeam_epoch::unprotected() };
        // avoid stack overflow
        while self.pop_front(guard).is_some() {}
    }
}

impl<T> LinkedList<T> {
    /// Creates a new empty `LinkedList`.
    pub fn new() -> Self {
        // this is only used for list head, should never deref it's data
        let head = Box::new(Node::default());
        let tail = Box::new(Node::default());

        tail.prev.write(&head);
        head.next.write(&tail);

        Self { head, tail }
    }

    /// Returns true if the list is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        let guard = unsafe { crossbeam_epoch::unprotected() };
        self.head.next.ptr_eq(&self.tail, guard)
    }

    /// Returns an Entry to the front element, or `None` if the list is empty.
    #[inline]
    pub fn front<'l: 'g, 'g>(&'l self, guard: &'g Guard) -> Option<Entry<'g, T>> {
        // head.next is always non empty
        let node = self.head.next_node(guard);
        // only the tail has None data
        node.data.is_some().then_some(Entry {
            list: self,
            node,
            guard,
        })
    }

    /// Returns an Entry to the back element, or `None` if the list is empty.
    #[inline]
    pub fn back<'l: 'g, 'g>(&'l self, guard: &'g Guard) -> Option<Entry<'g, T>> {
        // tail.prev is always non empty
        let node = self.tail.prev_node(guard);
        // only the head has None data
        node.data.is_some().then_some(Entry {
            list: self,
            node,
            guard,
        })
    }

    /// Pushes an element to the front of the list, and returns an Entry to it.
    pub fn push_front<'l: 'g, 'g>(&'l self, elt: T, guard: &'g Guard) -> Entry<'g, T> {
        match EntryImpl::new(self, &self.head, guard).insert_after(elt) {
            Ok(entry) => entry,
            Err(_) => unreachable!("push_front should always success"),
        }
    }

    /// Pops the front element of the list, returns `None` if the list is empty.
    pub fn pop_front<'l: 'g, 'g>(&'l self, guard: &'g Guard) -> Option<Entry<'g, T>> {
        EntryImpl::new(self, &self.head, guard).remove_after()
    }

    /// Pushes an element to the back of the list, and returns an Entry to it.
    pub fn push_back<'l: 'g, 'g>(&'l self, elt: T, guard: &'g Guard) -> Entry<'g, T> {
        match EntryImpl::new(self, &self.tail, guard).insert_ahead(elt) {
            Ok(entry) => entry,
            Err(_) => unreachable!("push_back should always success"),
        }
    }

    /// Pops the back element of the list, returns `None` if the list is empty.
    pub fn pop_back<'l: 'g, 'g>(&'l self, guard: &'g Guard) -> Option<Entry<'g, T>> {
        EntryImpl::new(self, &self.tail, guard).remove_ahead()
    }

    /// Returns an iterator over the elements of the list.
    #[inline]
    pub fn iter<'l: 'g, 'g>(&'l self, guard: &'g Guard) -> Iter<'l, 'g, T> {
        Iter {
            list: self,
            curr: &self.head,
            guard,
        }
    }

    /// Returns a pinned version of the list
    pub fn pin(&self) -> PinedLinkedList<T> {
        PinedLinkedList {
            list: self,
            guard: crossbeam_epoch::pin(),
        }
    }
}

pub struct PinedLinkedList<'l, T> {
    list: &'l LinkedList<T>,
    guard: Guard,
}

impl<'l, T> PinedLinkedList<'l, T> {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn front(&self) -> Option<Entry<T>> {
        self.list.front(&self.guard)
    }

    pub fn back(&self) -> Option<Entry<T>> {
        self.list.back(&self.guard)
    }

    pub fn push_front(&self, elt: T) -> Entry<T> {
        self.list.push_front(elt, &self.guard)
    }

    pub fn pop_front(&self) -> Option<Entry<T>> {
        self.list.pop_front(&self.guard)
    }

    pub fn push_back(&self, elt: T) -> Entry<T> {
        self.list.push_back(elt, &self.guard)
    }

    pub fn pop_back(&self) -> Option<Entry<T>> {
        self.list.pop_back(&self.guard)
    }

    pub fn iter(&self) -> Iter<'l, '_, T> {
        self.list.iter(&self.guard)
    }
}

/// An iterator over the elements of a `LinkedList`.
///
/// This `struct` is created by [`LinkedList::iter()`]. See its
/// documentation for more.
pub struct Iter<'l: 'g, 'g, T: 'l> {
    list: &'l LinkedList<T>,
    curr: &'g Node<T>,
    guard: &'g Guard,
}

impl<'g, T> Iterator for Iter<'_, 'g, T> {
    type Item = Entry<'g, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let next_node = self.curr.next.read(self.guard)?;
        self.curr = next_node;
        next_node.data.is_some().then_some(Entry {
            list: self.list,
            node: next_node,
            guard: self.guard,
        })
    }
}

impl<'g, T> DoubleEndedIterator for Iter<'_, 'g, T> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        let prev_node = self.curr.prev.read(self.guard)?;
        self.curr = prev_node;
        prev_node.data.is_some().then_some(Entry {
            list: self.list,
            node: prev_node,
            guard: self.guard,
        })
    }
}

struct EntryImpl<'a, 'g, T> {
    list: &'a LinkedList<T>,
    node: &'g Node<T>,
    guard: &'g Guard,
}

impl<'a: 'g, 'g, T> EntryImpl<'a, 'g, T> {
    #[inline]
    fn new(list: &'a LinkedList<T>, node: &'g Node<T>, guard: &'g Guard) -> Self {
        Self { list, node, guard }
    }

    /// Remove the entry from the list.
    fn remove(self) {
        let curr_node = self.node;
        let prev_node = match curr_node.lock_prev_node(self.guard) {
            Ok(node) => node,
            // the current node is already removed
            Err(_) => return,
        };

        {
            // unwrap safety: the prev node is locked
            let next_node = curr_node.lock(self.guard).unwrap();
            {
                next_node.set_prev_node(prev_node);
                prev_node.next.write(next_node);
            }
            curr_node.unlock_remove();
        }
        prev_node.unlock();
    }

    /// Replace the entry with new value,
    fn replace(&self, elt: T) -> Result<Entry<'g, T>, T> {
        let new_node = Box::new(Node::new(elt));
        new_node.next.write(self.node);

        let prev_node = match self.node.lock_prev_node(self.guard) {
            Ok(node) => node,
            Err(_) => {
                let node = *new_node;
                return Err(node.data.unwrap());
            }
        };
        let new_node = unsafe { &*Box::into_raw(new_node) };
        {
            new_node.set_prev_node(prev_node);
            new_node.try_lock().unwrap();
            self.node.lock(self.guard).unwrap();
            {
                let next_node = self.node.next_node(self.guard);
                next_node.set_prev_node(new_node);
                new_node.next.write(next_node);
                prev_node.next.write(new_node);
            }
            self.node.unlock_remove();
            new_node.unlock();
        }
        prev_node.unlock();

        Ok(Entry {
            list: self.list,
            node: new_node,
            guard: self.guard,
        })
    }

    /// insert an element after the entry.
    /// if the entry was removed, the element will be returned in Err()
    fn insert_after(&self, elt: T) -> Result<Entry<'g, T>, T> {
        let new_node = Box::new(Node::new(elt));
        new_node.set_prev_node(self.node);

        let next_node = match self.node.lock(self.guard) {
            Ok(node) => node,
            Err(_) => {
                // current entry removed, can't insert
                let n = *new_node;
                return Err(n.data.unwrap());
            }
        };
        let new_node = unsafe { &*Box::into_raw(new_node) };
        {
            // new_node.try_lock().unwrap();
            {
                new_node.next.write(next_node);
                next_node.set_prev_node(new_node);
                self.node.next.write(new_node);
            }
            // new_node.unlock();
        }
        self.node.unlock();

        Ok(Entry {
            list: self.list,
            node: new_node,
            guard: self.guard,
        })
    }

    /// remove element after this entry
    fn remove_after(&self) -> Option<Entry<'g, T>> {
        let curr_node = match self.node.lock(self.guard) {
            Ok(node) => node,
            Err(_) => return None,
        };
        {
            // there is no element after entry
            if core::ptr::eq(curr_node, self.list.tail.as_ref()) {
                self.node.unlock();
                return None;
            }

            // unwrap safety: next must be valid since it's still in the list
            let next_node = curr_node.lock(self.guard).unwrap();
            {
                next_node.set_prev_node(self.node);
                self.node.next.write(next_node);
            }
            curr_node.unlock_remove();
        }
        self.node.unlock();

        // recycle the old node
        unsafe {
            self.guard.defer_unchecked(move || {
                let _ = Box::from_raw(curr_node as *const Node<T> as *mut Node<T>);
            });
        }

        Some(Entry {
            list: self.list,
            node: curr_node,
            guard: self.guard,
        })
    }

    /// Insert an element ahead of the entry, and returns the new Entry to it.
    pub fn insert_ahead(&self, elt: T) -> Result<Entry<'g, T>, T> {
        let new_node = Box::new(Node::new(elt));
        new_node.next.write(self.node);

        let prev_node = match self.node.lock_prev_node(self.guard) {
            Ok(node) => node,
            Err(_) => {
                let node = *new_node;
                return Err(node.data.unwrap());
            }
        };
        new_node.set_prev_node(prev_node);
        let new_node = unsafe { &*Box::into_raw(new_node) };
        {
            // new_node.try_lock().unwrap();
            {
                self.node.set_prev_node(new_node);
                prev_node.next.write(new_node);
            }
            // new_node.unlock();
        }
        prev_node.unlock();

        Ok(Entry {
            list: self.list,
            node: new_node,
            guard: self.guard,
        })
    }

    /// Remove the element ahead of the entry, returns `None` if the list is empty.
    fn remove_ahead(&self) -> Option<Entry<'g, T>> {
        loop {
            if self.node.is_removed() {
                return None;
            }

            let curr_node = self.node.prev_node(self.guard);

            // the list is empty
            if core::ptr::eq(curr_node, self.list.head.as_ref()) {
                return None;
            }

            // try to lock the node.prev.prev node
            let prev_node = match curr_node.lock_prev_node(self.guard) {
                Ok(node) => node,
                Err(_) => continue,
            };

            {
                // lock the curr node, curr node is not removed
                let next_node = curr_node.lock(self.guard).unwrap();
                {
                    // after lock curr_node some thing changed, try again
                    if !core::ptr::eq(next_node, self.node) {
                        curr_node.unlock();
                        prev_node.unlock();
                        continue;
                    }

                    self.node.set_prev_node(prev_node);
                    prev_node.next.write(next_node);
                }
                curr_node.unlock_remove();
            }
            prev_node.unlock();

            // recycle the old node
            unsafe {
                self.guard.defer_unchecked(move || {
                    let _ = Box::from_raw(curr_node as *const Node<T> as *mut Node<T>);
                });
            }

            return Some(Entry {
                list: self.list,
                node: curr_node,
                guard: self.guard,
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

        let guard = &crossbeam_epoch::pin();

        list.push_back(1, guard);
        assert!(!list.is_empty());
        assert_eq!(*list.front(guard).unwrap(), 1);
        assert_eq!(*list.back(guard).unwrap(), 1);

        list.push_back(2, guard);
        assert_eq!(*list.front(guard).unwrap(), 1);
        assert_eq!(*list.back(guard).unwrap(), 2);

        list.push_front(0, guard);
        assert_eq!(*list.front(guard).unwrap(), 0);
        assert_eq!(*list.back(guard).unwrap(), 2);

        assert_eq!(*list.pop_front(guard).unwrap(), 0);
        assert_eq!(*list.pop_front(guard).unwrap(), 1);
        assert_eq!(*list.pop_front(guard).unwrap(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn test_list_1() {
        let list = super::LinkedList::new();
        assert!(list.is_empty());

        let guard = &crossbeam_epoch::pin();

        list.push_front(1, guard);
        assert!(!list.is_empty());
        assert_eq!(*list.front(guard).unwrap(), 1);
        assert_eq!(*list.back(guard).unwrap(), 1);

        list.push_front(2, guard);
        assert_eq!(*list.front(guard).unwrap(), 2);
        assert_eq!(*list.back(guard).unwrap(), 1);

        list.push_back(0, guard);
        assert_eq!(*list.front(guard).unwrap(), 2);
        assert_eq!(*list.back(guard).unwrap(), 0);

        assert_eq!(*list.pop_back(guard).unwrap(), 0);
        assert_eq!(*list.pop_back(guard).unwrap(), 1);
        assert_eq!(*list.pop_back(guard).unwrap(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn test_remove_entry() {
        let list = super::LinkedList::new();

        let guard = &crossbeam_epoch::pin();

        let entry = list.push_back(1, guard);
        assert!(!entry.is_removed());
        assert!(*entry == 1);
        entry.remove();
        assert!(list.is_empty());
        assert!(list.front(guard).is_none());
        assert!(list.back(guard).is_none());
    }

    #[test]
    fn test_iter() {
        let list = super::LinkedList::new();

        let guard = &crossbeam_epoch::pin();

        list.push_back(1, guard);
        list.push_back(2, guard);
        list.push_back(3, guard);

        let mut iter = list.iter(guard);
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_remove() {
        let list = super::LinkedList::new();

        let guard = &crossbeam_epoch::pin();

        list.push_back(1, guard);
        let entry = list.push_back(2, guard);
        list.push_back(3, guard);

        entry.remove();

        let mut iter = list.iter(guard);
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_insert_after() {
        let list = super::LinkedList::new();

        let guard = &crossbeam_epoch::pin();
        list.push_back(1, guard);
        let entry = list.push_back(2, guard);
        list.push_back(3, guard);

        entry.insert_after(100).unwrap();

        let mut iter = list.iter(guard);
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 100);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());
    }

    #[test]
    fn entry_insert_after_remove() {
        let list = super::LinkedList::new();

        let guard = &crossbeam_epoch::pin();

        list.push_back(1, guard);
        let entry = list.push_back(2, guard);
        list.push_back(3, guard);

        assert_eq!(*entry.insert_after(100).unwrap(), 100);

        let mut iter = list.iter(guard);
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

        let guard = crossbeam_epoch::pin();

        for i in 0..100 {
            list.push_back(Foo::new(i), &guard);
        }

        list.pop_back(&guard);

        drop(list);

        // force drop all the garbage
        drop(guard);
        for _ in 0..128 {
            crossbeam_epoch::pin().flush();
        }
        assert_eq!(REF.load(Ordering::Relaxed), (0..100).sum());
    }

    #[test]
    fn entry_replace() {
        let list = super::LinkedList::new();
        let list = list.pin();

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
}
