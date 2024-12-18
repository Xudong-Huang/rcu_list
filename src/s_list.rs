//! A simple concurrent singly linked list

use alloc::sync::Arc;
use rcu_cell::RcuCell;

use core::ops::Deref;
use core::{cmp, fmt};

use crate::version_lock::VersionLock;

#[derive(Debug)]
struct Node<T> {
    // mark the node is removed
    version: VersionLock,
    // the next node, None means the end of the list
    next: RcuCell<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

/// An entry in a `LinkedList`. You can `deref` it to get the value.
#[derive(Clone)]
pub struct Entry<'a, T> {
    list: &'a LinkedList<T>,
    node: Arc<Node<T>>,
}

impl<T> Entry<'_, T> {
    /// Inserts an element after the current entry and returns the new entry.
    /// If the current entry is removed, the element will be returned in `Err`.
    pub fn insert_after(&self, elt: T) -> Entry<T> {
        let node = EntryImpl::new(self.list, &self.node).insert_after(elt);
        Entry {
            list: self.list,
            node,
        }
    }

    /// Removes the element after the current entry and returns it.
    pub fn remove_after(&self) -> Option<Entry<T>> {
        EntryImpl::new(self.list, &self.node)
            .remove_after()
            .map(|node| Entry {
                list: self.list,
                node,
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

struct EntryImpl<'a, 'b, T> {
    list: &'a LinkedList<T>,
    node: &'b Arc<Node<T>>,
}

impl<'a, 'b, T> EntryImpl<'a, 'b, T> {
    fn new(list: &'a LinkedList<T>, node: &'b Arc<Node<T>>) -> Self {
        Self { list, node }
    }

    fn insert_after(&self, elt: T) -> Arc<Node<T>> {
        let new_node = Arc::new(Node {
            version: VersionLock::new(),
            next: RcuCell::none(),
            data: Some(elt),
        });

        let new_node1 = new_node.clone();

        let old_next = self.node.next.update(|next| {
            if let Some(next) = next {
                new_node1.next.write(next);
            }
            Some(new_node1)
        });

        if old_next.is_none() {
            // update the tail to the new node
            self.list.tail.update(|tail| {
                let tail = tail.unwrap(); // tail is never none
                if Arc::ptr_eq(&tail, self.node) {
                    Some(new_node.clone())
                } else {
                    Some(tail)
                }
            });
        }
        new_node
    }

    fn remove_after(&self) -> Option<Arc<Node<T>>> {
        self.node.next.update(|next| {
            let next = match next {
                Some(next) => next,
                None => return None,
            };
            self.list.tail.update(|tail| {
                let tail = tail.unwrap(); // tail is never none
                if Arc::ptr_eq(&tail, &next) {
                    Some(self.node.clone())
                } else {
                    Some(tail)
                }
            });
            next.next.read()
        })
    }
}

/// Concurrent singly linked list
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
        let head = Arc::new(Node {
            version: VersionLock::new(),
            next: RcuCell::none(),
            data: None,
        });

        let tail = RcuCell::from(head.clone());

        Self { head, tail }
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.tail.arc_eq(&self.head)
    }

    /// Returns the first element of the list, or None if the list is empty.
    pub fn front(&self) -> Option<Entry<T>> {
        self.head.next.read().map(|node| Entry { list: self, node })
    }

    /// Returns the last element of the list, or None if the list is empty.
    pub fn back(&self) -> Option<Entry<T>> {
        self.tail.read().map(|node| Entry { list: self, node })
    }

    /// Appends an element to the back of the list
    pub fn push_back(&self, elt: T) -> Entry<T> {
        let node = Arc::new(Node {
            version: VersionLock::new(),
            next: RcuCell::none(),
            data: Some(elt),
        });

        let new_node = node.clone();
        let new_node1 = node.clone();

        self.tail.update(|tail| {
            let old_tail = tail.unwrap(); // tail is never none
            old_tail.next.write(new_node);
            Some(new_node1)
        });

        Entry { list: self, node }
    }

    /// Insert an element to the front of the list.
    pub fn push_front(&self, elt: T) -> Entry<T> {
        let node = EntryImpl::new(self, &self.head).insert_after(elt);
        Entry { list: self, node }
    }

    /// Removes the first element of the list and returns it,
    pub fn pop_front(&self) -> Option<Entry<T>> {
        EntryImpl::new(self, &self.head)
            .remove_after()
            .map(|node| Entry { list: self, node })
    }

    /// Returns an iterator over the elements of the list.
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
        let next = self.curr.next.read();
        if let Some(ref node) = next {
            self.curr = node.clone();
        }
        next.map(|node| Entry {
            list: self.list,
            node,
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
    fn test_entry() {
        let list = super::LinkedList::new();
        list.push_back(1);
        let entry = list.push_back(2);
        list.push_back(3);

        let entry1 = entry.insert_after(4);
        assert_eq!(*entry1, 4);

        assert_eq!(entry1.remove_after().as_deref(), Some(&3));

        let mut iter = list.iter();
        assert_eq!(*iter.next().unwrap(), 1);
        assert_eq!(*iter.next().unwrap(), 2);
        assert_eq!(*iter.next().unwrap(), 4);
        assert!(iter.next().is_none());
    }
}
