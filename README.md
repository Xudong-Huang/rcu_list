## Concurrent Linked List

This is a concurrent linked list implementation in Rust using [RcuCell](https://github.com/Xudong-Huang/rcu_cell).

There are two types of linked list: `SignleLinkedList` and `DoubleLinkedList`.

## SingleLinkedList
The `SingleLinkedList` supports the following operations:

- front: Get the head entry of list.
- back: Get the tail entry of list.
- push_front: Insert a new element into the head of list.
- pop_front: Remove the element from the head of list.
- push_back: Insert a new element into the tail of list.
- iter: Iterate over the list.

each `Entry` that returned by instert operations could do the following operations:
- deref: Read the value of the current element.

## DoubleLinkedList
The `DoubleLinkedList` supports the following operations:

- front: Get the head entry of list.
- back: Get the tail entry of list.
- push_front: Insert a new element into the head of list.
- pop_front: Remove the element from the head of list.
- push_back: Insert a new element into the tail of list.
- pop_back: Remove the element from the tail of list.
- iter: Iterate over the list.

each `Entry` that returned by instert operations could do the following operations:
- remove: Remove the current element from the list.
- deref: Read the value of the current element.


### Note
1. `push_front` has better performance than `push_back`, because it doesn't need to lock the previous element.
2. `pop_front` has better performance than `pop_back`, because it doesn't need to lock the previous element.
3. `pop_back` could result more efficient memory recycle than `pop_front`, because it less likely hold the next element.
4. `push_front` and `push_back` only need to lock one element.
5. `pop_front` and `pop_back` need to lock two elements.
6. Don't hold an `Entry` for a long time, the drop may recursively drop next elements and cause a stack overflow.
