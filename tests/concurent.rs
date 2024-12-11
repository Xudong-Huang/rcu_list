use rcu_list::d_list::LinkedList;

use std::sync::{Arc, Barrier};
use std::thread;

const THREADS: usize = 20;
const ITEMS: usize = 1000;

#[test]
fn push_back() {
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
fn pop_front() {
    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));

	for i in 0..ITEMS * (THREADS) {
        queue.push_front(i);
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
fn push_back_pop_front() {
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
