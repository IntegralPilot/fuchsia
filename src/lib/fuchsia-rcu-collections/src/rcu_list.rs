// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(unsafe_op_in_unsafe_fn)]

use fuchsia_rcu::subtle::{RcuPtr, RcuPtrRef};
use fuchsia_rcu::{RcuDroppable, RcuReadScope, rcu_drop};

use crate::rcu_intrusive_list::{Link, RcuIntrusiveList, RcuIntrusiveListCursor, RcuListAdapter};

/// An `RcuList` is a doubly-linked list that supports concurrent access via
/// read-copy-update (RCU) synchronization.
///
/// An `RcuList` can be safely read by multiple readers, even while a writer
/// is modifying the list. To read from the list, you will need to enter an
/// `RcuReadScope`.
///
/// To modify the list, you will need to use some external synchronization,
/// such as a `Mutex`, to exclude concurrent writers.
#[derive(Debug)]
pub struct RcuList<T: RcuDroppable + Sync, A: RcuListAdapter<T>> {
    list: RcuIntrusiveList<T, A>,
}

// SAFETY: RcuList drops all elements through its intrusive list nodes, which are of type T
// (implementing RcuDroppable).
unsafe impl<T: RcuDroppable + Sync, A: RcuListAdapter<T> + Send + Sync + 'static> RcuDroppable
    for RcuList<T, A>
{
}

impl<T: RcuDroppable + Sync, A: RcuListAdapter<T>> Default for RcuList<T, A> {
    fn default() -> Self {
        Self { list: RcuIntrusiveList::default() }
    }
}

impl<T: RcuDroppable + Sync, A: RcuListAdapter<T>> RcuList<T, A> {
    /// Creates a new list with the given head and tail.
    pub fn new(head: RcuPtr<Link>, tail: RcuPtr<Link>) -> Self {
        Self { list: RcuIntrusiveList::new(head, tail) }
    }

    /// Pushes a new element to the front of the list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn push_front<'a>(&self, scope: &'a RcuReadScope, data: T) -> RcuPtrRef<'a, T> {
        let node = alloc(scope, data);
        // SAFETY: Our caller promises to exclude concurrent writers.
        unsafe {
            self.list.push_front(scope, node);
        }
        node
    }

    /// Pushes a new element to the back of the list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn push_back<'a>(&self, scope: &'a RcuReadScope, data: T) -> RcuPtrRef<'a, T> {
        let node = alloc(scope, data);
        // SAFETY: Our caller promises to exclude concurrent writers.
        unsafe {
            self.list.push_back(scope, node);
        }
        node
    }

    /// Appends another list to the end of this list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn append(&self, scope: &RcuReadScope, other: Self) {
        // SAFETY: Our caller promises to exclude concurrent writers.
        unsafe {
            let items = other.list.split_off(scope, 0);
            self.list.append(scope, items);
        }
    }

    /// Splits the list into two lists at the given position.
    ///
    /// If the given position is past the end of the list, returns an empty list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn split_off(&self, scope: &RcuReadScope, pos: usize) -> Self {
        // SAFETY: Our caller promises to exclude concurrent writers.
        Self { list: unsafe { self.list.split_off(scope, pos) } }
    }

    /// Removes all elements from the list.
    ///
    /// Concurrent readers may continue to see the old value of the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn clear(&self) {
        let scope = RcuReadScope::new();
        // SAFETY: Our caller promises to exclude concurrent writers.
        unsafe { self.list.clear(&scope, deferred_dealloc) };
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        let scope = RcuReadScope::new();
        self.list.is_empty(&scope)
    }

    /// Returns a cursor that can be used to traverse and modify the list.
    ///
    /// Concurrent readers may continue to see the old value of the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn cursor<'a>(&'a self, scope: &'a RcuReadScope) -> RcuListCursor<'a, T, A> {
        RcuListCursor { cursor: self.list.cursor(scope) }
    }

    /// Returns an iterator over the elements in the list.
    pub fn iter<'a>(&self, scope: &'a RcuReadScope) -> impl Iterator<Item = &'a T> {
        self.list.iter(scope)
    }
}

/// Allocates a new node.
///
/// The node must be deallocated using `deferred_dealloc`.
fn alloc<T>(scope: &RcuReadScope, data: T) -> RcuPtrRef<'_, T> {
    let ptr = Box::into_raw(Box::new(data));
    // SAFETY: All nodes must be deallocated using `deferred_dealloc`, which defers their
    // deallocation until all in-flight read operations have completed.
    unsafe { RcuPtrRef::new(scope, ptr) }
}

/// Deallocates a node once all in-flight read operations have completed.
///
/// The node must have been allocated using `alloc`.
fn deferred_dealloc<T>(node: RcuPtrRef<'_, T>)
where
    T: RcuDroppable + Sync,
{
    // SAFETY: The node was allocated using `alloc`.
    let value = unsafe { Box::from_raw(node.as_mut_ptr()) };
    rcu_drop(value);
}

pub struct RcuListCursor<'a, T: RcuDroppable + Sync, A: RcuListAdapter<T>> {
    cursor: RcuIntrusiveListCursor<'a, T, A>,
}

impl<'a, T: RcuDroppable + Sync, A: RcuListAdapter<T>> RcuListCursor<'a, T, A> {
    /// Returns the element at the current cursor position.
    pub fn current(&self) -> Option<&T> {
        self.cursor.current()
    }

    /// Advances the cursor to the next element in the list.
    pub fn advance(&mut self) {
        self.cursor.advance();
    }

    /// Removes the element at the current cursor position.
    ///
    /// After calling `remove`, the cursor will be positioned at the next element in the list.
    ///
    /// Concurrent readers may continue to see this entry in the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn remove(&mut self) -> RcuPtrRef<'a, T> {
        let removed = unsafe { self.cursor.remove() };
        deferred_dealloc(removed);
        removed
    }
}

impl<T: RcuDroppable + Sync, A: RcuListAdapter<T>> Drop for RcuList<T, A> {
    fn drop(&mut self) {
        // SAFETY: The list is being dropped, so there are no concurrent readers.
        unsafe { self.clear() };
    }
}

#[cfg(test)]
mod tests {
    use crate::rcu_intrusive_list::{RcuListAdapter, rcu_list_adapter};

    use super::*;
    use fuchsia_rcu::rcu_run_callbacks;

    #[derive(Debug)]
    struct TestNode {
        value: i64,
        link: Link,
    }

    // SAFETY: TestNode does not perform any blocking or contextual work on drop.
    unsafe impl RcuDroppable for TestNode {}

    impl TestNode {
        fn new(value: i64) -> Self {
            Self { value, link: Default::default() }
        }
    }

    impl RcuListAdapter<TestNode> for TestNode {
        rcu_list_adapter!(TestNode, link);
    }

    #[test]
    fn test_rcu_list_push_front() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_front(&scope, TestNode::new(1));
                list.push_front(&scope, TestNode::new(2));
                list.push_front(&scope, TestNode::new(3));
            }

            let mut cursor = list.cursor(&scope);
            assert_eq!(cursor.current().map(|node| node.value), Some(3));
            cursor.advance();
            assert_eq!(cursor.current().map(|node| node.value), Some(2));
            cursor.advance();
            assert_eq!(cursor.current().map(|node| node.value), Some(1));
            cursor.advance();
            assert_eq!(cursor.current().map(|node| node.value), None);
        }
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_push_back() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let mut cursor = list.cursor(&scope);
            assert_eq!(cursor.current().map(|node| node.value), Some(1));
            cursor.advance();
            assert_eq!(cursor.current().map(|node| node.value), Some(2));
            cursor.advance();
            assert_eq!(cursor.current().map(|node| node.value), Some(3));
            cursor.advance();
            assert_eq!(cursor.current().map(|node| node.value), None);
        }
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_clear() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            unsafe { list.clear() };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), None);
        }

        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_drop_clears_objects() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Debug)]
        struct DropCounter {
            _id: usize,
            counter: Arc<AtomicUsize>,
            link: Link,
        }

        // SAFETY: DropCounter only increments an atomic counter on drop.
        unsafe impl RcuDroppable for DropCounter {}

        impl RcuListAdapter<DropCounter> for DropCounter {
            rcu_list_adapter!(DropCounter, link);
        }

        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.counter.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drop_count = Arc::new(AtomicUsize::new(0));
        {
            let list = RcuList::<DropCounter, DropCounter>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(
                    &scope,
                    DropCounter {
                        _id: 1,
                        counter: Arc::clone(&drop_count),
                        link: Default::default(),
                    },
                );
                list.push_back(
                    &scope,
                    DropCounter {
                        _id: 2,
                        counter: Arc::clone(&drop_count),
                        link: Default::default(),
                    },
                );
                list.push_back(
                    &scope,
                    DropCounter {
                        _id: 3,
                        counter: Arc::clone(&drop_count),
                        link: Default::default(),
                    },
                );
            }
            assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        }

        rcu_run_callbacks();

        // The list is dropped here, so the contained objects should also be dropped.
        assert_eq!(drop_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_rcu_list_iter() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), Some(3));
            assert_eq!(iter.next().map(|node| node.value), None);
        }

        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_remove() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let mut cursor = list.cursor(&scope);
            cursor.advance(); // current is 2
            assert_eq!(cursor.current().map(|node| node.value), Some(2));
            unsafe { cursor.remove() };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(3));
            assert_eq!(iter.next().map(|node| node.value), None);

            // Test removing head
            let mut cursor = list.cursor(&scope);
            unsafe { cursor.remove() };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(3));
            assert_eq!(iter.next().map(|node| node.value), None);

            // Test removing tail
            let mut cursor = list.cursor(&scope);
            unsafe { cursor.remove() };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), None);
        }

        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_remove_all() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let mut cursor = list.cursor(&scope);
            while cursor.current().is_some() {
                unsafe { cursor.remove() };
            }

            assert_eq!(list.iter(&scope).next().map(|node| node.value), None);
        }

        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_append() {
        {
            let list1 = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list1.push_back(&scope, TestNode::new(1));
                list1.push_back(&scope, TestNode::new(2));
            }

            let list2 = RcuList::<TestNode, TestNode>::default();
            unsafe {
                list2.push_back(&scope, TestNode::new(3));
                list2.push_back(&scope, TestNode::new(4));
            }

            unsafe { list1.append(&scope, list2) };

            let mut iter = list1.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), Some(3));
            assert_eq!(iter.next().map(|node| node.value), Some(4));
            assert_eq!(iter.next().map(|node| node.value), None);
        }

        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_append_empty() {
        // Append to an empty list.
        {
            let list1 = RcuList::<TestNode, TestNode>::default();
            let list2 = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list2.push_back(&scope, TestNode::new(1));
                list2.push_back(&scope, TestNode::new(2));
            }
            unsafe { list1.append(&scope, list2) };

            let mut iter = list1.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), None);
        }
        rcu_run_callbacks();

        // Append an empty list.
        {
            let list1 = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list1.push_back(&scope, TestNode::new(1));
                list1.push_back(&scope, TestNode::new(2));
            }
            let list2 = RcuList::<TestNode, TestNode>::default();
            unsafe { list1.append(&scope, list2) };

            let mut iter = list1.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), None);
        }
        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_is_empty() {
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            assert!(list.is_empty());

            unsafe {
                list.push_back(&scope, TestNode::new(1));
            }
            assert!(!list.is_empty());

            unsafe {
                list.clear();
            }
            assert!(list.is_empty());
        }

        rcu_run_callbacks();
    }

    #[test]
    fn test_rcu_list_split_off() {
        // Split at the beginning.
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let new_list = unsafe { list.split_off(&scope, 0) };

            assert!(list.is_empty());
            let mut new_iter = new_list.iter(&scope);
            assert_eq!(new_iter.next().map(|node| node.value), Some(1));
            assert_eq!(new_iter.next().map(|node| node.value), Some(2));
            assert_eq!(new_iter.next().map(|node| node.value), Some(3));
            assert_eq!(new_iter.next().map(|node| node.value), None);
        }
        rcu_run_callbacks();

        // Split in the middle.
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
                list.push_back(&scope, TestNode::new(4));
            }

            let new_list = unsafe { list.split_off(&scope, 2) };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), None);

            let mut new_iter = new_list.iter(&scope);
            assert_eq!(new_iter.next().map(|node| node.value), Some(3));
            assert_eq!(new_iter.next().map(|node| node.value), Some(4));
            assert_eq!(new_iter.next().map(|node| node.value), None);
        }
        rcu_run_callbacks();

        // Split at the last element.
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let new_list = unsafe { list.split_off(&scope, 2) };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), None);

            let mut new_iter = new_list.iter(&scope);
            assert_eq!(new_iter.next().map(|node| node.value), Some(3));
            assert_eq!(new_iter.next().map(|node| node.value), None);
        }
        rcu_run_callbacks();

        // Split one past the last element.
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let new_list = unsafe { list.split_off(&scope, 3) };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), Some(3));
            assert_eq!(iter.next().map(|node| node.value), None);

            assert!(new_list.is_empty());
        }
        rcu_run_callbacks();

        // Split far past the end of the list.
        {
            let list = RcuList::<TestNode, TestNode>::default();
            let scope = RcuReadScope::new();
            unsafe {
                list.push_back(&scope, TestNode::new(1));
                list.push_back(&scope, TestNode::new(2));
                list.push_back(&scope, TestNode::new(3));
            }

            let new_list = unsafe { list.split_off(&scope, 10) };

            let mut iter = list.iter(&scope);
            assert_eq!(iter.next().map(|node| node.value), Some(1));
            assert_eq!(iter.next().map(|node| node.value), Some(2));
            assert_eq!(iter.next().map(|node| node.value), Some(3));
            assert_eq!(iter.next().map(|node| node.value), None);

            assert!(new_list.is_empty());
        }
        rcu_run_callbacks();
    }
}
