#![feature(test)]
extern crate test;

use rcu_list::d_list::LinkedList;
use test::Bencher;

#[bench]
fn simple_push_front_pop_back(b: &mut Bencher) {
    let list = LinkedList::new();
    let guard = &crossbeam_epoch::pin();
    b.iter(|| {
        let entry = list.push_front(42, guard);
        assert_eq!(list.pop_back(guard), Some(entry));
    });
}

#[bench]
fn simple_push_back_pop_front(b: &mut Bencher) {
    let list = LinkedList::new();
    let guard = &crossbeam_epoch::pin();
    b.iter(|| {
        let entry = list.push_back(42, guard);
        assert_eq!(list.pop_front(guard), Some(entry));
    });
}

#[bench]
fn simple_front(b: &mut Bencher) {
    let list = LinkedList::new();
    let guard = &crossbeam_epoch::pin();
    list.push_front(42, guard);

    b.iter(|| {
        assert_eq!(*list.front(guard).unwrap(), 42);
    });
}

#[bench]
fn simple_back(b: &mut Bencher) {
    let list = LinkedList::new();
    let guard = &crossbeam_epoch::pin();
    list.push_back(42, guard);
    b.iter(|| {
        assert_eq!(*list.back(guard).unwrap(), 42);
    });
}

#[bench]
fn simple_iter(b: &mut Bencher) {
    let list = LinkedList::new();
    let guard = &crossbeam_epoch::pin();
    for i in 0..1000 {
        list.push_back(i, guard);
    }
    let mut iter = list.iter(guard);
    let mut i = 0;
    b.iter(|| {
        assert_eq!(*iter.next().unwrap(), i);
        i += 1;
        if i == 1000 - 1 {
            iter = list.iter(guard);
            i = 0;
        }
    });
}

#[bench]
fn con_mpmc(b: &mut Bencher) {
    use std::sync::{Arc, Barrier};
    const THREADS: usize = 20;
    const ITEMS: usize = 10_000;

    b.iter(|| {
        let queue = Arc::new(LinkedList::new());
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles = (0..THREADS)
            .map(|_| {
                let queue = queue.clone();
                let barrier = barrier.clone();

                std::thread::spawn(move || {
                    barrier.wait();
                    let guard = &crossbeam_epoch::pin();
                    let entry = queue.push_back(0, guard);
                    let mut entry_vec = Vec::with_capacity(ITEMS);
                    entry_vec.push(entry);
                    for i in 1..ITEMS {
                        let node = entry.insert_after(i).unwrap();
                        entry_vec.push(node);
                    }

                    for entry in entry_vec {
                        entry.remove();
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap();
        }
        assert!(queue.is_empty());

        // force to release memory!!!
        drop(queue);
        for _ in 0..128 {
            crossbeam_epoch::pin().flush();
        }
    });
}

#[bench]
fn con_mpmc_crossbeam(b: &mut Bencher) {
    use std::sync::{Arc, Barrier};
    const THREADS: usize = 20;
    const ITEMS: usize = 10_000;

    b.iter(|| {
        let queue = Arc::new(crossbeam_queue::SegQueue::new());
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles = (0..THREADS)
            .map(|_| {
                let queue = queue.clone();
                let barrier = barrier.clone();

                std::thread::spawn(move || {
                    barrier.wait();
                    let mut vec = Vec::with_capacity(ITEMS);
                    for i in 0..ITEMS {
                        queue.push(i);
                        vec.push(i);
                    }

                    for _ in vec {
                        queue.pop().unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap();
        }
        assert!(queue.is_empty());
    });
}
