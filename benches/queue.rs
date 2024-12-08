use std::sync::{Arc, Barrier};
use std::thread;

use criterion::{criterion_group, criterion_main, Criterion};

const THREADS: usize = 20;
const ITEMS: usize = 1000;

fn treiber_stack(c: &mut Criterion) {
    c.bench_function("queue-rcu-list", |b| {
        b.iter(run::<rcu_single_list::ListQueue<usize>>)
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
