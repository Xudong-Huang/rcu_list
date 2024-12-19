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
