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

#[test]
fn push_front_and_push_back() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    const THREADS: usize = 8;
    const ITEMS: usize = 2000;

    let queue = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));
    let progress = Arc::new(AtomicUsize::new(0));
    let popped = Arc::new(AtomicUsize::new(0));

    let handles = (0..THREADS)
        .map(|t| {
            let queue = queue.clone();
            let barrier = barrier.clone();
            let progress = progress.clone();
            let popped = popped.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    if t % 2 == 0 {
                        queue.push_front(i);
                    } else {
                        queue.push_back(i);
                    }
                    // Push and pop alternately so the list frequently
                    // transitions through the empty state, where both ends
                    // need to be updated together.
                    if queue.pop_front().is_some() {
                        popped.fetch_add(1, Ordering::Relaxed);
                    }
                    progress.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect::<Vec<_>>();

    // Concurrent pushes used to be able to deadlock; fail instead of hanging
    // the test suite forever when there is no progress for a while.
    let mut last = 0;
    let mut last_change = Instant::now();
    loop {
        let p = progress.load(Ordering::Relaxed);
        if p == THREADS * ITEMS {
            break;
        }
        if p != last {
            last = p;
            last_change = Instant::now();
        } else if last_change.elapsed() > Duration::from_secs(5) {
            panic!("deadlock between push_front and push_back");
        }
        if handles.iter().all(|handle| handle.is_finished()) {
            // a worker panicked, let join report it
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Every pushed element must either have been popped or still be in the
    // list: nothing may be lost between the two ends.
    let mut remaining = 0;
    while queue.pop_front().is_some() {
        remaining += 1;
    }
    assert!(queue.is_empty());
    assert_eq!(
        popped.load(Ordering::Relaxed) + remaining,
        THREADS * ITEMS,
        "elements were lost"
    );
}
