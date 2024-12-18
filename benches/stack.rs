use std::sync::{Arc, Barrier};
use std::thread;

use criterion::{criterion_group, criterion_main, Criterion};

const THREADS: usize = 20;
const ITEMS: usize = 1000;

fn treiber_stack(c: &mut Criterion) {
    c.bench_function("trieber_stack-rcu-single-list", |b| {
        b.iter(run::<rcu_single_list::TreiberStack<usize>>)
    });

    c.bench_function("trieber_stack-rcu-double-list", |b| {
        b.iter(run::<rcu_double_list::TreiberStack<usize>>)
    });

    c.bench_function("trieber_stack-rcu-stack", |b| {
        b.iter(run::<rcu_stack::TreiberStack<usize>>)
    });

    c.bench_function("trieber_stack-crossbeam", |b| {
        b.iter(run::<crossbeam_stack::TreiberStack<usize>>)
    });

    c.bench_function("trieber_stack-seize", |b| {
        b.iter(run::<seize_stack::TreiberStack<usize>>)
    });

    c.bench_function("scc-stack", |b| {
        b.iter(run::<scc_stack::TreiberStack<usize>>)
    });
}

trait Stack<T> {
    fn new() -> Self;
    fn push(&self, value: T);
    fn pop(&self) -> Option<T>;
    fn is_empty(&self) -> bool;
}

fn run<T>()
where
    T: Stack<usize> + Send + Sync + 'static,
{
    let stack = Arc::new(T::new());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles = (0..THREADS - 1)
        .map(|_| {
            let stack = stack.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();
                for i in 0..ITEMS {
                    stack.push(i);
                    assert!(stack.pop().is_some());
                }
            })
        })
        .collect::<Vec<_>>();

    barrier.wait();
    for i in 0..ITEMS {
        stack.push(i);
        assert!(stack.pop().is_some());
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(stack.pop().is_none());
    assert!(stack.is_empty());
}

criterion_group!(benches, treiber_stack);
criterion_main!(benches);

mod seize_stack {
    use super::Stack;
    use seize::{reclaim, Collector, Guard, Linked};
    use std::mem::ManuallyDrop;
    use std::ptr::{self, NonNull};
    use std::sync::atomic::{AtomicPtr, Ordering};

    #[derive(Debug)]
    pub struct TreiberStack<T> {
        head: AtomicPtr<Linked<Node<T>>>,
        collector: Collector,
    }

    #[derive(Debug)]
    struct Node<T> {
        data: ManuallyDrop<T>,
        next: *mut Linked<Node<T>>,
    }

    impl<T> Stack<T> for TreiberStack<T> {
        fn new() -> TreiberStack<T> {
            TreiberStack {
                head: AtomicPtr::new(ptr::null_mut()),
                collector: Collector::new().epoch_frequency(None),
            }
        }

        fn push(&self, value: T) {
            let node = self.collector.link_boxed(Node {
                data: ManuallyDrop::new(value),
                next: ptr::null_mut(),
            });

            let guard = self.collector.enter();

            loop {
                let head = guard.protect(&self.head, Ordering::Relaxed);
                unsafe { (*node).next = head }

                if self
                    .head
                    .compare_exchange(head, node, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
        }

        fn pop(&self) -> Option<T> {
            let guard = self.collector.enter();

            loop {
                let head = NonNull::new(guard.protect(&self.head, Ordering::Acquire))?.as_ptr();

                let next = unsafe { (*head).next };

                if self
                    .head
                    .compare_exchange(head, next, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    unsafe {
                        let data = ptr::read(&(*head).data);
                        self.collector
                            .retire(head, reclaim::boxed::<Linked<Node<T>>>);
                        return Some(ManuallyDrop::into_inner(data));
                    }
                }
            }
        }

        fn is_empty(&self) -> bool {
            let guard = self.collector.enter();
            guard.protect(&self.head, Ordering::Relaxed).is_null()
        }
    }

    impl<T> Drop for TreiberStack<T> {
        fn drop(&mut self) {
            while self.pop().is_some() {}
        }
    }
}

mod crossbeam_stack {
    use super::Stack;
    use crossbeam_epoch::{Atomic, Owned, Shared};
    use std::mem::ManuallyDrop;
    use std::ptr;
    use std::sync::atomic::Ordering;

    #[derive(Debug)]
    pub struct TreiberStack<T> {
        head: Atomic<Node<T>>,
    }

    unsafe impl<T: Send> Send for TreiberStack<T> {}
    unsafe impl<T: Sync> Sync for TreiberStack<T> {}

    #[derive(Debug)]
    struct Node<T> {
        data: ManuallyDrop<T>,
        next: *const Node<T>,
    }

    impl<T> Stack<T> for TreiberStack<T> {
        fn new() -> TreiberStack<T> {
            TreiberStack {
                head: Atomic::null(),
            }
        }

        fn push(&self, value: T) {
            let guard = crossbeam_epoch::pin();

            let mut node = Owned::new(Node {
                data: ManuallyDrop::new(value),
                next: ptr::null_mut(),
            });

            loop {
                let head = self.head.load(Ordering::Relaxed, &guard);
                node.next = head.as_raw();

                match self.head.compare_exchange(
                    head,
                    node,
                    Ordering::Release,
                    Ordering::Relaxed,
                    &guard,
                ) {
                    Ok(_) => break,
                    Err(err) => node = err.new,
                }
            }
        }

        fn pop(&self) -> Option<T> {
            let guard = crossbeam_epoch::pin();

            loop {
                let head = self.head.load(Ordering::Acquire, &guard);

                if head.is_null() {
                    return None;
                }

                let next = unsafe { head.deref().next };

                if self
                    .head
                    .compare_exchange(
                        head,
                        Shared::from(next),
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                        &guard,
                    )
                    .is_ok()
                {
                    unsafe {
                        let data = ptr::read(&head.deref().data);
                        guard.defer_destroy(head);
                        return Some(ManuallyDrop::into_inner(data));
                    }
                }
            }
        }

        fn is_empty(&self) -> bool {
            let guard = crossbeam_epoch::pin();
            self.head.load(Ordering::Relaxed, &guard).is_null()
        }
    }

    impl<T> Drop for TreiberStack<T> {
        fn drop(&mut self) {
            while self.pop().is_some() {}
        }
    }
}

mod rcu_stack {
    use rcu_cell::RcuCell;
    use std::sync::Arc;

    use super::Stack;
    #[derive(Debug)]
    pub struct TreiberStack<T> {
        head: RcuCell<Node<T>>,
    }

    unsafe impl<T: Send> Send for TreiberStack<T> {}
    unsafe impl<T: Sync> Sync for TreiberStack<T> {}

    #[derive(Debug)]
    struct Node<T> {
        data: T,
        next: RcuCell<Node<T>>,
    }

    impl<T: Copy> Stack<T> for TreiberStack<T> {
        fn new() -> TreiberStack<T> {
            TreiberStack {
                head: RcuCell::none(),
            }
        }

        fn push(&self, value: T) {
            let node = Arc::new(Node {
                data: value,
                next: RcuCell::none(),
            });

            self.head.update(|head| {
                if let Some(h) = head {
                    node.next.write(h);
                }
                Some(node)
            });
        }

        fn pop(&self) -> Option<T> {
            self.head
                .update(|head| head.and_then(|node| node.next.read()))
                .map(|node| node.data)
        }

        fn is_empty(&self) -> bool {
            self.head.is_none()
        }
    }
}

mod rcu_single_list {
    use super::Stack;
    use rcu_list::s_list::LinkedList;

    #[derive(Debug)]
    pub struct TreiberStack<T> {
        list: LinkedList<T>,
    }

    impl<T: Copy> Stack<T> for TreiberStack<T> {
        fn new() -> TreiberStack<T> {
            TreiberStack {
                list: LinkedList::new(),
            }
        }

        fn push(&self, value: T) {
            self.list.push_front(value);
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

    use super::Stack;
    use rcu_list::d_list::LinkedList;

    #[derive(Debug)]
    pub struct TreiberStack<T> {
        list: LinkedList<T>,
    }

    impl<T: Copy + Debug> Stack<T> for TreiberStack<T> {
        fn new() -> TreiberStack<T> {
            TreiberStack {
                list: LinkedList::new(),
            }
        }

        fn push(&self, value: T) {
            self.list.push_front(value);
        }

        fn pop(&self) -> Option<T> {
            self.list.pop_front().map(|entry| *entry)
        }

        fn is_empty(&self) -> bool {
            self.list.is_empty()
        }
    }
}

mod scc_stack {
    use super::Stack;

    #[derive(Debug)]
    pub struct TreiberStack<T> {
        list: scc::Stack<T>,
    }

    impl<T: Copy + 'static> Stack<T> for TreiberStack<T> {
        fn new() -> TreiberStack<T> {
            TreiberStack {
                list: scc::Stack::default(),
            }
        }

        fn push(&self, value: T) {
            self.list.push(value);
        }

        fn pop(&self) -> Option<T> {
            self.list.pop().map(|entry| **entry)
        }

        fn is_empty(&self) -> bool {
            self.list.is_empty()
        }
    }
}
