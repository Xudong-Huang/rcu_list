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
        assert!(queue.pop_back().is_some());
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
fn con_pop_back() {
    const THREADS: usize = 20;
    const ITEMS: usize = 1000;

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
                    assert!(queue.pop_back().is_some());
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
                for i in 0..ITEMS {
                    queue.push_back(i);
                    assert!(queue.pop_back().is_some());
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
                for i in 0..ITEMS {
                    queue.push_front(i);
                    assert!(queue.pop_back().is_some());
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
fn mixed_operations() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    const THREADS: usize = 8;
    const ITEMS: usize = 1000;

    let list = Arc::new(LinkedList::new());
    let barrier = Arc::new(Barrier::new(THREADS));
    let progress = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let next_value = Arc::new(AtomicUsize::new(0));

    let handles = (0..THREADS)
        .map(|t| {
            let list = list.clone();
            let barrier = barrier.clone();
            let progress = progress.clone();
            let finished = finished.clone();
            let next_value = next_value.clone();

            thread::spawn(move || {
                barrier.wait();
                let mut seed = 0x9e37_79b9_u64 ^ (t as u64);
                for _ in 0..ITEMS {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    // every value is unique, so a duplicate in the list would
                    // show up in the final check
                    let value = next_value.fetch_add(1, Ordering::Relaxed);
                    match (seed >> 33) % 10 {
                        0 => {
                            let _ = list.push_front(value);
                        }
                        1 => {
                            let _ = list.push_back(value);
                        }
                        2 => {
                            let _ = list.pop_front();
                        }
                        3 => {
                            let _ = list.pop_back();
                        }
                        4 => {
                            if let Some(entry) = list.front() {
                                let _ = entry.insert_after(value);
                            }
                        }
                        5 => {
                            if let Some(entry) = list.back() {
                                let _ = entry.insert_ahead(value);
                            }
                        }
                        6 => {
                            if let Some(entry) = list.front() {
                                let _ = entry.remove_after();
                            }
                        }
                        7 => {
                            if let Some(entry) = list.back() {
                                let _ = entry.remove_ahead();
                            }
                        }
                        8 => {
                            if let Some(entry) = list.front() {
                                let _ = entry.replace(value);
                            }
                        }
                        _ => {
                            if let Some(entry) = list.front() {
                                entry.remove();
                            }
                        }
                    }
                    progress.fetch_add(1, Ordering::Relaxed);
                }
                finished.fetch_add(1, Ordering::Relaxed);
            })
        })
        .collect::<Vec<_>>();

    // fail instead of hanging the test suite forever if the operations stop
    // making progress
    let mut last = 0;
    let mut last_change = Instant::now();
    while finished.load(Ordering::Relaxed) < THREADS {
        let p = progress.load(Ordering::Relaxed);
        if p != last {
            last = p;
            last_change = Instant::now();
        } else if last_change.elapsed() > Duration::from_secs(5) {
            panic!("no progress in mixed list operations, deadlock suspected");
        }
        thread::sleep(Duration::from_millis(10));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // drain the list, nothing may be duplicated
    let mut values = Vec::new();
    while let Some(entry) = list.pop_front() {
        values.push(*entry);
    }
    assert!(list.is_empty());

    let count = values.len();
    values.sort_unstable();
    values.dedup();
    assert_eq!(count, values.len(), "duplicate values in the list");
}
