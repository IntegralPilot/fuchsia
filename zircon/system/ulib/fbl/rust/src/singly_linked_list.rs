// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ptr_traits::{ManagedPtr, PtrTraits};
use crate::size_tracker::{NonTrackingSize, SizeTracker, TrackingSize};
use crate::tag::DefaultObjectTag;
use core::cell::UnsafeCell;

/// A node in a singly linked list.
#[repr(C)]
pub struct SinglyLinkedListNode<T> {
    /// The next element in the list.
    /// This is null if the node is not in a container.
    /// This is a sentinel value (1) if the node is the last element of the list.
    pub next: UnsafeCell<*mut T>,
}

impl<T> SinglyLinkedListNode<T> {
    /// Creates a new, unlinked node.
    pub const fn new() -> Self {
        Self { next: UnsafeCell::new(core::ptr::null_mut()) }
    }

    /// Returns true if the node is currently in a list.
    pub fn in_container(&self) -> bool {
        // SAFETY: `self.next.get()` returns a valid pointer to the inner field of `self.next`
        // which is a validly allocated UnsafeCell inside `self`.
        !unsafe { *self.next.get() }.is_null()
    }

    fn get_next(&self) -> *mut T {
        // SAFETY: `self.next.get()` is a valid pointer to `self.next` which is owned by `self`.
        unsafe { *self.next.get() }
    }

    fn set_next(&self, next: *mut T) {
        // SAFETY: `self.next.get()` is a valid, writable pointer to `self.next` owned by `self`.
        // UnsafeCell allows interior mutability through a shared reference.
        unsafe {
            *self.next.get() = next;
        }
    }
}

impl<T> core::fmt::Debug for SinglyLinkedListNode<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SinglyLinkedListNode").field("in_container", &self.in_container()).finish()
    }
}

impl<T> Default for SinglyLinkedListNode<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for SinglyLinkedListNode<T> {
    fn drop(&mut self) {
        debug_assert!(!self.in_container(), "Object destroyed while still in container");
    }
}

/// Trait that types must implement to be contained in a `SinglyLinkedList`.
///
/// The `Tag` parameter is used to support objects participating in multiple
/// lists simultaneously. By implementing this trait multiple times with different
/// tags, an object can provide different `SinglyLinkedListNode` instances for
/// each list it belongs to.
pub trait SinglyLinkedListContainable<T, Tag = DefaultObjectTag> {
    /// Returns a reference to the list node.
    fn get_node(&self) -> &SinglyLinkedListNode<T>;
}

/// A singly linked list that supports intrusive nodes and different ownership semantics.
///
/// `SinglyLinkedList` is a layout-compatible analog to `fbl::SinglyLinkedList` in C++.
/// It allows managing singly linked lists of objects where the bookkeeping storage
/// (the node state) exists on the objects themselves, eliminating the need for
/// runtime allocation/deallocation to add or remove members.
///
/// The list stores pointers to the objects, and is parametrized by the pointer type `P`.
/// Supported pointer types are:
/// 1. `*mut T` : Raw unmanaged pointers.
/// 2. [`UniquePtr<T>`] : Unique managed pointers.
/// 3. [`RefPtr<T>`] : Managed pointers to ref-counted objects.
///
/// ### Ownership
/// - Lists of managed pointer types ([`UniquePtr`], [`RefPtr`]) hold references to objects
///   and follow the rules of the particular managed pointer patterns. Destroying or
///   clearing a list of managed pointers will release the references to the objects.
/// - Lists of unmanaged pointer types (`*mut T`) perform no lifecycle management.
///   It is up to the user to make sure that lifecycles are managed properly.
///   As an added safety, a list of unmanaged pointers will assert if it is
///   destroyed with elements still in it.
///
/// ### Multiple Lists
/// Objects may exist in multiple lists simultaneously through the use of custom
/// trait tags. See [`SinglyLinkedListContainable`] for more details.
///
/// ### Examples
///
/// #### Example 1: A simple list of unmanaged pointers to Foo objects
///
/// ```rust
/// use fbl::{SinglyLinkedList, SinglyLinkedListNode, SinglyLinkedListContainable};
///
/// #[derive(SinglyLinkedListContainable)]
/// struct Foo {
///     value: i32,
///     #[sll_node]
///     node: SinglyLinkedListNode<Foo>,
/// }
///
/// impl Foo {
///     fn new(value: i32) -> Self {
///         Self { value, node: SinglyLinkedListNode::new() }
///     }
/// }
///
/// fn test() {
///     let mut list = SinglyLinkedList::<*mut Foo>::new();
///
///     let mut foo1 = Foo::new(1);
///     let mut foo2 = Foo::new(2);
///
///     unsafe {
///         list.push_front_raw(&mut foo1);
///         list.push_front_raw(&mut foo2);
///     }
///
///     for foo in list.iter() {
///         println!("Value: {}", foo.value);
///     }
///
///     list.clear(); // Must clear before going out of scope if using raw pointers!
/// }
/// ```
///
/// #### Example 2: A simple list of unique pointers to Foo objects
///
/// ```rust
/// use fbl::{SinglyLinkedList, SinglyLinkedListNode, SinglyLinkedListContainable, UniquePtr};
///
/// #[derive(fbl::Recyclable, SinglyLinkedListContainable)]
/// struct Foo {
///     value: i32,
///     #[sll_node]
///     node: SinglyLinkedListNode<Foo>,
/// }
///
/// impl Foo {
///     fn new(value: i32) -> Self {
///         Self { value, node: SinglyLinkedListNode::new() }
///     }
/// }
///
/// fn test() {
///     let mut list = SinglyLinkedList::<UniquePtr<Foo>>::new();
///
///     let foo1 = UniquePtr::try_new(Foo::new(1)).unwrap();
///     let foo2 = UniquePtr::try_new(Foo::new(2)).unwrap();
///
///     list.push_front(foo1);
///     list.push_front(foo2);
///
///     for foo in list.iter() {
///         println!("Value: {}", foo.value);
///     }
///
///     // List drops here and cleans up objects automatically!
/// }
/// ```
///
/// #### Example 3: An object in multiple lists
///
/// ```rust
/// use fbl::{SinglyLinkedList, SinglyLinkedListNode, SinglyLinkedListContainable};
///
/// struct Tag2;
///
/// #[derive(SinglyLinkedListContainable)]
/// struct Foo {
///     value: i32,
///     #[sll_node]
///     node1: SinglyLinkedListNode<Foo>,
///     #[sll_node(tag = Tag2)]
///     node2: SinglyLinkedListNode<Foo>,
/// }
///
/// fn test() {
///     let mut list1 = SinglyLinkedList::<*mut Foo>::new();
///     let mut list2 = SinglyLinkedList::<*mut Foo, Tag2>::new();
///
///     let mut foo = Foo {
///         value: 42,
///         node1: SinglyLinkedListNode::new(),
///         node2: SinglyLinkedListNode::new(),
///     };
///
///     unsafe {
///         list1.push_front_raw(&mut foo);
///         list2.push_front_raw(&mut foo);
///     }
///
///     // ... access via both lists ...
///
///     list1.clear();
///     list2.clear();
/// }
/// ```
#[repr(C)]
pub struct SinglyLinkedList<P, Tag = DefaultObjectTag, S = NonTrackingSize>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    head: *mut P::Target,
    size: S,
    _phantom: core::marker::PhantomData<(P, Tag)>,
}

impl<P, Tag, S> SinglyLinkedList<P, Tag, S>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    /// Creates a new, empty list.
    pub const fn new() -> Self {
        Self {
            head: crate::make_sentinel_null(),
            size: S::INIT,
            _phantom: core::marker::PhantomData,
        }
    }

    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid, aligned, and dereferenceable pointer
    /// to an initialized `P::Target` object that is alive for `'a`.
    unsafe fn get_node_ref<'a>(&self, ptr: *mut P::Target) -> &'a SinglyLinkedListNode<P::Target> {
        let _ = self;
        // SAFETY: The caller guarantees `ptr` is valid, aligned, and dereferenceable.
        unsafe { &(*ptr) }.get_node()
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        crate::is_sentinel_ptr(self.head)
    }

    /// Returns a reference to the first element of the list, or `None` if it is empty.
    pub fn front(&self) -> Option<&P::Target> {
        if self.is_empty() {
            None
        } else {
            // SAFETY: The list is not empty, so `self.head` is a valid pointer to an element.
            unsafe { Some(&*self.head) }
        }
    }

    /// Returns a mutable reference to the first element of the list, or `None` if it is empty.
    pub fn front_mut(&mut self) -> Option<&mut P::Target> {
        if self.is_empty() {
            None
        } else {
            // SAFETY: The list is not empty, so `self.head` is a valid pointer to an element.
            // We have `&mut self`, ensuring exclusive access.
            unsafe { Some(&mut *self.head) }
        }
    }

    /// Pushes an element to the front of the list.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    pub fn push_front(&mut self, ptr: P)
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this list.
        unsafe { self.push_front_raw(ptr) }
    }

    /// Pushes an element to the front of the list.
    ///
    /// For managed pointers, use the safe [`push_front`] instead.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the list.
    pub unsafe fn push_front_raw(&mut self, ptr: P) {
        let raw = P::into_raw(ptr);
        debug_assert!(!raw.is_null());
        // SAFETY: `raw` is a valid pointer provided by caller.
        let node = unsafe { self.get_node_ref(raw) };
        assert!(!node.in_container());

        node.set_next(self.head);
        self.head = raw;
        self.size.increment();
    }

    /// Removes and returns the first element of the list, or `None` if it is empty.
    pub fn pop_front(&mut self) -> Option<P> {
        if self.is_empty() {
            return None;
        }

        let ptr = self.head;
        self.size.decrement();

        // SAFETY: `ptr` was `self.head` which is valid since list is not empty.
        let node = unsafe { self.get_node_ref(ptr) };

        self.head = node.get_next();
        node.set_next(core::ptr::null_mut());

        // SAFETY: `ptr` was popped, safe to reconstruct.
        Some(unsafe { P::from_raw(ptr) })
    }

    /// Removes all elements from the list.
    pub fn clear(&mut self) {
        while let Some(_) = self.pop_front() {}
    }

    /// Inserts an element after the specified position.
    ///
    /// For managed pointers, consider using [`CursorMut::insert_after`] for a safer alternative.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `pos` is a valid pointer to an element in this list,
    /// and that `ptr` is a valid pointer to an element not currently in any container.
    pub unsafe fn insert_after_raw(&mut self, pos: *mut P::Target, ptr: P) {
        debug_assert!(!pos.is_null());
        let raw = P::into_raw(ptr);
        debug_assert!(!raw.is_null());
        // SAFETY: `raw` is valid.
        let node = unsafe { self.get_node_ref(raw) };
        debug_assert!(!node.in_container());

        // SAFETY: `pos` is valid.
        let pos_node = unsafe { self.get_node_ref(pos) };
        node.set_next(pos_node.get_next());
        pos_node.set_next(raw);
        self.size.increment();
    }

    /// Erases the element after the specified position.
    ///
    /// For managed pointers, consider using [`CursorMut::erase_next`] for a safer alternative.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `pos` is a valid pointer to an element in this list.
    pub unsafe fn erase_next_raw(&mut self, pos: *mut P::Target) -> Option<P> {
        debug_assert!(!pos.is_null());
        // SAFETY: `pos` is valid.
        let pos_node = unsafe { self.get_node_ref(pos) };
        let next_ptr = pos_node.get_next();
        if crate::is_sentinel_ptr(next_ptr) {
            return None;
        }
        // SAFETY: `next_ptr` is valid.
        let next_node = unsafe { self.get_node_ref(next_ptr) };
        pos_node.set_next(next_node.get_next());
        next_node.set_next(core::ptr::null_mut());
        self.size.decrement();
        // SAFETY: `next_ptr` was erased, safe to reconstruct.
        Some(unsafe { P::from_raw(next_ptr) })
    }

    /// Swaps the contents of this list with another list.
    pub fn swap(&mut self, other: &mut Self) {
        core::mem::swap(&mut self.head, &mut other.head);
        self.size.swap(&mut other.size);
    }

    /// Finds the first element matching the predicate, removes it from the list,
    /// and returns it. Returns `None` if no element matches.
    pub fn erase_if<F>(&mut self, mut f: F) -> Option<P>
    where
        F: FnMut(&P::Target) -> bool,
    {
        // Step 1: Check if head matches.
        if let Some(head_ref) = self.front() {
            if f(head_ref) {
                return self.pop_front();
            }
        }

        if self.is_empty() {
            return None;
        }

        // Step 2: Use a cursor to check subsequent elements.
        let mut cursor = self.cursor_mut();
        while let Some(next_ref) = cursor.get_next() {
            if f(next_ref) {
                return cursor.erase_next();
            }
            cursor.move_next();
        }

        None
    }

    /// Replaces the first element matching the predicate with `value`. Returns the replaced
    /// element.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    pub fn replace_if<F>(&mut self, f: F, value: P) -> Option<P>
    where
        F: FnMut(&P::Target) -> bool,
        P: ManagedPtr,
    {
        unsafe { self.replace_if_raw(f, value) }
    }

    /// Replaces the first element matching the predicate with `value`. Returns the replaced
    /// element.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `value` is a valid pointer to a `T` and that the object outlives
    /// the reference from the list.
    pub unsafe fn replace_if_raw<F>(&mut self, mut f: F, value: P) -> Option<P>
    where
        F: FnMut(&P::Target) -> bool,
    {
        // Step 1: Handle matching elements at the head.
        if let Some(head_ref) = self.front() {
            if f(head_ref) {
                let old_head = self.pop_front().unwrap();
                // SAFETY: `value` is a valid pointer that will outlive its reference from this
                // list.
                unsafe { self.push_front_raw(value) };
                return Some(old_head);
            }
        }

        // Step 2: Head does not match. Use a cursor to check subsequent elements.
        let mut cursor = self.cursor_mut();
        while let Some(next_ref) = cursor.get_next() {
            if f(next_ref) {
                // SAFETY: `value` is a valid pointer that will outlive its reference from this
                // list.
                unsafe {
                    return cursor.replace_next_raw(value);
                }
            }
            cursor.move_next();
        }

        None
    }

    /// Splits the list after the specified position, returning a new list
    /// containing the elements that followed `pos`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `pos` is a valid pointer to an element in this list.
    pub unsafe fn split_after_raw(&mut self, pos: *mut P::Target) -> Self {
        debug_assert!(!pos.is_null());
        // SAFETY: The caller must ensure that `pos` is a valid pointer to an element in this list.
        let pos_node = unsafe { &(*pos) }.get_node();
        let next_ptr = unsafe { *pos_node.next.get() };

        unsafe { *pos_node.next.get() = crate::make_sentinel_null() };

        let mut new_list =
            Self { head: next_ptr, size: S::INIT, _phantom: core::marker::PhantomData };

        if S::IS_TRACKING {
            let new_size = new_list.size_slow();
            new_list.size.set(new_size);
            self.size.set(self.size.get() - new_size);
        }

        new_list
    }

    /// Retains only the elements specified by the predicate.
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&P::Target) -> bool,
    {
        self.erase_if(|x| !f(x));
    }

    /// Returns a cursor positioned at the front of the list.
    pub fn cursor_mut(&mut self) -> CursorMut<'_, P, Tag, S> {
        let head = self.head;
        CursorMut { list: self, current: head }
    }

    /// Calculates the size of the list by iterating through all elements. O(N) time complexity.
    pub fn size_slow(&self) -> usize {
        if let Some(head) = self.front() {
            let iter = Iterator::<'_, P, Tag>::from_element(head);
            let mut count = 0;
            for _ in iter {
                count += 1;
            }
            count
        } else {
            0
        }
    }

    /// Finds the first element matching the predicate.
    pub fn find_if<F>(&self, mut f: F) -> Option<&P::Target>
    where
        F: FnMut(&P::Target) -> bool,
    {
        self.iter().find(|&x| f(x))
    }

    /// Returns an iterator over the elements of the list.
    pub fn iter(&self) -> Iterator<'_, P, Tag> {
        Iterator::new(self.head)
    }
}

impl<P, Tag> SinglyLinkedList<P, Tag, TrackingSize>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
{
    /// Returns the size of the list. O(1) time complexity.
    pub fn size(&self) -> usize {
        self.size.get()
    }
}

impl<P, Tag, S> Drop for SinglyLinkedList<P, Tag, S>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    fn drop(&mut self) {
        if P::IS_MANAGED {
            self.clear();
        } else {
            debug_assert!(self.is_empty(), "List must be empty on destruction");
            if S::IS_TRACKING {
                debug_assert_eq!(self.size.get(), 0, "Size must be zero on destruction");
            }
        }
    }
}

/// An iterator over the elements of a `SinglyLinkedList`.
pub struct Iterator<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a (P, Tag)>,
}

impl<'a, P, Tag> Iterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }
}

impl<'a, P, Tag> core::iter::Iterator for Iterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        if crate::is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is not a sentinel, so it is a valid, aligned pointer to an element.
            // The list is guaranteed to be immutable for the lifetime `'a` of the iterator.
            let current = unsafe { &*self.current };
            // SAFETY: `current` is a valid reference, so we can safely read `next` from its node.
            self.current = unsafe { *current.get_node().next.get() };
            Some(current)
        }
    }
}

impl<'a, P, Tag> Iterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
{
    /// Creates an iterator starting from a specific element.
    ///
    /// # Panics
    ///
    /// Panics if the object is not in a container.
    pub fn from_element(obj: &'a P::Target) -> Self {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Self { current: obj as *const _ as *mut _, _phantom: core::marker::PhantomData }
    }
}

/// A cursor that can be used to iterate and modify a `SinglyLinkedList`.
pub struct CursorMut<'a, P, Tag = DefaultObjectTag, S = NonTrackingSize>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    list: &'a mut SinglyLinkedList<P, Tag, S>,
    current: *mut P::Target,
}

impl<'a, P, Tag, S> CursorMut<'a, P, Tag, S>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    /// Returns a reference to the element at the current position.
    pub fn get(&self) -> Option<&P::Target> {
        if crate::is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is not a sentinel, so it is a valid pointer to an element.
            Some(unsafe { &*self.current })
        }
    }

    /// Moves the cursor to the next element. Returns true if the cursor is now at a valid element.
    pub fn move_next(&mut self) -> bool {
        if crate::is_sentinel_ptr(self.current) {
            return false;
        }
        // SAFETY: `self.current` is valid.
        let node = unsafe { self.list.get_node_ref(self.current) };
        self.current = node.get_next();
        !crate::is_sentinel_ptr(self.current)
    }

    /// Returns a reference to the element after the current position.
    pub fn get_next(&self) -> Option<&P::Target> {
        if crate::is_sentinel_ptr(self.current) {
            return None;
        }
        // SAFETY: `self.current` is valid.
        let node = unsafe { self.list.get_node_ref(self.current) };
        let next_ptr = node.get_next();
        if crate::is_sentinel_ptr(next_ptr) {
            None
        } else {
            // SAFETY: `next_ptr` is not sentinel, so it is a valid pointer.
            unsafe { Some(&*next_ptr) }
        }
    }

    /// Inserts a new element after the current position.
    pub fn insert_after(&mut self, ptr: P)
    where
        P: ManagedPtr,
    {
        assert!(!crate::is_sentinel_ptr(self.current), "Cannot insert after end sentinel");
        // SAFETY: `self.current` is a valid pointer to an element in the list, and `ptr` is managed.
        unsafe { self.list.insert_after_raw(self.current, ptr) }
    }

    /// Erases the element after the current position. Returns the erased element.
    pub fn erase_next(&mut self) -> Option<P> {
        assert!(!crate::is_sentinel_ptr(self.current), "Cannot erase next of end sentinel");
        // SAFETY: `self.current` is a valid pointer to an element in the list.
        unsafe { self.list.erase_next_raw(self.current) }
    }

    /// Replaces the element after the current position. Returns the replaced element.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `value` is a valid pointer to a `T` and that the object outlives
    /// the reference from the list.
    pub unsafe fn replace_next_raw(&mut self, value: P) -> Option<P> {
        debug_assert!(!crate::is_sentinel_ptr(self.current), "Cannot replace next of end sentinel");

        // SAFETY: `self.current` is valid.
        let current_node = unsafe { self.list.get_node_ref(self.current) };
        let next_ptr = current_node.get_next();
        if crate::is_sentinel_ptr(next_ptr) {
            return None;
        }

        let value_raw = P::into_raw(value);
        // SAFETY: `value_raw` is valid.
        let value_node = unsafe { self.list.get_node_ref(value_raw) };
        assert!(!value_node.in_container());

        // SAFETY: `next_ptr` is valid.
        let next_node = unsafe { self.list.get_node_ref(next_ptr) };

        value_node.set_next(next_node.get_next());
        current_node.set_next(value_raw);
        next_node.set_next(core::ptr::null_mut());

        // SAFETY: `next_ptr` was replaced, safe to reconstruct.
        Some(unsafe { P::from_raw(next_ptr) })
    }
}

impl<P, Tag, S> core::fmt::Debug for SinglyLinkedList<P, Tag, S>
where
    P: PtrTraits,
    P::Target: SinglyLinkedListContainable<P::Target, Tag> + core::fmt::Debug,
    S: SizeTracker,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::intrusive_container_test_support::*;
    use crate::ref_ptr::RefPtr;
    use crate::unique_ptr::UniquePtr;
    use alloc::sync::Arc;
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicBool, Ordering};

    // Dummy type to satisfy trait bounds for static asserts.
    struct DummyTarget;
    impl SinglyLinkedListContainable<DummyTarget, DefaultObjectTag> for DummyTarget {
        fn get_node(&self) -> &SinglyLinkedListNode<DummyTarget> {
            unreachable!()
        }
    }

    zr::static_assert!(
        core::mem::size_of::<SinglyLinkedList<*mut DummyTarget>>()
            == core::mem::size_of::<*mut DummyTarget>()
    );
    zr::static_assert!(
        core::mem::align_of::<SinglyLinkedList<*mut DummyTarget>>()
            == core::mem::align_of::<*mut DummyTarget>()
    );

    zr::static_assert!(
        core::mem::size_of::<SinglyLinkedList<*mut DummyTarget, DefaultObjectTag, TrackingSize>>()
            == 2 * core::mem::size_of::<*mut DummyTarget>()
    );
    zr::static_assert!(
        core::mem::align_of::<SinglyLinkedList<*mut DummyTarget, DefaultObjectTag, TrackingSize>>()
            == core::mem::align_of::<*mut DummyTarget>()
    );

    zr::static_assert!(
        core::mem::size_of::<Iterator<'_, *mut DummyTarget>>()
            == core::mem::size_of::<*mut DummyTarget>()
    );
    zr::static_assert!(
        core::mem::align_of::<Iterator<'_, *mut DummyTarget>>()
            == core::mem::align_of::<*mut DummyTarget>()
    );

    macro_rules! generate_list_tests {
        ($mod_name:ident, $ptr_type:ty, $factory_type:ty, $get_val:expr, $push:expr) => {
            mod $mod_name {
                use super::*;

                #[test]
                fn test_basic_ops() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    assert!(list.is_empty());

                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(&mut list, obj1);
                    $push(&mut list, obj2);
                    $push(&mut list, obj3);

                    assert!(!list.is_empty());

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert_eq!(iter.next().unwrap().value, 2);
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert!(iter.next().is_none());

                    assert_eq!($get_val(list.pop_front().unwrap().get_ref()), 3);
                    assert_eq!($get_val(list.pop_front().unwrap().get_ref()), 2);
                    assert_eq!($get_val(list.pop_front().unwrap().get_ref()), 1);

                    assert!(list.is_empty());
                }

                #[test]
                fn test_insert_after() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();

                    let obj1 = factory.create(1);
                    let raw1 = <$ptr_type as PtrTraits>::into_raw(obj1);
                    $push(&mut list, unsafe { <$ptr_type as PtrTraits>::from_raw(raw1) });

                    let obj2 = factory.create(2);
                    let raw2 = <$ptr_type as PtrTraits>::into_raw(obj2);
                    unsafe {
                        list.insert_after_raw(raw1, <$ptr_type as PtrTraits>::from_raw(raw2));
                    }

                    let obj3 = factory.create(3);
                    unsafe {
                        list.insert_after_raw(raw2, obj3);
                    }

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert_eq!(iter.next().unwrap().value, 2);
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert!(iter.next().is_none());

                    // Cleanup
                    list.clear();
                }

                #[test]
                fn test_clear() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(&mut list, obj1);
                    $push(&mut list, obj2);

                    assert!(!list.is_empty());
                    list.clear();
                    assert!(list.is_empty());
                }

                #[test]
                fn test_erase_next() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();

                    let obj1 = factory.create(1);
                    let raw1 = <$ptr_type as PtrTraits>::into_raw(obj1);
                    $push(&mut list, unsafe { <$ptr_type as PtrTraits>::from_raw(raw1) });

                    let obj2 = factory.create(2);
                    let raw2 = <$ptr_type as PtrTraits>::into_raw(obj2);
                    unsafe {
                        list.insert_after_raw(raw1, <$ptr_type as PtrTraits>::from_raw(raw2));
                    }

                    let obj3 = factory.create(3);
                    unsafe {
                        list.insert_after_raw(raw2, obj3);
                    }

                    let erased = unsafe { list.erase_next_raw(raw1) };
                    assert_eq!($get_val(erased.unwrap().get_ref()), 2);

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert!(iter.next().is_none());

                    // Cleanup
                    list.clear();
                }

                #[test]
                fn test_swap() {
                    let mut factory = <$factory_type>::new();
                    let mut list1 = SinglyLinkedList::<$ptr_type>::new();
                    let mut list2 = SinglyLinkedList::<$ptr_type>::new();

                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(&mut list1, obj1);
                    $push(&mut list2, obj2);

                    list1.swap(&mut list2);

                    assert_eq!($get_val(list1.pop_front().unwrap().get_ref()), 2);
                    assert_eq!($get_val(list2.pop_front().unwrap().get_ref()), 1);
                }

                #[test]
                fn test_size_slow() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    assert_eq!(list.size_slow(), 0);

                    $push(&mut list, obj1);
                    assert_eq!(list.size_slow(), 1);
                    $push(&mut list, obj2);
                    assert_eq!(list.size_slow(), 2);

                    list.pop_front();
                    assert_eq!(list.size_slow(), 1);

                    list.pop_front();
                    assert_eq!(list.size_slow(), 0);
                }

                #[test]
                fn test_find_if() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(&mut list, obj1);
                    $push(&mut list, obj2);

                    let found = list.find_if(|o| o.value == 1);
                    assert!(found.is_some());
                    assert_eq!(found.unwrap().value, 1);

                    let found = list.find_if(|o| o.value == 3);
                    assert!(found.is_none());

                    // Cleanup
                    list.clear();
                }

                #[test]
                fn test_erase_if() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(&mut list, obj1);
                    $push(&mut list, obj2);
                    $push(&mut list, obj3);

                    // list: 3 -> 2 -> 1

                    let erased = list.erase_if(|o| o.value == 2);
                    assert!(erased.is_some());
                    assert_eq!($get_val(erased.unwrap().get_ref()), 2);

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert!(iter.next().is_none());

                    // Cleanup
                    list.clear();
                }

                #[test]
                fn test_replace_if() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(&mut list, obj1);
                    $push(&mut list, obj2);

                    // list: 2 -> 1

                    let replaced = unsafe { list.replace_if_raw(|o| o.value == 2, obj3) };
                    assert_eq!($get_val(replaced.unwrap().get_ref()), 2);

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert!(iter.next().is_none());

                    // Cleanup
                    list.clear();
                }

                #[test]
                fn test_split_after() {
                    let mut factory = <$factory_type>::new();
                    let mut list = SinglyLinkedList::<$ptr_type>::new();
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(&mut list, obj1);
                    $push(&mut list, obj2);
                    $push(&mut list, obj3);

                    // list: 3 -> 2 -> 1

                    let raw_pos = {
                        let found = list.find_if(|o| o.value == 2).unwrap();
                        found as *const <$ptr_type as PtrTraits>::Target
                            as *mut <$ptr_type as PtrTraits>::Target
                    };

                    let mut other = unsafe { list.split_after_raw(raw_pos) };

                    // list should be 3 -> 2
                    // other should be 1

                    assert_eq!(list.size_slow(), 2);
                    assert_eq!(other.size_slow(), 1);

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert_eq!(iter.next().unwrap().value, 2);
                    assert!(iter.next().is_none());

                    let mut iter = other.iter();
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert!(iter.next().is_none());

                    // Cleanup
                    list.clear();
                    other.clear();
                }
            }
        };
    }

    #[derive(fbl::Recyclable, crate::SinglyLinkedListContainable)]
    struct TestObject {
        value: i32,
        #[sll_node]
        node: SinglyLinkedListNode<TestObject>,
    }

    impl TestObject {
        fn new(value: i32) -> Self {
            Self { value, node: SinglyLinkedListNode::new() }
        }
    }

    impl TestValue for TestObject {
        fn new(value: i32) -> Self {
            Self::new(value)
        }
    }

    generate_list_tests!(
        raw_ptr_tests,
        *mut TestObject,
        RawFactory<TestObject>,
        |p: &TestObject| p.value,
        |list: &mut SinglyLinkedList<*mut TestObject>, obj| unsafe {
            list.push_front_raw(obj);
        }
    );

    #[derive(fbl::Recyclable, crate::SinglyLinkedListContainable)]
    struct UniqueTestObject {
        value: i32,
        #[sll_node]
        node: SinglyLinkedListNode<UniqueTestObject>,
    }

    impl UniqueTestObject {
        fn new(value: i32) -> Self {
            Self { value, node: SinglyLinkedListNode::new() }
        }
    }

    impl TestValue for UniqueTestObject {
        fn new(value: i32) -> Self {
            Self::new(value)
        }
    }

    generate_list_tests!(
        unique_ptr_tests,
        UniquePtr<UniqueTestObject>,
        UniqueFactory<UniqueTestObject>,
        |p: &UniqueTestObject| p.value,
        |list: &mut SinglyLinkedList<UniquePtr<UniqueTestObject>>, obj| list.push_front(obj)
    );

    #[fbl::ref_counted]
    #[derive(crate::SinglyLinkedListContainable, crate::Recyclable)]
    #[repr(C)]
    pub struct RefTestObject {
        value: i32,
        #[sll_node]
        node: SinglyLinkedListNode<RefTestObject>,
    }

    impl TestValue for RefTestObject {
        fn new_ref_counted(value: i32) -> RefPtr<Self> {
            crate::make_ref_counted!(RefTestObject {
                value: value,
                node: SinglyLinkedListNode::new()
            })
            .unwrap()
        }
    }

    generate_list_tests!(
        ref_ptr_tests,
        RefPtr<RefTestObject>,
        RefFactory<RefTestObject>,
        |p: &RefTestObject| p.value,
        |list: &mut SinglyLinkedList<RefPtr<RefTestObject>>, obj| list.push_front(obj)
    );

    #[test]
    fn test_ref_ptr_identity() {
        let mut list = SinglyLinkedList::<RefPtr<RefTestObject>>::new();
        let obj =
            crate::make_ref_counted!(RefTestObject { value: 1, node: SinglyLinkedListNode::new() })
                .unwrap();
        list.push_front(obj.clone());
        let popped = list.pop_front().unwrap();
        assert!(RefPtr::ptr_eq(&popped, &obj));
    }

    #[derive(crate::SinglyLinkedListContainable)]
    #[repr(C)]
    pub struct BaseItem {
        #[sll_node]
        node: SinglyLinkedListNode<BaseItem>,
    }

    #[repr(C)]
    pub struct RustItem {
        base: BaseItem,
        value: i32,
    }

    unsafe extern "C" {
        fn create_cpp_list() -> *mut c_void;
        fn destroy_cpp_list(list_ptr: *mut c_void);
        fn create_cpp_item(value: i32) -> *mut c_void;
        fn destroy_cpp_item(item_ptr: *mut c_void);
        fn list_push_front(list_ptr: *mut c_void, item_ptr: *mut c_void);
        fn list_pop_front(list_ptr: *mut c_void) -> *mut c_void;
        fn list_is_empty(list_ptr: *mut c_void) -> bool;
        fn get_cpp_item_value(item_ptr: *mut c_void) -> i32;
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_cross_lang_list() {
        use core::ffi::c_void;

        unsafe {
            let list_ptr = create_cpp_list();
            assert!(list_is_empty(list_ptr));

            let cpp_item1 = create_cpp_item(10);
            let cpp_item2 = create_cpp_item(20);

            let mut rust_item =
                RustItem { base: BaseItem { node: SinglyLinkedListNode::new() }, value: 30 };

            // Push C++ item 1
            list_push_front(list_ptr, cpp_item1);
            assert!(!list_is_empty(list_ptr));

            // Push Rust item
            list_push_front(list_ptr, &mut rust_item.base as *mut BaseItem as *mut c_void);

            // Push C++ item 2
            list_push_front(list_ptr, cpp_item2);

            // Now list should have: CppItem(20) -> RustItem(30) -> CppItem(10)

            // Pop C++ item 2
            let popped = list_pop_front(list_ptr);
            assert_eq!(get_cpp_item_value(popped), 20);
            destroy_cpp_item(popped);

            // Pop Rust item
            let popped = list_pop_front(list_ptr);
            let popped_rust_item = popped as *mut RustItem; // Safe because we know it's a RustItem
            assert_eq!((*popped_rust_item).value, 30);

            // Pop C++ item 1
            let popped = list_pop_front(list_ptr);
            assert_eq!(get_cpp_item_value(popped), 10);
            destroy_cpp_item(popped);

            assert!(list_is_empty(list_ptr));
            destroy_cpp_list(list_ptr);
        }
    }

    struct Tag2;

    #[fbl::ref_counted]
    #[derive(crate::SinglyLinkedListContainable, crate::Recyclable)]
    #[repr(C)]
    struct MultiListObject {
        value: i32,
        #[sll_node]
        node1: SinglyLinkedListNode<MultiListObject>,
        node2: SinglyLinkedListNode<MultiListObject>,
    }

    impl SinglyLinkedListContainable<MultiListObject, Tag2> for MultiListObject {
        fn get_node(&self) -> &SinglyLinkedListNode<MultiListObject> {
            &self.node2
        }
    }

    #[test]
    fn test_multiple_lists() {
        let mut list1 = SinglyLinkedList::<RefPtr<MultiListObject>, DefaultObjectTag>::new();
        let mut list2 = SinglyLinkedList::<RefPtr<MultiListObject>, Tag2>::new();

        let obj1 = fbl::make_ref_counted!(MultiListObject {
            value: 1,
            node1: SinglyLinkedListNode::new(),
            node2: SinglyLinkedListNode::new(),
        })
        .unwrap();

        let obj2 = fbl::make_ref_counted!(MultiListObject {
            value: 2,
            node1: SinglyLinkedListNode::new(),
            node2: SinglyLinkedListNode::new(),
        })
        .unwrap();

        list1.push_front(obj1.clone());
        list1.push_front(obj2.clone());

        list2.push_front(obj1.clone());

        let mut iter1 = list1.iter();
        assert_eq!(iter1.next().unwrap().value, 2);
        assert_eq!(iter1.next().unwrap().value, 1);
        assert!(iter1.next().is_none());

        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 1);
        assert!(iter2.next().is_none());

        assert_eq!(list1.pop_front().unwrap().value, 2);
        assert_eq!(list2.pop_front().unwrap().value, 1);

        assert!(!list1.is_empty());
        assert!(list2.is_empty());

        assert_eq!(list1.pop_front().unwrap().value, 1);
        assert!(list1.is_empty());
    }

    #[test]
    fn test_size_tracking() {
        let mut list =
            SinglyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new();

        assert_eq!(list.size(), 0);
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        assert_eq!(list.size(), 1);
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        assert_eq!(list.size(), 2);

        list.pop_front();
        assert_eq!(list.size(), 1);
        list.clear();
    }

    #[test]
    fn test_split_after_size_tracking() {
        let mut list = SinglyLinkedList::<*mut TestObject, DefaultObjectTag, TrackingSize>::new();
        let mut obj1 = TestObject::new(1);
        let mut obj2 = TestObject::new(2);
        let mut obj3 = TestObject::new(3);

        unsafe {
            list.push_front_raw(&mut obj1);
            list.push_front_raw(&mut obj2);
            list.push_front_raw(&mut obj3);
        }

        assert_eq!(list.size(), 3);

        let raw_pos = {
            let found = list.find_if(|o| o.value == 2).unwrap();
            found as *const TestObject as *mut TestObject
        };

        let mut other = unsafe { list.split_after_raw(raw_pos) };

        assert_eq!(list.size(), 2);
        assert_eq!(other.size(), 1);

        list.clear();
        other.clear();
    }

    #[test]
    fn test_lifecycle_on_drop() {
        let mut list = SinglyLinkedList::<UniquePtr<UniqueTestObject>>::new();
        let obj1 = UniquePtr::try_new(UniqueTestObject::new(1)).unwrap();
        let obj2 = UniquePtr::try_new(UniqueTestObject::new(2)).unwrap();

        list.push_front(obj1);
        list.push_front(obj2);

        drop(list);
    }

    #[derive(crate::SinglyLinkedListContainable)]
    struct DerivedObject {
        value: i32,
        #[sll_node]
        node: SinglyLinkedListNode<DerivedObject>,
    }

    impl DerivedObject {
        fn new(value: i32) -> Self {
            Self { value, node: SinglyLinkedListNode::new() }
        }
    }

    #[test]
    fn test_derive_containable() {
        let mut list = SinglyLinkedList::<*mut DerivedObject>::new();
        let mut obj1 = DerivedObject::new(1);
        let mut obj2 = DerivedObject::new(2);

        unsafe {
            list.push_front_raw(&mut obj1);
            list.push_front_raw(&mut obj2);
        }

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 2);
        assert_eq!(iter.next().unwrap().value, 1);
        assert!(iter.next().is_none());
        list.clear();
    }

    struct Tag3;

    #[derive(crate::SinglyLinkedListContainable)]
    struct MultiDerivedObject {
        value: i32,
        #[sll_node]
        node1: SinglyLinkedListNode<MultiDerivedObject>,
        #[sll_node(tag = Tag3)]
        node2: SinglyLinkedListNode<MultiDerivedObject>,
    }

    impl MultiDerivedObject {
        fn new(value: i32) -> Self {
            Self { value, node1: SinglyLinkedListNode::new(), node2: SinglyLinkedListNode::new() }
        }
    }

    #[test]
    fn test_derive_containable_multi() {
        let mut list1 = SinglyLinkedList::<*mut MultiDerivedObject, DefaultObjectTag>::new();
        let mut list2 = SinglyLinkedList::<*mut MultiDerivedObject, Tag3>::new();

        let mut obj1 = MultiDerivedObject::new(1);
        let mut obj2 = MultiDerivedObject::new(2);

        unsafe {
            list1.push_front_raw(core::ptr::addr_of_mut!(obj1));
            list1.push_front_raw(core::ptr::addr_of_mut!(obj2));

            list2.push_front_raw(core::ptr::addr_of_mut!(obj1));
        }

        let mut iter1 = list1.iter();
        assert_eq!(iter1.next().unwrap().value, 2);
        assert_eq!(iter1.next().unwrap().value, 1);

        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 1);

        list1.clear();
        list2.clear();
    }

    #[test]
    fn test_retain() {
        let mut list = SinglyLinkedList::<UniquePtr<UniqueTestObject>>::new();
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());

        list.retain(|o| o.value % 2 != 0);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());
        list.clear();
    }

    #[test]
    fn test_cursor_mut() {
        let mut list = SinglyLinkedList::<UniquePtr<UniqueTestObject>>::new();
        let obj1 = UniquePtr::try_new(UniqueTestObject::new(1)).unwrap();
        let obj2 = UniquePtr::try_new(UniqueTestObject::new(2)).unwrap();
        let obj3 = UniquePtr::try_new(UniqueTestObject::new(3)).unwrap();

        list.push_front(obj1);
        list.push_front(obj2);

        let mut cursor = list.cursor_mut();
        assert_eq!(cursor.get().unwrap().value, 2);

        cursor.insert_after(obj3);

        assert!(cursor.move_next());
        assert_eq!(cursor.get().unwrap().value, 3);

        assert!(cursor.move_next());
        assert_eq!(cursor.get().unwrap().value, 1);

        assert!(!cursor.move_next());

        // Reset cursor
        let mut cursor = list.cursor_mut();
        assert_eq!(cursor.get().unwrap().value, 2);

        let erased = cursor.erase_next().unwrap();
        assert_eq!(erased.value, 3);

        assert!(cursor.move_next());
        assert_eq!(cursor.get().unwrap().value, 1);

        assert!(!cursor.move_next());
    }

    #[test]
    fn test_iterator_from_element() {
        let mut list = SinglyLinkedList::<UniquePtr<UniqueTestObject>>::new();
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());

        let mut iter = list.iter();
        iter.next(); // obj1
        let obj2_ref = iter.next().unwrap();

        let mut from_element_iter: Iterator<'_, UniquePtr<UniqueTestObject>> =
            Iterator::from_element(obj2_ref);
        assert_eq!(from_element_iter.next().unwrap().value, 2);
        assert_eq!(from_element_iter.next().unwrap().value, 3);
        assert!(from_element_iter.next().is_none());

        list.clear();
    }

    #[test]
    fn test_front_ops() {
        let mut list = SinglyLinkedList::<UniquePtr<UniqueTestObject>>::new();
        assert!(list.front().is_none());

        list.push_front(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        assert_eq!(list.front().unwrap().value, 1);

        list.push_front(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        assert_eq!(list.front().unwrap().value, 2);

        list.clear();
    }

    // SLL FFI Declarations
    unsafe extern "C" {
        // UniqueList Helpers
        fn cpp_sll_create_unique_list() -> *mut c_void;
        fn cpp_sll_destroy_unique_list(list: *mut c_void);
        fn cpp_sll_unique_list_push_front(list: *mut c_void, item: *mut c_void);
        fn cpp_sll_unique_list_pop_front(list: *mut c_void) -> *mut c_void;
        fn cpp_sll_unique_list_is_empty(list: *mut c_void) -> bool;

        // RefList Helpers
        fn cpp_sll_create_ref_list() -> *mut c_void;
        fn cpp_sll_destroy_ref_list(list: *mut c_void);
        fn cpp_sll_ref_list_push_front(list: *mut c_void, item: *mut c_void);
        fn cpp_sll_ref_list_pop_front(list: *mut c_void) -> *mut c_void;
        fn cpp_sll_ref_list_is_empty(list: *mut c_void) -> bool;

        // SharedUniqueObject Helpers (Defined in DLL tests C++ file)
        fn cpp_create_unique_object(value: i32, destruction_flag: *mut bool) -> *mut c_void;
        fn cpp_get_unique_object_value(obj: *mut c_void) -> i32;

        // SharedRefObject Helpers (Defined in DLL tests C++ file)
        fn cpp_create_ref_object(value: i32, destruction_flag: *mut bool) -> *mut c_void;
        fn cpp_get_ref_object_value(obj: *mut c_void) -> i32;
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_interop_rust_list_cpp_unique_objects() {
        let destroyed1 = AtomicBool::new(false);
        let destroyed2 = AtomicBool::new(false);

        unsafe {
            let mut list = SinglyLinkedList::<UniquePtr<SharedUniqueObject>>::new();

            let cpp_raw1 = cpp_create_unique_object(1, destroyed1.as_ptr() as *mut bool);
            let cpp_raw2 = cpp_create_unique_object(2, destroyed2.as_ptr() as *mut bool);

            let obj1 = UniquePtr::from_raw(cpp_raw1 as *mut SharedUniqueObject);
            let obj2 = UniquePtr::from_raw(cpp_raw2 as *mut SharedUniqueObject);

            list.push_front(obj1);
            list.push_front(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one
            let popped = list.pop_front();
            assert!(popped.is_some());
            assert_eq!(popped.as_ref().unwrap().value, 2);

            // Drop popped -> should destroy in C++!
            drop(popped);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Drop list -> should destroy remaining in C++!
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_interop_cpp_list_rust_unique_objects() {
        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        unsafe {
            let cpp_list = cpp_sll_create_unique_list();
            assert!(cpp_sll_unique_list_is_empty(cpp_list));

            let obj1 = UniquePtr::try_new(SharedUniqueObject::new(1)).unwrap();
            let obj2 = UniquePtr::try_new(SharedUniqueObject::new(2)).unwrap();

            // Set destruction flags
            let raw1 = UniquePtr::as_ptr(&obj1) as *mut SharedUniqueObject;
            (*raw1).destruction_flag = destroyed1.as_ptr() as *mut bool;
            let raw2 = UniquePtr::as_ptr(&obj2) as *mut SharedUniqueObject;
            (*raw2).destruction_flag = destroyed2.as_ptr() as *mut bool;

            // Push to C++ list (transfers ownership)
            cpp_sll_unique_list_push_front(cpp_list, UniquePtr::into_raw(obj1) as *mut c_void);
            cpp_sll_unique_list_push_front(cpp_list, UniquePtr::into_raw(obj2) as *mut c_void);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one from C++
            let popped = cpp_sll_unique_list_pop_front(cpp_list);
            assert!(!popped.is_null());
            assert_eq!(cpp_get_unique_object_value(popped), 2);

            // Convert back to Rust UniquePtr and drop -> should free in Rust!
            let popped_rust = UniquePtr::from_raw(popped as *mut SharedUniqueObject);
            drop(popped_rust);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Destroy C++ list -> should destroy remaining in Rust!
            cpp_sll_destroy_unique_list(cpp_list);
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_interop_rust_list_cpp_ref_objects() {
        let destroyed1 = AtomicBool::new(false);
        let destroyed2 = AtomicBool::new(false);

        unsafe {
            let mut list = SinglyLinkedList::<RefPtr<SharedRefObject>>::new();

            let cpp_raw1 = cpp_create_ref_object(1, destroyed1.as_ptr() as *mut bool);
            let cpp_raw2 = cpp_create_ref_object(2, destroyed2.as_ptr() as *mut bool);

            let obj1 = RefPtr::from_raw(cpp_raw1 as *mut SharedRefObject);
            let obj2 = RefPtr::from_raw(cpp_raw2 as *mut SharedRefObject);

            list.push_front(obj1);
            list.push_front(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one
            let popped = list.pop_front();
            assert!(popped.is_some());
            assert_eq!(popped.as_ref().unwrap().value, 2);

            // Drop popped -> should destroy in C++!
            drop(popped);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Drop list -> should destroy remaining in C++!
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_interop_cpp_list_rust_ref_objects() {
        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        unsafe {
            let cpp_list = cpp_sll_create_ref_list();
            assert!(cpp_sll_ref_list_is_empty(cpp_list));

            let obj1 = SharedRefObject::new_ref_counted(1);
            let obj2 = SharedRefObject::new_ref_counted(2);

            // Set destruction flags
            let raw1 = RefPtr::as_ptr(&obj1) as *mut SharedRefObject;
            (*raw1).destruction_flag = destroyed1.as_ptr() as *mut bool;
            let raw2 = RefPtr::as_ptr(&obj2) as *mut SharedRefObject;
            (*raw2).destruction_flag = destroyed2.as_ptr() as *mut bool;

            // Push to C++ list (transfers ownership)
            cpp_sll_ref_list_push_front(
                cpp_list,
                RefPtr::into_raw(obj1) as *mut SharedRefObject as *mut c_void,
            );
            cpp_sll_ref_list_push_front(
                cpp_list,
                RefPtr::into_raw(obj2) as *mut SharedRefObject as *mut c_void,
            );

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one from C++
            let popped = cpp_sll_ref_list_pop_front(cpp_list);
            assert!(!popped.is_null());
            assert_eq!(cpp_get_ref_object_value(popped), 2);

            // Convert back to Rust RefPtr and drop -> should free in Rust!
            let popped_rust = RefPtr::from_raw(popped as *mut SharedRefObject);
            drop(popped_rust);
            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(destroyed2.load(Ordering::Relaxed));

            // Destroy C++ list -> should destroy remaining in Rust!
            cpp_sll_destroy_ref_list(cpp_list);
        }
        assert!(destroyed1.load(Ordering::Relaxed));
    }
}
