//! A simple concurrent singly linked list

use alloc::sync::Arc;
use rcu_cell::RcuCell;

use core::ops::Deref;
use core::{cmp, fmt};

#[derive(Debug)]
struct Node<T> {
    next: RcuCell<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

/// An entry in a `LinkedList`. You can `deref` it to get the value.
#[derive(Clone)]
pub struct Entry<T>(Arc<Node<T>>);

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

struct EntryImpl<'a, 'b, T> {
    list: &'a LinkedList<T>,
    node: &'b Arc<Node<T>>,
}

impl<'a, 'b, T> EntryImpl<'a, 'b, T> {
    fn new(list: &'a LinkedList<T>, node: &'b Arc<Node<T>>) -> Self {
        Self { list, node }
    }

    fn insert_after(&self, elt: T) {
        let new_node = Arc::new(Node {
            next: RcuCell::none(),
            data: Some(elt),
        });

        let old_next = self.node.next.update(|next| {
            if let Some(next) = next {
                new_node.next.write(next);
            }
            Some(new_node.clone())
        });

        if old_next.is_none() {
            // update the tail to the new node
            self.list.tail.update(|tail| {
                let tail = tail.unwrap(); // tail is never none
                if Arc::ptr_eq(&tail, self.node) {
                    Some(new_node)
                } else {
                    Some(tail)
                }
            });
        }
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
        self.head.next.read().map(Entry)
    }

    /// Returns the last element of the list, or None if the list is empty.
    pub fn back(&self) -> Option<Entry<T>> {
        self.tail.read().map(Entry)
    }

    /// Appends an element to the back of the list
    pub fn push_back(&self, elt: T) {
        let new_node = Arc::new(Node {
            next: RcuCell::none(),
            data: Some(elt),
        });

        self.tail.update(|tail| {
            let old_tail = tail.unwrap(); // tail is never none
            old_tail.next.write(new_node.clone());
            Some(new_node)
        });
    }

    /// Insert an element to the front of the list.
    pub fn push_front(&self, elt: T) {
        EntryImpl::new(self, &self.head).insert_after(elt);
    }

    /// Removes the first element of the list and returns it,
    pub fn pop_front(&self) -> Option<Entry<T>> {
        EntryImpl::new(self, &self.head).remove_after().map(Entry)
    }

    /// Returns an iterator over the elements of the list.
    pub fn iter(&self) -> Iter<T> {
        Iter {
            _list: self,
            curr: self.head.clone(),
        }
    }
}

/// An iterator over the elements of a `LinkedList`.
///
/// This `struct` is created by [`LinkedList::iter()`]. See its
/// documentation for more.
pub struct Iter<'a, T: 'a> {
    _list: &'a LinkedList<T>,
    curr: Arc<Node<T>>,
}

impl<T> Iterator for Iter<'_, T> {
    type Item = Entry<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.curr.next.read();
        if let Some(ref node) = next {
            self.curr = node.clone();
        }
        next.map(Entry)
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
