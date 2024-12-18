#![doc = include_str!("../README.md")]
#![no_std]

extern crate alloc;

pub mod d_list;
pub mod s_list;
pub mod version_lock;

pub use d_list::LinkedList as DoubleLinkedList;
pub use s_list::LinkedList as SingleLinedList;
pub use version_lock::{TryLockErr, VersionLock};
