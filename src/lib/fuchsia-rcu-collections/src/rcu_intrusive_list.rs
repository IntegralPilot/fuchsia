// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(unsafe_op_in_unsafe_fn)]

use fuchsia_rcu::subtle::{RcuPtr, RcuPtrRef};
use fuchsia_rcu::{RcuDroppable, RcuReadScope};

/// `Link` is an intrusive structure in a doubly-linked list.
///
/// Links are address-sensitive and cannot be moved once inserted into a list.
#[derive(Debug, RcuDroppable)]
pub struct Link {
    /// The next node in the list.
    ///
    /// This field can be used to traverse the list within an RcuReadScope.
    next: RcuPtr<Link>,

    /// The previous node in the list.
    ///
    /// This pointer cannot be used without external synchronization.
    prev: RcuPtr<Link>,
}

impl Default for Link {
    fn default() -> Self {
        Self { next: RcuPtr::null(), prev: RcuPtr::null() }
    }
}

/// Returns the container of a given field.
///
/// # Safety
///
/// The pointer must point to the given field in a valid instance of the container.
#[macro_export]
macro_rules! container_of {
    ($ptr:expr, $container:path, $field:ident) => {{ $ptr.sub_byte_offset::<$container>(memoffset::offset_of!($container, $field)) }};
}

/// Returns the field of a given container.
///
/// # Safety
///
/// The pointer must point to a valid instance of the container.
#[macro_export]
macro_rules! field_of {
    ($ptr:expr, $container:path, $field:ident, $field_type:ty) => {{ $ptr.add_byte_offset::<$field_type>(memoffset::offset_of!($container, $field)) }};
}

#[macro_export]
macro_rules! rcu_list_adapter {
    ($node:ty, $link:ident) => {
        fn to_link(
            node: fuchsia_rcu::subtle::RcuPtrRef<'_, $node>,
        ) -> fuchsia_rcu::subtle::RcuPtrRef<'_, Link> {
            if node.is_null() {
                return fuchsia_rcu::subtle::RcuPtrRef::null();
            }
            // SAFETY: The pointer is valid and points to the given field.
            unsafe { $crate::field_of!(node, $node, $link, Link) }
        }

        fn from_link(
            link: fuchsia_rcu::subtle::RcuPtrRef<'_, Link>,
        ) -> fuchsia_rcu::subtle::RcuPtrRef<'_, $node> {
            if link.is_null() {
                return fuchsia_rcu::subtle::RcuPtrRef::null();
            }
            // SAFETY: The pointer is valid and points to the given field.
            unsafe { $crate::container_of!(link, $node, $link) }
        }
    };
}

pub use container_of;
pub use field_of;
pub use rcu_list_adapter;

pub trait RcuListAdapter<T> {
    /// Returns a pointer to the Link embedded in a Node.
    fn to_link(node: RcuPtrRef<'_, T>) -> RcuPtrRef<'_, Link>;

    /// Returns a pointer to the Node containing the given Link.
    fn from_link(link: RcuPtrRef<'_, Link>) -> RcuPtrRef<'_, T>;
}

#[derive(Debug, RcuDroppable)]
pub struct RcuIntrusiveList<T, A: RcuListAdapter<T>> {
    /// The first element of the list, if any.
    ///
    /// This field can be used to traverse the list within an RcuReadScope.
    head: RcuPtr<Link>,

    /// The last element of the list, if any.
    ///
    /// This pointer cannot be used without external synchronization.
    tail: RcuPtr<Link>,

    _marker: std::marker::PhantomData<(T, A)>,
}

impl<T, A: RcuListAdapter<T>> Default for RcuIntrusiveList<T, A> {
    fn default() -> Self {
        Self::new(RcuPtr::null(), RcuPtr::null())
    }
}

impl<T, A: RcuListAdapter<T>> RcuIntrusiveList<T, A> {
    /// Creates a new list with the given head and tail.
    pub(crate) fn new(head: RcuPtr<Link>, tail: RcuPtr<Link>) -> Self {
        Self { head, tail, _marker: std::marker::PhantomData }
    }

    /// Pushes a new element to the front of the list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn push_front<'a>(&self, scope: &'a RcuReadScope, data: RcuPtrRef<'a, T>) {
        let link_ptr = A::to_link(data);
        let link = link_ptr.as_ref().unwrap();
        let head_ptr = self.head.read(&scope);
        if let Some(head) = head_ptr.as_ref() {
            head.prev.assign_ptr(link_ptr);
            link.next.assign_ptr(head_ptr);
        } else {
            self.tail.assign_ptr(link_ptr);
        }
        self.head.assign_ptr(link_ptr);
    }

    /// Pushes a new element to the back of the list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn push_back<'a>(&self, scope: &RcuReadScope, data: RcuPtrRef<'a, T>) {
        let link_ptr = A::to_link(data);
        let link = link_ptr.as_ref().unwrap();
        let tail_ptr = self.tail.read(&scope);
        if let Some(tail) = tail_ptr.as_ref() {
            link.prev.assign_ptr(tail_ptr);
            tail.next.assign_ptr(link_ptr);
        } else {
            self.head.assign_ptr(link_ptr);
        }
        self.tail.assign_ptr(link_ptr);
    }

    /// Appends another list to the end of this list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn append(&self, scope: &RcuReadScope, other: Self) {
        let other_head_ptr = other.head.read(&scope);
        if let Some(other_head) = other_head_ptr.as_ref() {
            let tail_ptr = self.tail.read(&scope);
            if let Some(tail) = tail_ptr.as_ref() {
                tail.next.assign_ptr(other_head_ptr);
                other_head.prev.assign_ptr(tail_ptr);
            } else {
                self.head.assign_ptr(other_head_ptr);
            }
            let other_tail_ptr = other.tail.read(&scope);
            assert!(!other_tail_ptr.is_null());
            self.tail.assign_ptr(other_tail_ptr);
        }
        other.head.assign(std::ptr::null_mut());
        other.tail.assign(std::ptr::null_mut());
    }

    /// Removes the given node from the list.
    ///
    /// Returns the link of the next node in the list, if any.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn remove<'a>(
        &self,
        scope: &'a RcuReadScope,
        node: RcuPtrRef<'a, T>,
    ) -> RcuPtrRef<'a, Link> {
        let link_ptr = A::to_link(node);
        let link = link_ptr.as_ref().unwrap();

        let prev = link.prev.read(scope);
        let next = link.next.read(scope);

        if let Some(next) = next.as_ref() {
            next.prev.assign_ptr(prev);
        } else {
            self.tail.assign_ptr(prev);
        }
        if let Some(prev) = prev.as_ref() {
            prev.next.assign_ptr(next);
        } else {
            self.head.assign_ptr(next);
        }

        // Other readers may continue to see this entry in the list and use the `next` pointer,
        // but they should not read the `prev` pointer anymore.
        link.prev.poison();

        next
    }

    /// Splits the list into two lists at the given position.
    ///
    /// If the given position is past the end of the list, returns an empty list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn split_off(&self, scope: &RcuReadScope, pos: usize) -> Self {
        // If we're splitting at the front, just return the entire list and
        // clear the list.
        if pos == 0 {
            let head = RcuPtr::new(self.head.replace(std::ptr::null_mut()));
            let tail = RcuPtr::new(self.tail.replace(std::ptr::null_mut()));
            return Self::new(head, tail);
        }
        let mut i = 1;
        let mut prev_ptr = self.head.read(&scope);
        while let Some(prev) = prev_ptr.as_ref() {
            if i == pos {
                let head = prev.next.replace(std::ptr::null_mut());
                if head.is_null() {
                    // There are no elements after the split point, so return an empty list.
                    break;
                }
                let tail = self.tail.read(&scope);
                self.tail.assign_ptr(prev_ptr);
                return Self::new(RcuPtr::new(head), RcuPtr::new(tail.as_mut_ptr()));
            }
            prev_ptr = prev.next.read(&scope);
            i += 1;
        }
        // We reached the end of the list, so return an empty list.
        Self::default()
    }

    /// Updates the list with the contents of another list.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn update(&self, scope: &RcuReadScope, other: Self) {
        self.head.assign_ptr(other.head.read(scope));
        self.tail.assign_ptr(other.tail.read(scope));
    }

    /// Removes all elements from the list.
    ///
    /// The callback is called for each element in the list. The caller is responsible for cleaning
    /// up the removed elements.
    ///
    /// Concurrent readers may continue to see the old value of the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn clear<'a>(&self, scope: &'a RcuReadScope, callback: impl Fn(RcuPtrRef<'a, T>))
    where
        T: 'static,
    {
        let mut current = self.head.read(scope);

        self.head.assign(std::ptr::null_mut());
        self.tail.assign(std::ptr::null_mut());

        while let Some(link) = current.as_ref() {
            let next = link.next.read(scope);

            // Other readers may continue to see this entry in the list and use the `next` pointer,
            // but they should not read the `prev` pointer anymore.
            link.prev.poison();
            callback(A::from_link(current));
            current = next;
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self, scope: &RcuReadScope) -> bool {
        self.head.read(scope).is_null()
    }

    /// Returns a cursor that can be used to traverse and modify the list.
    ///
    /// Concurrent readers may continue to see the old value of the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    pub fn cursor<'a>(&'a self, scope: &'a RcuReadScope) -> RcuIntrusiveListCursor<'a, T, A> {
        let current = self.head.read(scope);
        RcuIntrusiveListCursor { scope, list: self, current }
    }

    /// Returns an iterator over the elements in the list.
    pub fn iter<'a>(&self, scope: &'a RcuReadScope) -> impl Iterator<Item = &'a T>
    where
        T: 'static,
    {
        let next = self.head.read(&scope);
        RcuIntrusiveListIter::<T, A> { scope, next, _marker: std::marker::PhantomData }
    }
}

/// A cursor for traversing and modifying an `RcuList`.
///
/// See `RcuList::cursor` for more information.
pub struct RcuIntrusiveListCursor<'a, T, A: RcuListAdapter<T>> {
    scope: &'a RcuReadScope,
    list: &'a RcuIntrusiveList<T, A>,
    current: RcuPtrRef<'a, Link>,
}

impl<'a, T, A: RcuListAdapter<T>> RcuIntrusiveListCursor<'a, T, A> {
    /// Returns the element at the current cursor position.
    pub fn current(&self) -> Option<&'a T> {
        let node = A::from_link(self.current);
        node.as_ref()
    }

    /// Advances the cursor to the next element in the list.
    pub fn advance(&mut self) {
        if let Some(link) = self.current.as_ref() {
            self.current = link.next.read(&self.scope);
        }
    }

    /// Removes the element at the current cursor position.
    ///
    /// After calling `remove`, the cursor will be positioned at the next element in the list.
    ///
    /// Returns a pointer to the removed element. The caller is responsible for cleaning up the
    /// removed element.
    ///
    /// Concurrent readers may continue to see this entry in the list until the RCU state machine
    /// has made sufficient progress to ensure that no concurrent readers are holding read guards.
    ///
    /// # Safety
    ///
    /// Requires external synchronization to exclude concurrent writers.
    pub unsafe fn remove(&mut self) -> RcuPtrRef<'a, T> {
        if self.current.is_null() {
            return RcuPtrRef::null();
        }
        let removed_node = A::from_link(self.current);
        // SAFETY: The caller promises to exclude concurrent writers.
        unsafe {
            self.current = self.list.remove(&self.scope, removed_node);
        }
        removed_node
    }
}

struct RcuIntrusiveListIter<'a, T, A: RcuListAdapter<T>> {
    scope: &'a RcuReadScope,
    next: RcuPtrRef<'a, Link>,
    _marker: std::marker::PhantomData<(T, A)>,
}

impl<'a, T: 'static, A: RcuListAdapter<T>> Iterator for RcuIntrusiveListIter<'a, T, A> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(link) = self.next.as_ref() {
            let current = self.next;
            self.next = link.next.read(&self.scope);
            Some(A::from_link(current).as_ref().unwrap())
        } else {
            None
        }
    }
}
