use rcu_list::s_list::LinkedList;

use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn con_push_back() {
    const THREADS: usize = 20;
    const ITEMS: usize = 1000;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    queue.push_back(i);
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    for _i in 0..ITEMS * (THREADS) {
        assert!(queue.pop_front().is_some());
    }

    assert!(queue.is_empty());
}

#[test]
fn con_push_front() {
    const THREADS: usize = 20;
    const ITEMS: usize = 1000;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    queue.push_front(i);
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    for _i in 0..ITEMS * (THREADS) {
        assert!(queue.pop_front().is_some());
    }

    assert!(queue.is_empty());
}

#[test]
fn con_pop_front() {
    const THREADS: usize = 16;
    const ITEMS: usize = 400;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    for i in 0..ITEMS * (THREADS) {
        queue.push_back(i);
    }

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for _i in 0..ITEMS {
                    assert!(queue.pop_front().is_some());
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(queue.is_empty());
}

#[test]
fn push_front_pop_front() {
    const THREADS: usize = 16;
    const ITEMS: usize = 400;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    queue.push_front(i);
                    assert!(queue.pop_front().is_some());
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(queue.is_empty());
}

#[test]
fn push_back_pop_front() {
    const THREADS: usize = 16;
    const ITEMS: usize = 400;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    queue.push_back(i);
                    assert!(queue.pop_front().is_some());
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(queue.is_empty());
}
