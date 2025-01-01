use rcu_list::d_list::LinkedList;

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
                let guard = &crossbeam_epoch::pin();
                for i in 0..ITEMS {
                    queue.push_back(i, guard);
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    let guard = &crossbeam_epoch::pin();
    for _i in 0..ITEMS * (THREADS) {
        assert!(queue.pop_front(guard).is_some());
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
                let guard = &crossbeam_epoch::pin();
                for i in 0..ITEMS {
                    queue.push_front(i, guard);
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    let guard = &crossbeam_epoch::pin();
    for _i in 0..ITEMS * (THREADS) {
        assert!(queue.pop_back(guard).is_some());
    }

    assert!(queue.is_empty());
}

#[test]
fn con_pop_front() {
    const THREADS: usize = 16;
    const ITEMS: usize = 400;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let guard = &crossbeam_epoch::pin();

    for i in 0..ITEMS * (THREADS) {
        queue.push_back(i, guard);
    }

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                let guard = &crossbeam_epoch::pin();
                for _i in 0..ITEMS {
                    assert!(queue.pop_front(guard).is_some());
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
fn con_pop_back() {
    const THREADS: usize = 20;
    const ITEMS: usize = 1000;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let guard = &crossbeam_epoch::pin();

    for i in 0..ITEMS * (THREADS) {
        queue.push_front(i, guard);
    }

    let handles = (0..THREADS)
        .map(|_| {
            let queue = queue.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                let guard = &crossbeam_epoch::pin();
                for _i in 0..ITEMS {
                    assert!(queue.pop_back(guard).is_some());
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
fn push_back_pop_back() {
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
                let guard = &crossbeam_epoch::pin();
                for i in 0..ITEMS {
                    queue.push_back(i, guard);
                    assert!(queue.pop_back(guard).is_some());
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
                let guard = &crossbeam_epoch::pin();
                for i in 0..ITEMS {
                    queue.push_front(i, guard);
                    assert!(queue.pop_front(guard).is_some());
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
                let guard = &crossbeam_epoch::pin();
                for i in 0..ITEMS {
                    queue.push_back(i, guard);
                    assert!(queue.pop_front(guard).is_some());
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
fn push_front_pop_back() {
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
                let guard = &crossbeam_epoch::pin();
                for i in 0..ITEMS {
                    queue.push_front(i, guard);
                    assert!(queue.pop_back(guard).is_some());
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(queue.is_empty());
}
