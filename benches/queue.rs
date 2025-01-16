use std::sync::{Arc, Barrier};
use std::thread;

use criterion::{criterion_group, criterion_main, Criterion};

const THREADS: usize = 20;
const ITEMS: usize = 1000;

fn concurrent_queue(c: &mut Criterion) {
    c.bench_function("queue-epoch-list-queue", |b| {
        b.iter(run::<epoch_list::ListQueue<usize>>)
    });

    c.bench_function("queue-rcu-single-list", |b| {
        b.iter(run::<rcu_single_list::ListQueue<usize>>)
    });

    c.bench_function("queue-rcu-double-list-head", |b| {
        b.iter(run::<rcu_double_list::ListQueue<usize>>)
    });

    c.bench_function("queue-rcu-double-list-tail", |b| {
        b.iter(run::<rcu_double_list_rev::ListQueue<usize>>)
    });

    c.bench_function("scc_queue", |b| b.iter(run::<scc_queue::SccQueue<usize>>));

    c.bench_function("queue-mutex-list", |b| {
        b.iter(run::<mutex_single_list::MutexQueue<usize>>)
    });

    c.bench_function("crossbeam-queue", |b| {
        b.iter(run::<crossbem_seg_queue::CrossbeamQueue<usize>>)
    });
}

fn single_queue(c: &mut Criterion) {
    c.bench_function("single_queue-epoch-list-queue", |b| {
        b.iter(single_run::<epoch_list::ListQueue<usize>>)
    });

    c.bench_function("single_queue-rcu-single-list", |b| {
        b.iter(single_run::<rcu_single_list::ListQueue<usize>>)
    });

    c.bench_function("single_queue-rcu-double-list-head", |b| {
        b.iter(single_run::<rcu_double_list::ListQueue<usize>>)
    });

    c.bench_function("single_queue-rcu-double-list-tail", |b| {
        b.iter(single_run::<rcu_double_list_rev::ListQueue<usize>>)
    });

    c.bench_function("single_scc_queue", |b| {
        b.iter(single_run::<scc_queue::SccQueue<usize>>)
    });

    c.bench_function("single_queue-mutex-list", |b| {
        b.iter(single_run::<mutex_single_list::MutexQueue<usize>>)
    });

    c.bench_function("single_crossbeam-queue", |b| {
        b.iter(single_run::<crossbem_seg_queue::CrossbeamQueue<usize>>)
    });
}

trait Queue<T> {
    fn new() -> Self;
    fn push(&self, value: T);
    fn pop(&self) -> Option<T>;
    fn is_empty(&self) -> bool;
}

fn single_run<T>()
where
    T: Queue<usize> + Send + Sync + 'static,
{
    let queue = T::new();

    for i in 0..ITEMS * THREADS {
        queue.push(i);
        assert!(queue.pop().is_some());
    }

    assert!(queue.pop().is_none());
    assert!(queue.is_empty());
}

fn run<T>()
where
    T: Queue<usize> + Send + Sync + 'static,
{
    let queue = Arc::new(T::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles = (0..THREADS - 1)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    queue.push(i);
                    assert!(queue.pop().is_some());
                }
            })
        })
        .collect::<Vec<_>>();

    barrier.wait();
    for i in 0..ITEMS {
        queue.push(i);
        assert!(queue.pop().is_some());
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(queue.pop().is_none());
    assert!(queue.is_empty());
}

criterion_group!(benches, concurrent_queue, single_queue);
criterion_main!(benches);

mod rcu_single_list {
    use super::Queue;
    use rcu_list::s_list::LinkedList;

    #[derive(Debug)]
    pub struct ListQueue<T> {
        list: LinkedList<T>,
    }

    impl<T: Copy> Queue<T> for ListQueue<T> {
        fn new() -> ListQueue<T> {
            ListQueue {
                list: LinkedList::new(),
            }
        }

        fn push(&self, value: T) {
            self.list.push_back(value);
        }

        fn pop(&self) -> Option<T> {
            self.list.pop_front().map(|entry| *entry)
        }

        fn is_empty(&self) -> bool {
            self.list.is_empty()
        }
    }
}

mod rcu_double_list {
    use std::fmt::Debug;

    use super::Queue;
    use rcu_list::d_list::LinkedList;

    #[derive(Debug)]
    pub struct ListQueue<T> {
        list: LinkedList<T>,
    }

    impl<T: Copy + Debug> Queue<T> for ListQueue<T> {
        fn new() -> ListQueue<T> {
            ListQueue {
                list: LinkedList::new(),
            }
        }

        fn push(&self, value: T) {
            self.list.push_back(value);
        }

        fn pop(&self) -> Option<T> {
            self.list.pop_front().map(|entry| *entry)
        }

        fn is_empty(&self) -> bool {
            self.list.is_empty()
        }
    }
}

mod rcu_double_list_rev {
    use std::fmt::Debug;

    use super::Queue;
    use rcu_list::d_list::LinkedList;

    #[derive(Debug)]
    pub struct ListQueue<T> {
        list: LinkedList<T>,
    }

    impl<T: Copy + Debug> Queue<T> for ListQueue<T> {
        fn new() -> ListQueue<T> {
            ListQueue {
                list: LinkedList::new(),
            }
        }

        fn push(&self, value: T) {
            self.list.push_front(value);
        }

        fn pop(&self) -> Option<T> {
            self.list.pop_back().map(|entry| *entry)
        }

        fn is_empty(&self) -> bool {
            self.list.is_empty()
        }
    }
}

mod mutex_single_list {
    use std::collections::VecDeque;

    use super::Queue;
    use parking_lot::Mutex;

    #[derive(Debug)]
    pub struct MutexQueue<T> {
        list: Mutex<VecDeque<T>>,
    }

    impl<T: Copy> Queue<T> for MutexQueue<T> {
        fn new() -> MutexQueue<T> {
            MutexQueue {
                list: Default::default(),
            }
        }

        fn push(&self, value: T) {
            self.list.lock().push_back(value);
        }

        fn pop(&self) -> Option<T> {
            self.list.lock().pop_front()
        }

        fn is_empty(&self) -> bool {
            self.list.lock().is_empty()
        }
    }
}

mod scc_queue {
    use super::Queue;

    #[derive(Debug)]
    pub struct SccQueue<T> {
        queue: scc::Queue<T>,
    }

    impl<T: Copy + 'static> Queue<T> for SccQueue<T> {
        fn new() -> SccQueue<T> {
            SccQueue {
                queue: Default::default(),
            }
        }

        fn push(&self, value: T) {
            self.queue.push(value);
        }

        fn pop(&self) -> Option<T> {
            self.queue.pop().map(|v| **v)
        }

        fn is_empty(&self) -> bool {
            self.queue.is_empty()
        }
    }
}

mod crossbem_seg_queue {
    use super::Queue;

    #[derive(Debug)]
    pub struct CrossbeamQueue<T> {
        queue: crossbeam_queue::SegQueue<T>,
    }

    impl<T> Queue<T> for CrossbeamQueue<T> {
        fn new() -> CrossbeamQueue<T> {
            CrossbeamQueue {
                queue: Default::default(),
            }
        }

        fn push(&self, value: T) {
            self.queue.push(value);
        }

        fn pop(&self) -> Option<T> {
            self.queue.pop()
        }

        fn is_empty(&self) -> bool {
            self.queue.is_empty()
        }
    }
}

mod epoch_list {
    //! Michael-Scott lock-free queue.
    //!
    //! Usable with any number of producers and consumers.
    //!
    //! Michael and Scott.  Simple, Fast, and Practical Non-Blocking and Blocking Concurrent Queue
    //! Algorithms.  PODC 1996.  <http://dl.acm.org/citation.cfm?id=248106>
    //!
    //! Simon Doherty, Lindsay Groves, Victor Luchangco, and Mark Moir. 2004b. Formal Verification of a
    //! Practical Lock-Free Queue Algorithm. <https://doi.org/10.1007/978-3-540-30232-2_7>

    use core::mem::MaybeUninit;
    use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};

    use crossbeam_utils::CachePadded;

    use crossbeam_epoch::{unprotected, Atomic, Guard, Owned, Shared};

    // The representation here is a singly-linked list, with a sentinel node at the front. In general
    // the `tail` pointer may lag behind the actual tail. Non-sentinel nodes are either all `Data` or
    // all `Blocked` (requests for data from blocked threads).
    #[derive(Debug)]
    pub(crate) struct ListQueue<T> {
        head: CachePadded<Atomic<Node<T>>>,
        tail: CachePadded<Atomic<Node<T>>>,
    }

    struct Node<T> {
        /// The slot in which a value of type `T` can be stored.
        ///
        /// The type of `data` is `MaybeUninit<T>` because a `Node<T>` doesn't always contain a `T`.
        /// For example, the sentinel node in a queue never contains a value: its slot is always empty.
        /// Other nodes start their life with a push operation and contain a value until it gets popped
        /// out. After that such empty nodes get added to the collector for destruction.
        data: MaybeUninit<T>,

        next: Atomic<Node<T>>,
    }

    // Any particular `T` should never be accessed concurrently, so no need for `Sync`.
    unsafe impl<T: Send> Sync for ListQueue<T> {}
    unsafe impl<T: Send> Send for ListQueue<T> {}

    impl<T> ListQueue<T> {
        /// Create a new, empty queue.
        pub(crate) fn new() -> Self {
            let q = Self {
                head: CachePadded::new(Atomic::null()),
                tail: CachePadded::new(Atomic::null()),
            };
            let sentinel = Owned::new(Node {
                data: MaybeUninit::uninit(),
                next: Atomic::null(),
            });
            unsafe {
                let guard = unprotected();
                let sentinel = sentinel.into_shared(guard);
                q.head.store(sentinel, Relaxed);
                q.tail.store(sentinel, Relaxed);
                q
            }
        }

        /// Attempts to atomically place `n` into the `next` pointer of `onto`, and returns `true` on
        /// success. The queue's `tail` pointer may be updated.
        #[inline(always)]
        fn push_internal(
            &self,
            onto: Shared<'_, Node<T>>,
            new: Shared<'_, Node<T>>,
            guard: &Guard,
        ) -> bool {
            // is `onto` the actual tail?
            let o = unsafe { onto.deref() };
            let next = o.next.load(Acquire, guard);
            if unsafe { next.as_ref().is_some() } {
                // if not, try to "help" by moving the tail pointer forward
                let _ = self
                    .tail
                    .compare_exchange(onto, next, Release, Relaxed, guard);
                false
            } else {
                // looks like the actual tail; attempt to link in `n`
                let result = o
                    .next
                    .compare_exchange(Shared::null(), new, Release, Relaxed, guard)
                    .is_ok();
                if result {
                    // try to move the tail pointer forward
                    let _ = self
                        .tail
                        .compare_exchange(onto, new, Release, Relaxed, guard);
                }
                result
            }
        }

        /// Adds `t` to the back of the queue, possibly waking up threads blocked on `pop`.
        pub(crate) fn push(&self, t: T, guard: &Guard) {
            let new = Owned::new(Node {
                data: MaybeUninit::new(t),
                next: Atomic::null(),
            });
            let new = Owned::into_shared(new, guard);

            loop {
                // We push onto the tail, so we'll start optimistically by looking there first.
                let tail = self.tail.load(Acquire, guard);

                // Attempt to push onto the `tail` snapshot; fails if `tail.next` has changed.
                if self.push_internal(tail, new, guard) {
                    break;
                }
            }
        }

        /// Attempts to pop a data node. `Ok(None)` if queue is empty; `Err(())` if lost race to pop.
        #[inline(always)]
        fn pop_internal(&self, guard: &Guard) -> Result<Option<T>, ()> {
            let head = self.head.load(Acquire, guard);
            let h = unsafe { head.deref() };
            let next = h.next.load(Acquire, guard);
            match unsafe { next.as_ref() } {
                Some(n) => unsafe {
                    self.head
                        .compare_exchange(head, next, Release, Relaxed, guard)
                        .map(|_| {
                            let tail = self.tail.load(Relaxed, guard);
                            // Advance the tail so that we don't retire a pointer to a reachable node.
                            if head == tail {
                                let _ = self
                                    .tail
                                    .compare_exchange(tail, next, Release, Relaxed, guard);
                            }
                            guard.defer_destroy(head);
                            Some(n.data.assume_init_read())
                        })
                        .map_err(|_| ())
                },
                None => Ok(None),
            }
        }

        // /// Attempts to pop a data node, if the data satisfies the given condition. `Ok(None)` if queue
        // /// is empty or the data does not satisfy the condition; `Err(())` if lost race to pop.
        // #[inline(always)]
        // fn pop_if_internal<F>(&self, condition: F, guard: &Guard) -> Result<Option<T>, ()>
        // where
        //     T: Sync,
        //     F: Fn(&T) -> bool,
        // {
        //     let head = self.head.load(Acquire, guard);
        //     let h = unsafe { head.deref() };
        //     let next = h.next.load(Acquire, guard);
        //     match unsafe { next.as_ref() } {
        //         Some(n) if condition(unsafe { &*n.data.as_ptr() }) => unsafe {
        //             self.head
        //                 .compare_exchange(head, next, Release, Relaxed, guard)
        //                 .map(|_| {
        //                     let tail = self.tail.load(Relaxed, guard);
        //                     // Advance the tail so that we don't retire a pointer to a reachable node.
        //                     if head == tail {
        //                         let _ = self
        //                             .tail
        //                             .compare_exchange(tail, next, Release, Relaxed, guard);
        //                     }
        //                     guard.defer_destroy(head);
        //                     Some(n.data.assume_init_read())
        //                 })
        //                 .map_err(|_| ())
        //         },
        //         None | Some(_) => Ok(None),
        //     }
        // }

        /// Attempts to dequeue from the front.
        ///
        /// Returns `None` if the queue is observed to be empty.
        pub(crate) fn try_pop(&self, guard: &Guard) -> Option<T> {
            loop {
                if let Ok(head) = self.pop_internal(guard) {
                    return head;
                }
            }
        }

        // /// Attempts to dequeue from the front, if the item satisfies the given condition.
        // ///
        // /// Returns `None` if the queue is observed to be empty, or the head does not satisfy the given
        // /// condition.
        // pub(crate) fn try_pop_if<F>(&self, condition: F, guard: &Guard) -> Option<T>
        // where
        //     T: Sync,
        //     F: Fn(&T) -> bool,
        // {
        //     loop {
        //         if let Ok(head) = self.pop_if_internal(&condition, guard) {
        //             return head;
        //         }
        //     }
        // }

        pub(crate) fn is_empty(&self) -> bool {
            let guard = &unsafe { unprotected() };
            self.head.load(Relaxed, guard) == self.tail.load(Relaxed, guard)
        }
    }

    impl<T> Drop for ListQueue<T> {
        fn drop(&mut self) {
            unsafe {
                let guard = unprotected();

                while self.try_pop(guard).is_some() {}

                // Destroy the remaining sentinel node.
                let sentinel = self.head.load(Relaxed, guard);
                drop(sentinel.into_owned());
            }
        }
    }

    impl<T> super::Queue<T> for ListQueue<T> {
        fn new() -> ListQueue<T> {
            ListQueue::new()
        }

        fn push(&self, value: T) {
            let guard = &crossbeam_epoch::pin();
            self.push(value, guard);
        }

        fn pop(&self) -> Option<T> {
            let guard = &crossbeam_epoch::pin();
            self.try_pop(guard)
        }

        fn is_empty(&self) -> bool {
            self.is_empty()
        }
    }
}
