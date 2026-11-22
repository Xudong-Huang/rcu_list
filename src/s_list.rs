//! A simple concurrent singly linked list

use alloc::sync::Arc;
use rcu_cell::RcuCell;

use core::ops::Deref;
use core::sync::atomic::Ordering;
use core::{cmp, fmt};

#[derive(Debug)]
struct Node<T> {
    // the next node, None means the end of the list
    next: RcuCell<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Self {
            next: RcuCell::none(),
            data: None,
        }
    }
}

impl<T> Node<T> {
    #[inline]
    fn new(data: T) -> Self {
        Node {
            next: RcuCell::none(),
            data: Some(data),
        }
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        // Removed nodes keep their `next` pointer, so one held `Arc<Node>` can
        // own a whole chain of removed nodes. Walk the chain iteratively
        // instead of letting the recursive drop overflow the stack.
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

/// An entry in a `LinkedList`. You can `deref` it to get the value.
pub struct Entry<'a, T> {
    list: &'a LinkedList<T>,
    node: Arc<Node<T>>,
}

impl<T> Clone for Entry<'_, T> {
    fn clone(&self) -> Self {
        Entry {
            list: self.list,
            node: self.node.clone(),
        }
    }
}

impl<'a, T> Entry<'a, T> {
    /// Returns the next entry in the list, or `None` if this is the last entry.
    pub fn next(&self) -> Option<Entry<'a, T>> {
        let next = self.node.next.read()?;
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
        Arc::ptr_eq(&self.node, &other.node) || self.node.data == other.node.data
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
        if Arc::ptr_eq(&self.node, &other.node) {
            return cmp::Ordering::Equal;
        }
        (**self).cmp(&**other)
    }
}

impl<T: Eq> Eq for Entry<'_, T> {}

struct EntryImpl<'a, 'b, T> {
    list: &'a LinkedList<T>,
    node: &'b Arc<Node<T>>,
}

impl<'a, 'b, T> EntryImpl<'a, 'b, T> {
    fn new(list: &'a LinkedList<T>, node: &'b Arc<Node<T>>) -> Self {
        Self { list, node }
    }

    /// Move the tail back to `self.node` if `next` is still the tail.
    ///
    /// A real CAS is required: it must serialize with push_back's tail update,
    /// and it must happen before the new `head.next` is published by the caller.
    fn rollback_tail(&self, next: &Arc<Node<T>>) {
        // Test the tail first: normally `next` is not the last node, and a
        // failed exchange would clone `self.node` only to have it handed back
        // and dropped. Only clone when the exchange can actually apply.
        while self.list.tail.arc_eq(next) {
            let exchanged = self.list.tail.compare_exchange(
                Some(next),
                Some(self.node.clone()),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            if exchanged.is_ok() {
                break;
            }
        }
    }

    fn insert_after(&self, elt: T) {
        let new_node = Arc::new(Node {
            next: RcuCell::none(),
            data: Some(elt),
        });

        self.node.next.update(|next| {
            if let Some(next) = next {
                new_node.next.write(next.clone());
            } else {
                // update the tail to the new node
                self.list.tail.update(|tail| {
                    // tail is never none
                    let tail = tail.unwrap();
                    if Arc::ptr_eq(tail, self.node) {
                        Some(new_node.clone())
                    } else {
                        Some(tail.clone())
                    }
                });
            }

            Some(new_node)
        });
    }

    fn remove_after(&self) -> Option<Arc<Node<T>>> {
        self.node.next.update(|next| {
            let next = match next {
                Some(next) => next,
                None => return None,
            };
            // Only move the tail back if `next` is still the tail, and do it
            // before the new `head.next` is published below.
            self.rollback_tail(next);
            next.next.read()
        })
    }

    fn remove_after_if<F>(&self, condition: F) -> Option<Arc<Node<T>>>
    where
        F: Fn(&T) -> bool,
    {
        // Check the condition on a borrowed node first: an `update` that finds
        // it unmet has to clone the node just to write it back unchanged. The
        // result is only used to fail fast; if it does match, the `update`
        // below rechecks it, so a concurrent change is still handled.
        let matched = self
            .node
            .next
            .with(|next| next.is_some_and(|next| condition(next.data.as_ref().unwrap())));
        if !matched {
            return None;
        }

        let mut condition_met = false;
        let ret = self.node.next.update(|next| {
            let next = match next {
                Some(next) => next,
                None => return None,
            };

            condition_met = condition(next.data.as_ref().unwrap());
            if !condition_met {
                // condition not met, do nothing
                return Some(next.clone());
            }

            // Only move the tail back if `next` is still the tail; see
            // `rollback_tail`.
            self.rollback_tail(next);
            next.next.read()
        });

        if !condition_met {
            None
        } else {
            ret
        }
    }
}

/// Concurrent singly linked list
///
/// The `tail` cell only needs to be updated when the list becomes empty or
/// stops being empty. Operations that touch both `head.next` and `tail`
/// (`push_front`, `pop_front`, `pop_front_if`, and `push_back` on an empty
/// list) must acquire `head.next` first and `tail` second; acquiring them in
/// the opposite order can deadlock.
#[derive(Debug)]
pub struct LinkedList<T> {
    head: Arc<Node<T>>,
    tail: RcuCell<Node<T>>,
}

impl<T> Default for LinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LinkedList<T> {
    /// Creates a new, empty `LinkedList`.
    pub fn new() -> Self {
        // this is only used for list head, should never deref it's data
        let head = Arc::new(Node::<T>::default());
        let tail = RcuCell::from(head.clone());

        Self { head, tail }
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.tail.arc_eq(&self.head)
    }

    /// Returns the first element of the list, or None if the list is empty.
    pub fn front(&self) -> Option<Entry<'_, T>> {
        self.head.next.read().map(|node| Entry { list: self, node })
    }

    /// Returns the last element of the list, or None if the list is empty.
    pub fn back(&self) -> Option<Entry<'_, T>> {
        let node = self.tail.read()?;
        // on an empty list the tail is the head node, which has None data
        node.data.is_some().then(|| Entry { list: self, node })
    }

    /// Appends an element to the back of the list.
    pub fn push_back(&self, elt: T) -> Entry<'_, T> {
        let node = Arc::new(Node::new(elt));

        // Fast path: if the list is not empty, appending only needs the tail
        // lock. The pushed node is linked after `old_tail` and becomes the new
        // tail.
        //
        // Probe the tail with a plain load first. If it is the head sentinel
        // the list is empty, and the update below would only write the same
        // tail back before taking the slow path anyway.
        if !self.tail.arc_eq(&self.head) {
            let mut tail_is_head = false;
            let new_next = node.clone();
            let new_tail = node.clone();
            self.tail.update(|tail| {
                // tail is never none
                let old_tail = tail.unwrap();
                if Arc::ptr_eq(old_tail, &self.head) {
                    // The tail still points at the head sentinel: either the
                    // list is empty, or a concurrent pop_front has already
                    // moved the tail back but has not published the empty
                    // head.next yet. This needs both the head.next and the tail
                    // locks. Do not take head.next while holding tail here,
                    // that would invert the lock order used by
                    // push_front/pop_front and could deadlock. Leave the tail
                    // unchanged and use the slow path.
                    tail_is_head = true;
                    Some(old_tail.clone())
                } else {
                    old_tail.next.write(new_next);
                    Some(new_tail)
                }
            });
            if !tail_is_head {
                return Entry { list: self, node };
            }
        }

        // Slow path for an empty list: acquire head.next first, then tail,
        // the same order push_front/pop_front use. While head.next is locked
        // no other thread can move the tail back to the head sentinel, and the
        // fast path above never does either, so the emptiness check is stable.
        let new_next = node.clone();
        let new_tail = node.clone();
        let mut tail_was_head = false;
        self.head.next.update(|head_next| {
            self.tail.update(|tail| {
                // tail is never none
                let old_tail = tail.unwrap();
                if Arc::ptr_eq(old_tail, &self.head) {
                    // Still empty: the outer update publishes the new node as
                    // head.next, so head.next must not be written here.
                    tail_was_head = true;
                } else {
                    // A concurrent push_back added a node in the meantime;
                    // append after the current tail instead.
                    old_tail.next.write(new_next);
                }
                Some(new_tail)
            });

            // head.next is empty exactly when the tail is the head sentinel.
            debug_assert_eq!(tail_was_head, head_next.is_none());
            if tail_was_head {
                Some(node.clone())
            } else {
                head_next.cloned()
            }
        });

        Entry { list: self, node }
    }

    /// Insert an element to the front of the list.
    pub fn push_front(&self, elt: T) {
        EntryImpl::new(self, &self.head).insert_after(elt);
    }

    /// Removes the first element of the list and returns it.
    pub fn pop_front(&self) -> Option<Entry<'_, T>> {
        EntryImpl::new(self, &self.head)
            .remove_after()
            .map(|node| Entry { list: self, node })
    }

    /// Removes the first element of the list if it satisfies the condition.
    pub fn pop_front_if<F>(&self, condition: F) -> Option<Entry<'_, T>>
    where
        F: Fn(&T) -> bool,
    {
        EntryImpl::new(self, &self.head)
            .remove_after_if(condition)
            .map(|node| Entry { list: self, node })
    }

    /// Returns an iterator over the elements of the list.
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
        let next = self.curr.next.read()?;
        self.curr = next.clone();
        Some(Entry {
            list: self.list,
            node: next,
        })
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

    #[test]
    fn test_pop_if() {
        let list = super::LinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        assert_eq!(list.pop_front_if(|x| *x == 2), None);
        assert_eq!(list.pop_front_if(|x| *x == 1).as_deref(), Some(&1));
    }

    #[test]
    fn empty_front_back() {
        let list: super::LinkedList<i32> = super::LinkedList::new();
        assert!(list.is_empty());
        // the tail is the head node on an empty list, neither end is an entry
        assert!(list.front().is_none());
        assert!(list.back().is_none());
    }

    #[test]
    fn back_after_pop_last() {
        let list = super::LinkedList::new();
        list.push_back(1);
        assert_eq!(*list.back().unwrap(), 1);

        assert_eq!(*list.pop_front().unwrap(), 1);
        assert!(list.is_empty());
        assert!(list.back().is_none());
        assert!(list.front().is_none());
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
    fn back_with_both_ends() {
        let list = super::LinkedList::new();
        list.push_front(2);
        list.push_back(0);
        assert_eq!(*list.front().unwrap(), 2);
        assert_eq!(*list.back().unwrap(), 0);
        assert!(!list.is_empty());

        assert_eq!(*list.pop_front().unwrap(), 2);
        assert_eq!(*list.back().unwrap(), 0);

        assert_eq!(*list.pop_front().unwrap(), 0);
        assert!(list.back().is_none());
    }

    #[test]
    fn entry_next() {
        use alloc::vec;
        use alloc::vec::Vec;

        let list = super::LinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        // Walk the list through `Entry::next`; the chain must end after the
        // last data node instead of exposing an invalid entry.
        let mut values = Vec::new();
        let mut entry = list.front();
        while let Some(e) = entry {
            values.push(*e);
            entry = e.next();
        }
        assert_eq!(values, vec![1, 2, 3]);
    }

    #[test]
    fn iter_walks_past_removed_front() {
        let list = super::LinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);

        // The node the iterator is positioned on is removed, but a removed
        // node keeps its `next`, so the iterator can still walk past it.
        let removed = list.pop_front().unwrap();
        assert_eq!(*removed, 1);
        assert_eq!(*removed.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 3);
        assert!(iter.next().is_none());

        // A fresh iterator only sees the live nodes.
        assert!(list.iter().map(|e| *e).eq([2, 3]));
    }

    #[test]
    fn pop_front_if_empty() {
        let list: super::LinkedList<i32> = super::LinkedList::new();
        assert!(list.pop_front_if(|_| true).is_none());
        assert!(list.is_empty());
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
    fn default_is_empty() {
        let list: super::LinkedList<i32> = Default::default();
        assert!(list.is_empty());
        assert!(list.iter().next().is_none());
    }
}
