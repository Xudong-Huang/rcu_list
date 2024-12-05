use alloc::sync::Arc;
use core::fmt;
use core::ops::Deref;
use rcu_cell::RcuCell;

pub struct Node<T> {
    next: RcuCell<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

#[derive(Clone)]
pub struct Entry<'a, T> {
    list: &'a LinkedList<T>,
    node: Arc<Node<T>>,
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

impl<T> Entry<'_, T> {
    fn new_node(&self, node: Arc<Node<T>>) -> Self {
        Self {
            list: self.list,
            node,
        }
    }

    /// Inserts an element after the current element and returns an Entry that can be used to manipulate the element.
    pub fn insert_after(&self, elt: T) -> Entry<T> {
        self.new_node(EntryImpl::new(self.list, &self.node).insert_after(elt))
    }

    /// Inserts an element after the current element and returns an Entry that can be used to manipulate the element.
    pub fn remove_after(&self) -> Option<Entry<T>> {
        EntryImpl::new(self.list, &self.node)
            .remove_after()
            .map(|n| self.new_node(n))
    }

    /// Returns an iterator over the elements of the list from the current element(not include).
    pub fn iter(&self) -> Iter<T> {
        Iter {
            list: self.list,
            curr: self.node.clone(),
        }
    }
}

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
            // We first set the next to the head, this create a
            // circular LinkedList. After the new node is setup,
            // we will update this next to the old node. This
            // prevent iter to a false tail node which next is None
            // and the head.next swap would not be unexpected none
            next: RcuCell::from(self.list.head.clone()),
            data: Some(elt),
        });

        // swap the next
        match self.node.next.write(new_node.clone()) {
            Some(old_node) => {
                // setup the next node
                new_node.next.write(old_node);
            }
            None => {
                // update the tail to the new node
                self.list.tail.write(new_node.clone());
                self.list.tail.update(|tail| {
                    let tail = tail.unwrap(); // tail is never none
                    if Arc::ptr_eq(&tail, &self.node) {
                        Some(new_node.clone())
                    } else {
                        Some(tail)
                    }
                });
            }
        }

        new_node
    }

    fn remove_after(&self) -> Option<Arc<Node<T>>> {
        let next = &self.node.next;
        next.update(|next| {
            next.and_then(|next| {
                match next.next.read() {
                    Some(next_next) => Some(next_next),
                    None => {
                        self.list.tail.update(|tail| {
                            let tail = tail.unwrap(); // tail is never none
                            if Arc::ptr_eq(&tail, &next) {
                                Some(self.node.clone())
                            } else {
                                Some(tail)
                            }
                        });
                        None
                    }
                }
            })
        })
    }
}

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
    pub fn new() -> Self {
        // this is only used for list head, should never deref it's data
        let head = Arc::new(Node {
            next: RcuCell::none(),
            data: None,
        });

        let tail = RcuCell::from(head.clone());

        Self { head, tail }
    }

    fn new_entry(&self, node: Arc<Node<T>>) -> Entry<T> {
        Entry { list: self, node }
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.tail.arc_eq(&self.head)
    }

    /// Returns the first element of the list, or None if the list is empty.
    pub fn front(&self) -> Option<Entry<T>> {
        self.head.next.read().map(|n| self.new_entry(n))
    }

    /// Returns the last element of the list, or None if the list is empty.
    pub fn back(&self) -> Option<Entry<T>> {
        self.tail.read().map(|n| self.new_entry(n))
    }

    /// Appends an element to the back of the list
    /// and returns an Entry that can be used to manipulate the element.
    pub fn push_back(&self, elt: T) -> Entry<T> {
        let new_node = Arc::new(Node {
            next: RcuCell::none(),
            data: Some(elt),
        });

        // swap the tail
        match self.tail.write(new_node.clone()) {
            Some(tail_node) => {
                // the iter may stop here before the next is setup
                let old = tail_node.next.write(new_node.clone());
                assert!(old.is_none());
            }
            None => {
                // the tail always points to a valid node
                unreachable!("tail node should always be valid");
            }
        }

        self.new_entry(new_node)
    }

    /// Insert an element to the front of the list.
    /// and returns an Entry that can be used to manipulate the element.
    pub fn push_front(&self, elt: T) -> Entry<T> {
        self.new_entry(EntryImpl::new(self, &self.head).insert_after(elt))
    }

    /// Removes the first element of the list and returns it,
    pub fn pop_front(&self) -> Option<Entry<T>> {
        EntryImpl::new(self, &self.head)
            .remove_after()
            .map(|n| self.new_entry(n))
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
        let node = loop {
            match self.curr.next.read() {
                Some(next) => {
                    if Arc::ptr_eq(&next, &self.list.head) {
                        // skip the head node which is being setup
                        core::hint::spin_loop();
                        continue;
                    }
                    self.curr = next.clone();
                    break next;
                }
                None => {
                    if self.list.tail.arc_eq(&self.curr) {
                        return None;
                    }
                    // the next node is not setup yet
                    // wait for setup
                    core::hint::spin_loop();
                }
            }
        };

        Some(Entry {
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

        let a = list.push_back(1);
        assert!(!list.is_empty());
        assert_eq!(*list.front().unwrap(), 1);
        assert_eq!(*list.back().unwrap(), 1);
        assert_eq!(*a, 1);

        let b = list.push_back(2);
        assert_eq!(*list.front().unwrap(), 1);
        assert_eq!(*list.back().unwrap(), 2);
        assert_eq!(*b, 2);

        let c = list.push_front(0);
        assert_eq!(*list.front().unwrap(), 0);
        assert_eq!(*list.back().unwrap(), 2);
        assert_eq!(*c, 0);

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
