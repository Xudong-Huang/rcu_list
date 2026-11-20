//! A simple concurrent singly linked list

use alloc::sync::Arc;
use rcu_cell::RcuCell;

use core::ops::Deref;
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

/// An entry in a `LinkedList`. You can `deref` it to get the value.
#[derive(Clone)]
pub struct Entry<'a, T> {
    list: &'a LinkedList<T>,
    node: Arc<Node<T>>,
}

impl<'a, T> Entry<'a, T> {
    /// Returns the next entry in the list.
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

    fn insert_after(&self, elt: T) {
        let new_node = Arc::new(Node {
            next: RcuCell::none(),
            data: Some(elt),
        });

        self.node.next.update(|next| {
            if let Some(next) = next {
                new_node.next.write(next);
            } else {
                // update the tail to the new node
                self.list.tail.update(|tail| {
                    // tail is never none
                    let tail = tail.unwrap();
                    if Arc::ptr_eq(&tail, self.node) {
                        Some(new_node.clone())
                    } else {
                        Some(tail)
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
            self.list.tail.update(|tail| {
                // tail is never none
                let tail = tail.unwrap();
                if Arc::ptr_eq(&tail, &next) {
                    Some(self.node.clone())
                } else {
                    Some(tail)
                }
            });
            next.next.read()
        })
    }

    fn remove_after_if<F>(&self, condition: F) -> Option<Arc<Node<T>>>
    where
        F: Fn(&T) -> bool,
    {
        let mut condition_met = false;
        let ret = self.node.next.update(|next| {
            let next = match next {
                Some(next) => next,
                None => return None,
            };

            condition_met = condition(next.data.as_ref().unwrap());
            if !condition_met {
                // condition not met, do nothing
                return Some(next);
            }

            self.list.tail.update(|tail| {
                // tail is never none
                let tail = tail.unwrap();
                if Arc::ptr_eq(&tail, &next) {
                    Some(self.node.clone())
                } else {
                    Some(tail)
                }
            });
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
/// (`push_front`, `pop_front`, `remove_after_if`, and `push_back` on an
/// empty list) must acquire `head.next` first and `tail` second; acquiring
/// them in the opposite order can deadlock.
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

impl<T> Drop for LinkedList<T> {
    fn drop(&mut self) {
        // avoid stack overflow
        while self.pop_front().is_some() {}
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

    /// Appends an element to the back of the list
    pub fn push_back(&self, elt: T) -> Entry<'_, T> {
        let node = Arc::new(Node::new(elt));

        // Fast path: if the list is not empty, appending only needs the tail
        // lock. The pushed node is linked after `old_tail` and becomes the new
        // tail.
        let mut tail_is_head = false;
        let new_next = node.clone();
        let new_tail = node.clone();
        self.tail.update(|tail| {
            // tail is never none
            let old_tail = tail.unwrap();
            if Arc::ptr_eq(&old_tail, &self.head) {
                // The tail still points at the head sentinel: either the list
                // is empty, or a concurrent pop_front has already moved the
                // tail back but has not published the empty head.next yet.
                // This needs both the head.next and the tail locks. Do not
                // take head.next while holding tail here, that would invert
                // the lock order used by push_front/pop_front and could
                // deadlock. Leave the tail unchanged and use the slow path.
                tail_is_head = true;
                Some(old_tail)
            } else {
                old_tail.next.write(new_next);
                Some(new_tail)
            }
        });
        if !tail_is_head {
            return Entry { list: self, node };
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
                if Arc::ptr_eq(&old_tail, &self.head) {
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
                head_next
            }
        });

        Entry { list: self, node }
    }

    /// Insert an element to the front of the list.
    pub fn push_front(&self, elt: T) {
        EntryImpl::new(self, &self.head).insert_after(elt);
    }

    /// Removes the first element of the list and returns it,
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
}
