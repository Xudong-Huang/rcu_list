use std::sync::{Arc, Barrier};
use std::thread;

use criterion::{criterion_group, criterion_main, Criterion};

const THREADS: usize = 20;
const ITEMS: usize = 1000;

fn treiber_stack(c: &mut Criterion) {
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

trait Queue<T> {
    fn new() -> Self;
    fn push(&self, value: T);
    fn pop(&self) -> Option<T>;
    fn is_empty(&self) -> bool;
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

criterion_group!(benches, treiber_stack);
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
    use super::Queue;
    use rcu_list::d_list::LinkedList;

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

mod rcu_double_list_rev {
    use super::Queue;
    use rcu_list::d_list::LinkedList;

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
