use alloc::sync::Arc;
use core::fmt;
use core::ops::Deref;
use rcu_cell::RcuCell;

pub struct Node<T> {
    next: RcuCell<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

pub struct EntryRef<T>(Arc<Node<T>>);

impl<T> Deref for EntryRef<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.data.as_ref().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for EntryRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntryRef({:?})", self.0.data.as_ref().unwrap())
    }
}

impl<T: PartialEq> PartialEq for EntryRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.data == other.0.data
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

    pub fn is_empty(&self) -> bool {
        self.tail.arc_eq(&self.head)
    }

    pub fn front(&self) -> Option<EntryRef<T>> {
        self.head.next.read().map(EntryRef)
    }

    pub fn back(&self) -> Option<EntryRef<T>> {
        self.tail.read().map(EntryRef)
    }

    pub fn push_back(&mut self, elt: T) -> EntryRef<T> {
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

        EntryRef(new_node)
    }

    pub fn push_front(&mut self, elt: T) -> EntryRef<T> {
        let new_node = Arc::new(Node {
            // We first set the next to the head, this create a
            // circular LinkedList. After the new node is setup,
            // we will update this next to the old node. This
            // prevent iter to a false tail node which next is None
            next: RcuCell::from(self.head.clone()),
            data: Some(elt),
        });
        // swap the head.next
        match self.head.next.write(new_node.clone()) {
            Some(old_next) => {
                new_node.next.write(old_next);
            }
            None => {
                // update the tail to the new node
                self.tail.write(new_node.clone());
            }
        }

        EntryRef(new_node)
    }

    pub fn pop_front(&mut self) -> Option<EntryRef<T>> {
        let old_node = self.head.next.update(|next| {
            match next {
                Some(next) => match next.next.read() {
                    Some(next_next) => Some(next_next),
                    None => {
                        self.tail.update(|tail| {
                            let tail = tail.unwrap(); // tail is never none
                            if Arc::ptr_eq(&tail, &next) {
                                Some(self.head.clone())
                            } else {
                                Some(tail)
                            }
                        });
                        None
                    }
                },
                None => None,
            }
        });
        old_node.map(EntryRef)
    }

    pub fn iter(&self) -> Iter<T> {
        Iter {
            head: &self.head,
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
    head: &'a Arc<Node<T>>,
    tail: &'a RcuCell<Node<T>>,
    curr: Arc<Node<T>>,
}

impl<T> Iterator for Iter<'_, T> {
    type Item = EntryRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = loop {
            match self.curr.next.read() {
                Some(next) => {
                    if Arc::ptr_eq(&next, self.head) {
                        // skip the head node which is being setup
                        core::hint::spin_loop();
                        continue;
                    }
                    self.curr = next.clone();
                    break next;
                }
                None => {
                    if self.tail.arc_eq(&self.curr) {
                        return None;
                    }
                    // the next node is not setup yet
                    // wait for setup
                    core::hint::spin_loop();
                }
            }
        };

        Some(EntryRef(node))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_list() {
        let mut list = super::LinkedList::new();
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
        let mut list = super::LinkedList::new();
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
