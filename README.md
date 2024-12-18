## Concurrent Linked List

This is a concurrent linked list implementation in Rust using [RcuCell](https://github.com/Xudong-Huang/rcu_cell).

[![Build Status](https://github.com/Xudong-Huang/co_managed/workflows/CI/badge.svg)](https://github.com/Xudong-Huang/rcu_list/actions?query=workflow%3ACI)
[![Current Crates.io Version](https://img.shields.io/crates/v/rcu_list.svg)](https://crates.io/crates/rcu_list)
[![Document](https://img.shields.io/badge/doc-rcu_list-green.svg)](https://docs.rs/rcu_list)

There are two types of linked list: `SignleLinkedList` and `DoubleLinkedList`.

## SingleLinkedList
The `SingleLinkedList` supports the following operations:

- `front`: Get the head entry of list.
- `back`: Get the tail entry of list.
- `push_front`: Insert a new element into the head of list.
- `pop_front`: Remove the element from the head of list.
- `push_back`: Insert a new element into the tail of list.
- `iter`: Iterate over the list.

each `Entry` that returned by instert operations could do the following operations:
- `deref`: Read the value of the current element.

## DoubleLinkedList
The `DoubleLinkedList` supports the following operations:

- `front`: Get the head entry of list.
- `back`: Get the tail entry of list.
- `push_front`: Insert a new element into the head of list.
- `pop_front`: Remove the element from the head of list.
- `push_back`: Insert a new element into the tail of list.
- `pop_back`: Remove the element from the tail of list.
- `iter`: Iterate over the list.

each `Entry` that returned by insert operations could do the following operations:
- `remove`: Remove the current element from the list.
- `insert_after`: Insert an element after the entry.
- `insert_ahead`: Insert an element ahead the entry.
- `remove_after`: Remove the element after the entry.
- `remove_ahead`: Remove the element ahead the entry.
- `deref`: Read the value of the current element.
- `is_removed`: Check if the element is removed from the list.
- `next`: Get the next entry in the list.

Any write operations like insert/remove on a removed `Entry` would just fail.
But it's safe to read(deref) the value of a removed `Entry`.

### Note
1. `push_front` has better performance than `push_back`, because it doesn't need to lock the previous element.
2. `pop_front` has better performance than `pop_back`, because it doesn't need to lock the previous element.
3. `pop_back` could result more efficient memory recycle than `pop_front`, because it less likely hold the next element.
4. `push_front` and `push_back` only need to lock one element.
5. `pop_front` and `pop_back` need to lock two elements.
6. Don't hold an `Entry` for a long time, the drop may recursively drop next elements and cause a stack overflow.
