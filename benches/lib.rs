#![feature(test)]
extern crate test;

use rcu_list::d_list::LinkedList;
use test::Bencher;

#[bench]
fn simple_push_front_pop_back(b: &mut Bencher) {
    let list = LinkedList::new();
    b.iter(|| {
        let entry = list.push_front(42);
        assert_eq!(list.pop_back(), Some(entry));
    });
}

#[bench]
fn simple_push_back_pop_front(b: &mut Bencher) {
    let list = LinkedList::new();
    b.iter(|| {
        let entry = list.push_back(42);
        assert_eq!(list.pop_front(), Some(entry));
    });
}

#[bench]
fn simple_front(b: &mut Bencher) {
    let list = LinkedList::new();
    list.push_front(42);
    b.iter(|| {
        assert_eq!(*list.front().unwrap(), 42);
    });
}

#[bench]
fn simple_back(b: &mut Bencher) {
    let list = LinkedList::new();
    list.push_back(42);
    b.iter(|| {
        assert_eq!(*list.back().unwrap(), 42);
    });
}

#[bench]
fn simple_iter(b: &mut Bencher) {
    let list = LinkedList::new();
    for i in 0..1000 {
        list.push_back(i);
    }
    let mut iter = list.iter();
    let mut i = 0;
    b.iter(|| {
        assert_eq!(*iter.next().unwrap(), i);
        i += 1;
        if i == 1000 - 1 {
            iter = list.iter();
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
                    let entry = queue.push_back(0);
                    let mut entry_vec = Vec::with_capacity(ITEMS);
                    entry_vec.push(entry.clone());
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
                        let item = Arc::new(i);
                        queue.push(item.clone());
                        vec.push(item);
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
