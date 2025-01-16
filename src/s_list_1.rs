//! Michael-Scott lock-free queue.
//!
//! Usable with any number of producers and consumers.
//!
//! Michael and Scott.  Simple, Fast, and Practical Non-Blocking and Blocking Concurrent Queue
//! Algorithms.  PODC 1996.  <http://dl.acm.org/citation.cfm?id=248106>
//!
//! Simon Doherty, Lindsay Groves, Victor Luchangco, and Mark Moir. 2004b. Formal Verification of a
//! Practical Lock-Free Queue Algorithm. <https://doi.org/10.1007/978-3-540-30232-2_7>

use alloc::sync::Arc;
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::Ordering::{Relaxed, Release};

use crossbeam_utils::CachePadded;
use rcu_cell::RcuCell;

// The representation here is a singly-linked list, with a sentinel node at the front. In general
// the `tail` pointer may lag behind the actual tail. Non-sentinel nodes are either all `Data` or
// all `Blocked` (requests for data from blocked threads).
#[derive(Debug)]
pub struct Queue<T> {
    head: CachePadded<RcuCell<Node<T>>>,
    tail: CachePadded<RcuCell<Node<T>>>,
}

#[derive(Debug)]
struct Node<T> {
    /// The slot in which a value of type `T` can be stored.
    ///
    /// The type of `data` is `MaybeUninit<T>` because a `Node<T>` doesn't always contain a `T`.
    /// For example, the sentinel node in a queue never contains a value: its slot is always empty.
    /// Other nodes start their life with a push operation and contain a value until it gets popped
    /// out. After that such empty nodes get added to the collector for destruction.
    data: MaybeUninit<T>,

    next: RcuCell<Node<T>>,
}

// Any particular `T` should never be accessed concurrently, so no need for `Sync`.
unsafe impl<T: Send> Sync for Queue<T> {}
unsafe impl<T: Send> Send for Queue<T> {}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Queue<T> {
    /// Create a new, empty queue.
    pub fn new() -> Self {
        let q = Self {
            head: CachePadded::new(RcuCell::none()),
            tail: CachePadded::new(RcuCell::none()),
        };
        let sentinel = Arc::new(Node {
            data: MaybeUninit::uninit(),
            next: RcuCell::none(),
        });

        q.head.write(sentinel.clone());
        q.tail.write(sentinel);
        q
    }

    // /// Attempts to atomically place `n` into the `next` pointer of `onto`, and returns `true` on
    // /// success. The queue's `tail` pointer may be updated.
    // #[inline(always)]
    // fn push_internal(&self, onto: &Arc<Node<T>>, new: &Arc<Node<T>>) -> bool {
    //     // is `onto` the actual tail?
    //     match onto.next.read() {
    //         Some(next) => {
    //             // if not, try to "help" by moving the tail pointer forward
    //             let _ = unsafe {
    //                 self.tail
    //                     .compare_exchange(Arc::as_ptr(onto), Some(&next), Release, Relaxed)
    //             };
    //             false
    //         }
    //         None => {
    //             // looks like the actual tail; attempt to link in `new`
    //             let result = unsafe {
    //                 onto.next
    //                     .compare_exchange(ptr::null(), Some(new), Release, Relaxed)
    //             }
    //             .is_ok();
    //             if result {
    //                 // try to move the tail pointer forward
    //                 let _ = unsafe {
    //                     self.tail
    //                         .compare_exchange(Arc::as_ptr(onto), Some(new), Release, Relaxed)
    //                 };
    //             }
    //             result
    //         }
    //     }
    // }

    /// Adds `t` to the back of the queue
    pub fn push(&self, t: T) {
        let new = Arc::new(Node {
            data: MaybeUninit::new(t),
            next: RcuCell::none(),
        });

        loop {
            // We push onto the tail, so we'll start optimistically by looking there first.
            let tail = self.tail.read().unwrap();

            // Attempt to push onto the `tail` snapshot; fails if `tail.next` has changed.
            match tail.next.read() {
                Some(next) => {
                    // if not, try to "help" by moving the tail pointer forward
                    let _ = unsafe {
                        self.tail
                            .compare_exchange(tail.as_ref(), Some(&next), Release, Relaxed)
                    };
                }
                None => {
                    // looks like the actual tail; attempt to link in `new`
                    let result = unsafe {
                        tail.next
                            .compare_exchange(ptr::null(), Some(&new), Release, Relaxed)
                    }
                    .is_ok();
                    if result {
                        // try to move the tail pointer forward
                        let _ = unsafe {
                            self.tail
                                .compare_exchange(tail.as_ref(), Some(&new), Release, Relaxed)
                        };
                        break;
                    }
                }
            }
        }
    }

    /// Attempts to pop a data node. `Ok(None)` if queue is empty; `Err(())` if lost race to pop.
    #[inline(always)]
    fn pop_internal(&self) -> Result<Option<T>, ()> {
        let head = self.head.read().unwrap();
        match head.next.read() {
            Some(next) => unsafe {
                match self
                    .head
                    .compare_exchange(Arc::as_ptr(&head), Some(&next), Release, Relaxed)
                {
                    Ok(_) => {
                        let tail = self.tail.read().unwrap();
                        // Advance the tail so that we don't retire a pointer to a reachable node.
                        if Arc::ptr_eq(&head, &tail) {
                            let _ = self.tail.compare_exchange(
                                Arc::as_ptr(&tail),
                                Some(&next),
                                Release,
                                Relaxed,
                            );
                        }
                        Ok(Some(next.data.assume_init_read()))
                    }
                    Err(_) => Err(()),
                }
            },
            None => Ok(None),
        }
    }

    /// Attempts to pop a data node, if the data satisfies the given condition. `Ok(None)` if queue
    /// is empty or the data does not satisfy the condition; `Err(())` if lost race to pop.
    #[inline(always)]
    fn pop_if_internal<F>(&self, condition: F) -> Result<Option<T>, ()>
    where
        T: Sync,
        F: Fn(&T) -> bool,
    {
        let h = self.head.read().unwrap();
        let next = h.next.read();
        match next {
            Some(n) if condition(unsafe { &*n.data.as_ptr() }) => unsafe {
                self.head
                    .compare_exchange(Arc::as_ptr(&h), Some(&n), Release, Relaxed)
                    .map(|_| {
                        let tail = self.tail.read().unwrap();
                        // Advance the tail so that we don't retire a pointer to a reachable node.
                        if Arc::ptr_eq(&h, &tail) {
                            let _ = self.tail.compare_exchange(
                                Arc::as_ptr(&tail),
                                Some(&n),
                                Release,
                                Relaxed,
                            );
                        }
                        Some(n.data.assume_init_read())
                    })
                    .map_err(|_| ())
            },
            None | Some(_) => Ok(None),
        }
    }

    /// Attempts to dequeue from the front.
    ///
    /// Returns `None` if the queue is observed to be empty.
    pub fn pop(&self) -> Option<T> {
        loop {
            if let Ok(head) = self.pop_internal() {
                return head;
            }
            core::hint::spin_loop();
        }
    }

    /// Attempts to dequeue from the front, if the item satisfies the given condition.
    ///
    /// Returns `None` if the queue is observed to be empty, or the head does not satisfy the given
    /// condition.
    pub fn try_pop_if<F>(&self, condition: F) -> Option<T>
    where
        T: Sync,
        F: Fn(&T) -> bool,
    {
        loop {
            if let Ok(head) = self.pop_if_internal(&condition) {
                return head;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        RcuCell::ptr_eq(&self.head, &self.tail)
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod test {
    use crossbeam_utils::thread;

    struct Queue<T> {
        queue: super::Queue<T>,
    }

    impl<T> Queue<T> {
        pub(crate) fn new() -> Self {
            Self {
                queue: super::Queue::new(),
            }
        }

        pub(crate) fn push(&self, t: T) {
            self.queue.push(t);
        }

        pub(crate) fn is_empty(&self) -> bool {
            self.queue.is_empty()
        }

        pub(crate) fn try_pop(&self) -> Option<T> {
            self.queue.pop()
        }

        pub(crate) fn pop(&self) -> T {
            loop {
                if let Some(t) = self.try_pop() {
                    return t;
                }
            }
        }
    }

    const CONC_COUNT: i64 = 1000000;

    #[test]
    fn push_try_pop_1() {
        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());
        q.push(37);
        assert!(!q.is_empty());
        assert_eq!(q.try_pop(), Some(37));
        assert!(q.is_empty());
    }

    #[test]
    fn push_try_pop_2() {
        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());
        q.push(37);
        q.push(48);
        assert_eq!(q.try_pop(), Some(37));
        assert!(!q.is_empty());
        assert_eq!(q.try_pop(), Some(48));
        assert!(q.is_empty());
    }

    #[test]
    fn push_try_pop_many_seq() {
        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());
        for i in 0..200 {
            q.push(i)
        }
        assert!(!q.is_empty());
        for i in 0..200 {
            assert_eq!(q.try_pop(), Some(i));
        }
        assert!(q.is_empty());
    }

    #[test]
    fn push_pop_1() {
        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());
        q.push(37);
        assert!(!q.is_empty());
        assert_eq!(q.pop(), 37);
        assert!(q.is_empty());
    }

    #[test]
    fn push_pop_2() {
        let q: Queue<i64> = Queue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.pop(), 37);
        assert_eq!(q.pop(), 48);
    }

    #[test]
    fn push_pop_many_seq() {
        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());
        for i in 0..200 {
            q.push(i)
        }
        assert!(!q.is_empty());
        for i in 0..200 {
            let x = q.pop();
            // println!("x: {}", x);
            assert_eq!(x, i);
        }
        assert!(q.is_empty());
        // drop(q);
        // println!("done!");
    }

    #[test]
    fn push_try_pop_many_spsc() {
        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());

        thread::scope(|scope| {
            scope.spawn(|_| {
                let mut next = 0;

                while next < CONC_COUNT {
                    if let Some(elem) = q.try_pop() {
                        assert_eq!(elem, next);
                        next += 1;
                    }
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        })
        .unwrap();
    }

    #[test]
    fn push_try_pop_many_spmc() {
        fn recv(_t: i32, q: &Queue<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.try_pop() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 {
                        break;
                    }
                }
            }
        }

        let q: Queue<i64> = Queue::new();
        assert!(q.is_empty());
        thread::scope(|scope| {
            for i in 0..3 {
                let q = &q;
                scope.spawn(move |_| recv(i, q));
            }

            scope.spawn(|_| {
                for i in 0..CONC_COUNT {
                    q.push(i);
                }
            });
        })
        .unwrap();
    }

    #[test]
    fn push_try_pop_many_mpmc() {
        use alloc::vec::Vec;

        enum LR {
            Left(i64),
            Right(i64),
        }

        let q: Queue<LR> = Queue::new();
        assert!(q.is_empty());

        thread::scope(|scope| {
            for _t in 0..2 {
                scope.spawn(|_| {
                    for i in CONC_COUNT - 1..CONC_COUNT {
                        q.push(LR::Left(i))
                    }
                });
                scope.spawn(|_| {
                    for i in CONC_COUNT - 1..CONC_COUNT {
                        q.push(LR::Right(i))
                    }
                });
                scope.spawn(|_| {
                    let mut vl = Vec::new();
                    let mut vr = Vec::new();
                    for _i in 0..CONC_COUNT {
                        match q.try_pop() {
                            Some(LR::Left(x)) => vl.push(x),
                            Some(LR::Right(x)) => vr.push(x),
                            _ => {}
                        }
                    }

                    let mut vl2 = vl.clone();
                    let mut vr2 = vr.clone();
                    vl2.sort_unstable();
                    vr2.sort_unstable();

                    assert_eq!(vl, vl2);
                    assert_eq!(vr, vr2);
                });
            }
        })
        .unwrap();
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: Queue<i64> = Queue::new();

        thread::scope(|scope| {
            scope.spawn(|_| {
                let mut next = 0;
                while next < CONC_COUNT {
                    assert_eq!(q.pop(), next);
                    next += 1;
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        })
        .unwrap();
        assert!(q.is_empty());
    }

    #[test]
    fn is_empty_dont_pop() {
        let q: Queue<i64> = Queue::new();
        q.push(20);
        q.push(20);
        assert!(!q.is_empty());
        assert!(!q.is_empty());
        assert!(q.try_pop().is_some());
    }
}
