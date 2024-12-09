use alloc::sync::{Arc, Weak};
use rcu_cell::RcuCell;

use core::fmt;
use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct Node<T> {
    version: AtomicUsize,
    next: RcuCell<Node<T>>,
    // this is actually Weak<Node<T>>, but we can't use Weak in atomic
    // we use Weak to avoid reference cycles
    prev: Weak<Node<T>>,
    // only the head node has None data
    data: Option<T>,
}

impl<T> Node<T> {
    fn try_lock(&self) -> Result<usize, ()> {
        let version = self.version.load(Ordering::Relaxed);
        if version & 1 == 1 {
            return Err(());
        }
        match self.version.compare_exchange_weak(
            version,
            version + 2,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(version),
            Err(_) => Ok(usize::MAX),
        }
    }

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

    fn unlock(&self) {
        self.version.fetch_add(2, Ordering::Relaxed);
    }

    fn unlock_remove(&self) {
        self.version.fetch_add(3, Ordering::Relaxed);
    }

    fn is_removed(&self) -> bool {
        self.version.load(Ordering::Relaxed) & 1 == 1
    }

    // fn set_prev(&self, new_prev: &Arc<Node<T>>) {
    //     let weak_prev = Arc::downgrade(new_prev);
    //     let prev = Weak::into_raw(weak_prev);
    //     let old = self.prev.swap(prev as *mut Node<T>, Ordering::Relaxed);
    //     unsafe { Weak::from_raw(old) };
    // }
}

// pub struct Entry<T> {
//     node: Arc<RcuCell<Node<T>>>,
// }

// impl<T> Entry<T> {
//     fn as_impl(&self) -> EntryImpl<T> {
//         EntryImpl(&self.node)
//     }

//     /// insert a node after the current node
//     pub fn insert_after(&self, data: T) -> Result<Entry<T>, T> {
//         Ok(Entry {
//             node: self.as_impl().insert_after(data)?,
//         })
//     }

//     /// insert a node before the current node
//     pub fn insert_ahead(&self, data: T) -> Result<Entry<T>, T> {
//         Ok(Entry {
//             node: self.as_impl().insert_ahead(data)?,
//         })
//     }

//     /// remove the node from the list
//     pub fn remove(self) {
//         let (prev, prev_node) = match self.as_impl().lock_prev_node() {
//             Ok((prev, prev_node)) => (prev, prev_node),
//             Err(_) => return,
//         };
//         let curr_node = self.node.read().unwrap();
//         let next_node = curr_node.next.read().unwrap();
//         if curr_node.lock().is_err() {
//             // current node is removed
//             prev_node.unlock();
//             return;
//         }
//         assert!(Arc::ptr_eq(&prev, &curr_node.prev.upgrade().unwrap()));
//         // update prev.next
//         // prev.write(Node {
//         //     version: prev_node.version.clone(),
//         //     prev: prev_node.prev.clone(),
//         //     next: curr_node.next.clone(),
//         //     data: prev_node.data.clone(),
//         // });
//         prev_node.next.write(next_node.clone());
//         // update next.prev
//         // curr_node.next.write(Node {
//         //     version: next_node.version.clone(),
//         //     prev: curr_node.prev.clone(),
//         //     next: next_node.next.clone(),
//         //     data: next_node.data.clone(),
//         // });
//         next_node.prev = curr_node.prev.clone();
//         // make current node removed
//         curr_node.unlock_remove();
//         prev_node.unlock();
//     }
// }

// pub struct EntryImpl<'a, T>(&'a Arc<RcuCell<Node<T>>>);

// impl<T> EntryImpl<'_, T> {
//     /// lock the prev node
//     #[allow(clippy::type_complexity)]
//     fn lock_prev_node(&self) -> Result<(Arc<RcuCell<Node<T>>>, Arc<Node<T>>), ()> {
//         loop {
//             let curr = self.0.read().unwrap();
//             let prev = match curr.prev.upgrade() {
//                 // something wrong, like the prev node is deleted, or the current node is deleted
//                 None => {
//                     if curr.is_removed() {
//                         return Err(());
//                     } else {
//                         // try again
//                         continue;
//                     }
//                 }

//                 // the prev can change due to prev insert/remove
//                 Some(prev) => prev,
//             };
//             let prev_node = prev.read().unwrap();
//             if prev_node.lock().is_err() {
//                 // the prev node is removed
//                 continue;
//             }
//             if !Arc::ptr_eq(self.0, &prev_node.next) {
//                 // the prev node is changed
//                 prev_node.unlock();
//                 continue;
//             }
//             // successfully lock the prev node
//             return Ok((prev, prev_node));
//         }
//     }

//     /// insert a node after the current node
//     /// this could failed due to the current node is removed
//     fn insert_after(&self, data: T) -> Result<Arc<RcuCell<Node<T>>>, T> {
//         // first lock the current node
//         let curr_node = self.0.read().unwrap();
//         if curr_node.lock().is_err() {
//             // current node is removed
//             return Err(data);
//         }
//         self.insert_after_locked(curr_node, data)
//     }

//     fn insert_after_locked(
//         &self,
//         curr_node: Arc<Node<T>>,
//         data: T,
//     ) -> Result<Arc<RcuCell<Node<T>>>, T> {
//         let next_node = curr_node.next.read().unwrap();
//         let new_entry = Arc::new(RcuCell::new(Node {
//             version: Arc::new(AtomicUsize::new(0)),
//             prev: Arc::downgrade(self.0),
//             next: curr_node.next.clone(),
//             data: Arc::new(Some(data)),
//         }));

//         curr_node.next.write(Node {
//             version: next_node.version.clone(),
//             prev: Arc::downgrade(&new_entry),
//             next: next_node.next.clone(),
//             data: next_node.data.clone(),
//         });

//         self.0.write(Node {
//             version: curr_node.version.clone(),
//             prev: curr_node.prev.clone(),
//             next: new_entry.clone(),
//             data: curr_node.data.clone(),
//         });
//         curr_node.unlock();
//         Ok(new_entry)
//     }

//     /// insert a node before the current node
//     /// this could failed due to the current node is removed
//     fn insert_ahead(&self, data: T) -> Result<Arc<RcuCell<Node<T>>>, T> {
//         // first lock the current node
//         let (prev, prev_node) = match self.lock_prev_node() {
//             Ok((prev, prev_node)) => (prev, prev_node),
//             Err(_) => return Err(data),
//         };
//         let prev = EntryImpl(&prev);
//         prev.insert_after_locked(prev_node, data)
//     }

//     /// remove the node before the current node
//     /// this could invalidate the prev Entry!!
//     fn remove_ahead(&self, head: &Arc<RcuCell<Node<T>>>) -> Option<Arc<RcuCell<Node<T>>>> {
//         let (prev, prev_node) = match self.lock_prev_node() {
//             Ok((prev, prev_node)) => (prev, prev_node),
//             Err(_) => return None,
//         };
//         if Arc::ptr_eq(&prev, head) {
//             // we can't remove the head node
//             prev_node.unlock();
//             return None;
//         }
//         self.remove_after_locked(prev_node, None)
//     }

//     /// remove the node after the current node
//     /// if the current node is removed, return None
//     /// if the next node is tail, return None
//     fn remove_after(&self, tail: &Arc<RcuCell<Node<T>>>) -> Option<Arc<RcuCell<Node<T>>>> {
//         // self.0 is the prev node
//         let curr_node = self.0.read().unwrap();
//         curr_node.lock().ok()?;
//         self.remove_after_locked(curr_node, Some(tail))
//     }

//     fn remove_after_locked(
//         &self,
//         curr_node: Arc<Node<T>>,
//         tail: Option<&Arc<RcuCell<Node<T>>>>,
//     ) -> Option<Arc<RcuCell<Node<T>>>> {
//         // self.0 is the prev node
//         let prev_node = curr_node;
//         if let Some(tail) = tail {
//             if Arc::ptr_eq(&prev_node.next, tail) {
//                 // we can't remove the tail node
//                 prev_node.unlock();
//                 return None;
//             }
//         }

//         let curr_node = prev_node.next.read().unwrap();
//         curr_node.lock().unwrap();
//         let ret = prev_node.next.clone();
//         // update prev.next
//         self.0.write(Node {
//             version: prev_node.version.clone(),
//             prev: prev_node.prev.clone(),
//             next: curr_node.next.clone(),
//             data: prev_node.data.clone(),
//         });

//         // update next.prev
//         let next_node = curr_node.next.read().unwrap();
//         curr_node.next.write(Node {
//             version: next_node.version.clone(),
//             prev: curr_node.prev.clone(),
//             next: next_node.next.clone(),
//             data: next_node.data.clone(),
//         });
//         curr_node.unlock_remove();
//         prev_node.unlock();
//         Some(ret)
//     }
// }

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

impl<T> EntryRef<T> {
    // /// get the next node
    // pub fn next(&self) -> Option<EntryRef<T>> {
    // 	let next = self.0.next.read().unwrap();
    // 	(!Arc::ptr_eq(&next, &self.0.next)).then(|| EntryRef(next))
    // }

    // /// get the previous node
    // pub fn prev(&self) -> Option<EntryRef<T>> {
    // 	let prev = self.0.prev.upgrade().unwrap();
    // 	(!Arc::ptr_eq(&prev, &self.0.prev)).then(|| EntryRef(prev))
    // }
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
        let head = Arc::new_cyclic(|me| Node {
            version: AtomicUsize::new(0),
            prev: me.clone(),
            next: RcuCell::none(),
            data: None,
        });

        // let prev piont to head, so we can simply clone it when insert new node
        let tail = RcuCell::from(head.clone());

        Self { head, tail }
    }

    pub fn front(&self) -> Option<EntryRef<T>> {
        (!self.tail.arc_eq(&self.head)).then(|| {
            // we are sure the next exist
            let node = self.head.next.read().unwrap();
            EntryRef(node)
        })
    }

    pub fn back(&self) -> Option<EntryRef<T>> {
        (!self.tail.arc_eq(&self.head)).then(|| {
            // the tail is always valid
            let tail_node = self.tail.read().unwrap();
            EntryRef(tail_node)
        })
    }

    fn lock_tail(&self) -> Arc<Node<T>> {
        loop {
            // tail always exists
            let tail = self.tail.read().unwrap();
            if tail.lock().is_ok() {
                // if err, the tail is removed, try again
                return tail;
            }
        }
    }

    fn lock_prev_node(&self, curr_node: &Arc<Node<T>>) -> Result<Arc<Node<T>>, ()> {
        loop {
            let prev_node = match curr_node.prev.upgrade() {
                // something wrong, like the prev node is deleted, or the current node is deleted
                None => {
                    if curr_node.is_removed() {
                        return Err(());
                    } else {
                        continue;
                    }
                }
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

    pub fn push_back(&mut self, elt: T) -> EntryRef<T> {
        let tail_node = self.lock_tail();
        let new_node = Arc::new(Node {
            version: AtomicUsize::new(0),
            prev: tail_node.prev.clone(),
            next: RcuCell::none(),
            data: Some(elt),
        });
        // swap the tail node
        let old_tail_node = self.tail.write(new_node.clone()).unwrap();
        tail_node.next.write(new_node.clone());
        tail_node.unlock();
        // check the tail is not changed
        assert!(Arc::ptr_eq(&old_tail_node, &tail_node));
        EntryRef(new_node)
    }

    pub fn pop_back(&mut self) -> Option<EntryRef<T>> {
        loop {
            let tail_node = self.tail.read().unwrap();
            if Arc::ptr_eq(&tail_node, &self.head) {
                return None;
            }
            let prev_node = match self.lock_prev_node(&tail_node) {
                Ok(prev_node) => prev_node,
                // the tail is removed, try again
                Err(_) => continue,
            };
            // since the prev node is lock, the curr node should not be removed
            tail_node.lock().expect("tail node removed!");

            match tail_node.next.read() {
                None => {
                    // tail node is really the tail
                    prev_node.next.take();
                }
                Some(next_node) => {
                    // the tail is advanced
                    prev_node.next.write(next_node.clone());
                    // next_node.prev = prev_node.prev.clone();
                }
            }
            tail_node.unlock_remove();
            prev_node.unlock();
            return Some(EntryRef(tail_node));
        }
    }

    // pub fn push_front(&mut self, elt: T) -> EntryRef<T> {
    //     let entry = EntryImpl(&self.head);
    //     let new_entry = match entry.insert_after(elt) {
    //         Ok(entry) => entry,
    //         Err(_) => panic!("push_front failed"),
    //     };
    //     new_entry.read().map(EntryRef).unwrap()
    // }

    // pub fn pop_front(&mut self) -> Option<EntryRef<T>> {
    //     let entry = EntryImpl(&self.head);
    //     entry
    //         .remove_after(&self.tail)
    //         .and_then(|node| node.read().map(EntryRef))
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
    tail: &'a RcuCell<Node<T>>,
    curr: Arc<Node<T>>,
}

impl<T> Iterator for Iter<'_, T> {
    type Item = EntryRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let curr = loop {
            match self.curr.next.read() {
                Some(next) => {
                    let curr = self.curr.clone();
                    self.curr = next;
                    break curr;
                }
                None => {
                    if self.tail.arc_eq(&self.curr) {
                        return None;
                    }
                    // the next node is not setup yet
                    core::hint::spin_loop();
                }
            }
        };

        Some(EntryRef(curr))
    }
}

// #[test]
// fn test_basic() {
//     use alloc::boxed::Box;

//     let mut m = LinkedList::<Box<usize>>::new();
//     assert_eq!(m.pop_front(), None);
//     assert_eq!(m.pop_back(), None);
//     assert_eq!(m.pop_front(), None);
//     m.push_front(Box::new(1));
//     assert_eq!(m.pop_front().as_deref(), Some(&Box::new(1)));
//     m.push_back(Box::new(2));
//     m.push_back(Box::new(3));
//     // // assert_eq!(m.len(), 2);
//     assert_eq!(m.pop_front().as_deref(), Some(&Box::new(2)));
//     assert_eq!(m.pop_front().as_deref(), Some(&Box::new(3)));
//     // // assert_eq!(m.len(), 0);
//     assert_eq!(m.pop_front(), None);
//     m.push_back(Box::new(1));
//     m.push_back(Box::new(3));
//     m.push_back(Box::new(5));
//     m.push_back(Box::new(7));
//     assert_eq!(m.pop_front().as_deref(), Some(&Box::new(1)));

//     let mut n = LinkedList::new();
//     n.push_front(2);
//     n.push_front(3);
//     {
//         let x = n.front().unwrap();
//         assert_eq!(&*x, &3);
//         // let x = n.front_mut().unwrap();
//         // assert_eq!(*x, 3);
//         // *x = 0;
//     }
//     // {
//     //     assert_eq!(n.back().unwrap(), &2);
//     //     let y = n.back_mut().unwrap();
//     //     assert_eq!(*y, 2);
//     //     *y = 1;
//     // }
//     // assert_eq!(n.pop_front(), Some(0));
//     // assert_eq!(n.pop_front(), Some(1));
// }
