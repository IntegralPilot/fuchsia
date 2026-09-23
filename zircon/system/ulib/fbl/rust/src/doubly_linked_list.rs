// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ptr_traits::{ManagedPtr, PtrTraits};
use crate::sentinel::{is_sentinel_ptr, make_sentinel};
use crate::size_tracker::{NonTrackingSize, SizeTracker, TrackingSize};
use crate::tag::DefaultObjectTag;
use core::cell::UnsafeCell;
use core::pin::Pin;
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};

/// A node in a doubly linked list.
#[repr(C)]
pub struct DoublyLinkedListNode<T> {
    /// The next element in the list.
    pub next: UnsafeCell<*mut T>,
    /// The previous element in the list.
    pub prev: UnsafeCell<*mut T>,
}

impl<T> DoublyLinkedListNode<T> {
    /// Creates a new, unlinked node.
    pub const fn new() -> Self {
        Self {
            next: UnsafeCell::new(core::ptr::null_mut()),
            prev: UnsafeCell::new(core::ptr::null_mut()),
        }
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

    fn get_prev(&self) -> *mut T {
        // SAFETY: `self.prev.get()` is a valid pointer to `self.prev` which is owned by `self`.
        unsafe { *self.prev.get() }
    }

    fn set_prev(&self, prev: *mut T) {
        // SAFETY: `self.prev.get()` is a valid, writable pointer to `self.prev` owned by `self`.
        // UnsafeCell allows interior mutability through a shared reference.
        unsafe {
            *self.prev.get() = prev;
        }
    }
}

impl<T> core::fmt::Debug for DoublyLinkedListNode<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DoublyLinkedListNode").field("in_container", &self.in_container()).finish()
    }
}

impl<T> Default for DoublyLinkedListNode<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for DoublyLinkedListNode<T> {
    fn drop(&mut self) {
        debug_assert!(!self.in_container(), "Object destroyed while still in container");
    }
}

/// Trait that types must implement to be contained in a `DoublyLinkedList`.
pub trait DoublyLinkedListContainable<T, Tag = DefaultObjectTag> {
    /// Returns a reference to the list node.
    fn get_node(&self) -> &DoublyLinkedListNode<T>;
}

/// An intrusive doubly linked list container supporting custom ownership semantics, constant-time
/// operations, and circular-like node layout.
///
/// ### Bookkeeping & Memory Storage
///
/// The bookkeeping storage (`DoublyLinkedListNode`) required to link elements exists directly
/// on the objects themselves. This intrusive pattern eliminates the need for runtime bookkeeping
/// allocations/deallocations when adding or removing members to/from the container.
///
/// The list stores pointers to the objects, not the objects themselves, and is parameterized
/// based on the specific pointer wrapper to be stored (`P`). Supported pointer wrappers are:
///
/// * `*mut T`       : Raw unmanaged pointers.
/// * `UniquePtr<T>` : Unique managed pointers.
/// * `RefPtr<T>`    : Shared managed pointers to reference-counted objects.
///
/// ### Lifecycle Management
///
/// * **Managed Pointers (`UniquePtr`/`RefPtr`)**: The list holds ownership references of elements
///   and follows the rules of the respective smart pointer. Clearing the list or dropping it out of
///   scope automatically releases references, which may destruct the elements if it was their last
///   reference.
///
/// * **Unmanaged Pointers (`*mut T`)**: The list performs no lifecycle management. It is up to the
///   caller to ensure elements outlive the list and are freed correctly. As a safety check, a list
///   of unmanaged pointers will panic/debug-assert if it is dropped with elements still inside.
///
/// ### Ring Layout & Sentinel
///
/// Nodes are arranged in a circular-like ring structure:
///
/// * `head` stores a sentinel value (a pointer to the container itself) when the list is empty.
/// * For non-empty lists, the `next` pointer of the tail node points to the sentinel, and the
///   `prev` pointer of the head node points to the tail node. This allows constant-time O(1) tail
///   lookup and bidirectionality.
/// * Because the sentinel points back to the container's own memory address, the `DoublyLinkedList`
///   container **must be pinned in memory** (typically via `pin_init::stack_pin_init!`) and cannot
///   be safely moved after initialization.
///
/// ### Additional Functionality over SinglyLinkedList
///
/// * O(1) `push_back`, `pop_back`, and `back` operations.
/// * The ability to `insert` (before an element) in addition to `insert_after`.
/// * The ability to `erase` (by reference or iterator) in addition to `erase_next`.
/// * Bidirectional iteration support.
///
/// ### Multiple List Participation
///
/// Objects may exist on multiple lists simultaneously through the use of custom `Tag` classes
/// implementing `DoublyLinkedListContainable` multiple times.
///
/// ---
///
/// ### Example: Simple list of unmanaged raw pointers
///
/// ```rust
/// # use fbl::{DoublyLinkedList, DoublyLinkedListNode, stack_pin_init, pin_init::PinInit};
/// #[derive(fbl::DoublyLinkedListContainable)]
/// struct Foo {
///     value: i32,
///     #[dll_node]
///     node: DoublyLinkedListNode<Foo>,
/// }
///
/// impl Foo {
///     fn new(value: i32) -> Self {
///         Self { value, node: DoublyLinkedListNode::new() }
///     }
/// }
///
/// unsafe {
///     stack_pin_init!(let mut list = DoublyLinkedList::<*mut Foo>::new());
///     let list = list.get_unchecked_mut();
///
///     list.push_front(Box::into_raw(Box::new(Foo::new(1))));
///     list.push_back(Box::into_raw(Box::new(Foo::new(2))));
///
///     for foo in list.iter() {
///         println!("Value: {}", foo.value);
///     }
///
///     while let Some(foo_ptr) = list.pop_front() {
///         let _ = Box::from_raw(foo_ptr);
///     }
/// }
/// ```
///
/// ### Example: Simple list of unique managed pointers
///
/// ```rust
/// use fbl::{DoublyLinkedList, DoublyLinkedListNode, UniquePtr, stack_pin_init};
///
/// #[derive(fbl::DoublyLinkedListContainable, fbl::Recyclable)]
/// struct Foo {
///     value: i32,
///     #[dll_node]
///     node: DoublyLinkedListNode<Foo>,
/// }
///
/// impl Foo {
///     fn new(value: i32) -> Self {
///         Self { value, node: DoublyLinkedListNode::new() }
///     }
/// }
///
/// stack_pin_init!(let mut list = DoublyLinkedList::<UniquePtr<Foo>>::new());
/// let list = list.get_unchecked_mut();
///
/// list.push_front(UniquePtr::try_new(Foo::new(1)).unwrap());
/// list.push_back(UniquePtr::try_new(Foo::new(2)).unwrap());
///
/// for foo in list.iter() {
///     println!("Value: {}", foo.value);
/// }
///
/// // Clearing the list automatically drops unique pointers and reclaims their memory!
/// list.clear();
/// ```
///
/// ### Example: Shared objects in multiple lists simultaneously using Tags
///
/// ```rust
/// use fbl::{DoublyLinkedList, DoublyLinkedListNode, RefPtr, stack_pin_init};
///
/// struct TagA;
/// struct TagB;
///
/// #[fbl::ref_counted]
/// #[derive(fbl::DoublyLinkedListContainable)]
/// struct Foo {
///     value: i32,
///     #[dll_node(TagA)]
///     node_a: DoublyLinkedListNode<Foo>,
///     #[dll_node(TagA)]
///     node_b: DoublyLinkedListNode<Foo>,
/// }
///
/// stack_pin_init!(let mut list_a = DoublyLinkedList::<RefPtr<Foo>, TagA>::new());
/// stack_pin_init!(let mut list_b = DoublyLinkedList::<RefPtr<Foo>, TagB>::new());
/// let list_a = list_a.get_unchecked_mut();
/// let list_b = list_b.get_unchecked_mut();
///
/// let foo = fbl::make_ref_counted!(Foo {
///     value: 42,
///     node_a: DoublyLinkedListNode::new(),
///     node_b: DoublyLinkedListNode::new(),
/// }).unwrap();
///
/// list_a.push_back(foo.clone());
/// list_b.push_back(foo);
/// ```
#[repr(C)]
#[pin_data(PinnedDrop)]
pub struct DoublyLinkedList<P, Tag = DefaultObjectTag, S = NonTrackingSize>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    /// Pointer to the first element of the list.
    ///
    /// # Link Structure
    ///
    /// Nodes in the list are arranged in a circular-like ring structure using a sentinel:
    /// * For a non-empty list, the `next` pointer of each node points to the next element,
    ///   and the `next` pointer of the **tail** node points to the **sentinel** (which is
    ///   a pointer back to this `DoublyLinkedList` container itself).
    /// * The `prev` pointer of each node points to the previous element.
    ///
    /// # Empty List Value
    ///
    /// When the list is empty, this `head` pointer holds the **sentinel** value (a pointer
    /// to the container itself).
    ///
    /// # Tail Pointer Location
    ///
    /// The tail pointer of the list is located in the `prev` field of the **head** node's
    /// list node (`head->prev`), which can be accessed or updated via `self.get_tail()` and
    /// `self.set_tail()`.
    head: *mut P::Target,

    /// The size tracker for the list, supporting either O(N) or O(1) size operations
    /// depending on the `S` parameter (e.g., `NonTrackingSize` or `TrackingSize`).
    size: S,

    /// Marker to ensure the list container is pinned in memory. Pinning is required
    /// because the sentinel pointer points back to the container's own memory address,
    /// meaning the list cannot be safely moved once initialized.
    #[pin]
    _pin: core::marker::PhantomPinned,

    _phantom: core::marker::PhantomData<(P, Tag)>,
}

impl<P, Tag, S> DoublyLinkedList<P, Tag, S>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    /// Creates a new, empty list.
    pub fn new() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(&this in Self {
            head: make_sentinel(this.as_ptr()),
            size: S::INIT,
            _pin: core::marker::PhantomPinned,
            _phantom: core::marker::PhantomData,
        })
    }

    fn get_sentinel(&self) -> *mut P::Target {
        make_sentinel(self as *const Self as *mut Self)
    }

    fn get_tail(&self) -> *mut P::Target {
        if self.is_empty() {
            self.get_sentinel()
        } else {
            // SAFETY: `self.head` is a valid, aligned pointer to an element in the list.
            // Reading `prev` from its node returns a valid pointer (either another node or
            // sentinel).
            unsafe { *(*self.head).get_node().prev.get() }
        }
    }

    /// # Safety
    ///
    /// The caller must ensure that the list is not empty.
    unsafe fn set_tail(&self, tail: *mut P::Target) {
        debug_assert!(!self.is_empty());
        // SAFETY: `self.head` is a valid, aligned pointer to an element in the list.
        // Writing to its `prev` node UnsafeCell is safe because we have exclusive or shared access
        // and interior mutability is allowed.
        unsafe {
            *(*self.head).get_node().prev.get() = tail;
        }
    }

    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid, aligned, and dereferenceable pointer
    /// to an initialized `P::Target` object that is alive for `'a`.
    unsafe fn get_node_ref<'a>(&self, ptr: *mut P::Target) -> &'a DoublyLinkedListNode<P::Target> {
        let _ = self;
        // SAFETY: The caller guarantees `ptr` is valid, aligned, and dereferenceable.
        unsafe { &(*ptr) }.get_node()
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        is_sentinel_ptr(self.head)
    }

    /// Returns a reference to the first element of the list, or `None` if it is empty.
    pub fn front(&self) -> Option<&P::Target> {
        if self.is_empty() { None } else { unsafe { Some(&*self.head) } }
    }

    /// Returns a mutable reference to the first element of the list, or `None` if it is empty.
    pub fn front_mut(&mut self) -> Option<&mut P::Target> {
        if self.is_empty() { None } else { unsafe { Some(&mut *self.head) } }
    }

    /// Returns a reference to the last element of the list, or `None` if it is empty.
    pub fn back(&self) -> Option<&P::Target> {
        let tail = self.get_tail();
        if is_sentinel_ptr(tail) { None } else { unsafe { Some(&*tail) } }
    }

    /// Returns a mutable reference to the last element of the list, or `None` if it is empty.
    pub fn back_mut(&mut self) -> Option<&mut P::Target> {
        let tail = self.get_tail();
        if is_sentinel_ptr(tail) { None } else { unsafe { Some(&mut *tail) } }
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
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the list.
    pub unsafe fn push_front_raw(&mut self, ptr: P) {
        let head = self.head;
        let mut cursor = CursorMut { list: self, current: head };
        // SAFETY: `ptr` is valid and not in container (asserted inside insert_before_raw).
        unsafe {
            cursor.insert_before_raw(ptr);
        }
    }

    /// Pushes an element to the back of the list.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    pub fn push_back(&mut self, ptr: P)
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this list.
        unsafe { self.push_back_raw(ptr) }
    }

    /// Pushes an element to the back of the list.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to an object that is not
    /// currently in any list.
    pub unsafe fn push_back_raw(&mut self, ptr: P) {
        let sentinel = self.get_sentinel();
        let mut cursor = CursorMut { list: self, current: sentinel };
        // SAFETY: `ptr` is valid and not in container.
        unsafe {
            cursor.insert_before_raw(ptr);
        }
    }

    /// Removes and returns the first element of the list, or `None` if it is empty.
    pub fn pop_front(&mut self) -> Option<P> {
        if self.is_empty() {
            return None;
        }
        let head = self.head;
        let mut cursor = CursorMut { list: self, current: head };
        cursor.erase()
    }

    /// Removes and returns the last element of the list, or `None` if it is empty.
    pub fn pop_back(&mut self) -> Option<P> {
        if self.is_empty() {
            return None;
        }
        let tail = self.get_tail();
        let mut cursor = CursorMut { list: self, current: tail };
        cursor.erase()
    }

    /// Removes all elements from the list.
    pub fn clear(&mut self) {
        while let Some(_) = self.pop_front() {}
    }

    /// Erases the given element from the list. Returns the erased element.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `obj` is a valid reference to an object that is
    /// currently in this list instance.
    pub unsafe fn erase(&mut self, obj: &P::Target) -> Option<P> {
        let ptr = obj as *const P::Target as *mut P::Target;
        let node = obj.get_node();

        if !node.in_container() {
            return None;
        }

        let mut cursor = self.cursor_front_mut();
        cursor.current = ptr;
        cursor.erase()
    }

    /// Replaces the given element with `replacement`. Returns the replaced element.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `obj` is a valid reference to an object that is
    /// currently in this list instance, and `replacement` is not in any list.
    pub unsafe fn replace_raw(&mut self, obj: &P::Target, replacement: P) -> Option<P> {
        let ptr = obj as *const P::Target as *mut P::Target;
        let node = obj.get_node();

        if !node.in_container() {
            return None;
        }

        let mut cursor = self.cursor_front_mut();
        cursor.current = ptr;
        // SAFETY: `replacement` is not in any list, and cursor is positioned at a valid element.
        unsafe { cursor.replace_raw(replacement) }
    }

    /// Finds the first element matching the predicate, removes it from the list,
    /// and returns it. Returns `None` if no element matches.
    pub fn erase_if<F>(&mut self, mut f: F) -> Option<P>
    where
        F: FnMut(&P::Target) -> bool,
    {
        let mut cursor = self.cursor_front_mut();
        while let Some(item) = cursor.get() {
            if f(item) {
                return cursor.erase();
            } else {
                cursor.move_next();
            }
        }
        None
    }

    /// Finds the first element that satisfies the predicate.
    pub fn find_if<F>(&self, mut f: F) -> Option<&P::Target>
    where
        F: FnMut(&P::Target) -> bool,
    {
        self.iter().find(|&x| f(x))
    }

    /// Returns a cursor positioned at the front of the list.
    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, P, Tag, S> {
        let head = self.head;
        CursorMut { list: self, current: head }
    }

    /// Returns a cursor positioned at the back (end sentinel) of the list.
    pub fn cursor_back_mut(&mut self) -> CursorMut<'_, P, Tag, S> {
        let sentinel = self.get_sentinel();
        CursorMut { list: self, current: sentinel }
    }

    /// Returns a cursor positioned at the given element.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `obj` is a member of this list.
    /// It is undefined behavior to use the returned cursor if `obj` is not in the list,
    /// or if it is in a different list.
    pub unsafe fn cursor_at(&mut self, obj: &P::Target) -> CursorMut<'_, P, Tag, S> {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        CursorMut { list: self, current: obj as *const P::Target as *mut P::Target }
    }

    /// Splices all elements from `other` onto the end of `self`.
    ///
    /// Upon completion, `other` is left empty.
    ///
    /// This operation is O(1).
    pub fn splice(&mut self, other: &mut DoublyLinkedList<P, Tag, S>) {
        self.cursor_back_mut().splice(other);
    }

    pub fn iter(&self) -> Iterator<'_, P, Tag> {
        Iterator::new(self)
    }

    /// Returns a mutable bidirectional iterator over the elements of the list.
    pub fn iter_mut(&mut self) -> IteratorMut<'_, P, Tag> {
        IteratorMut::new(self)
    }

    /// Returns a unidirectional forward iterator over the elements of the list.
    pub fn forward_iter(&self) -> ForwardIterator<'_, P, Tag> {
        ForwardIterator::new(self.head)
    }

    /// Returns a unidirectional forward mutable iterator over the elements of the list.
    pub fn forward_iter_mut(&mut self) -> ForwardIteratorMut<'_, P, Tag> {
        ForwardIteratorMut::new(self.head)
    }

    /// Returns a unidirectional reverse iterator over the elements of the list.
    pub fn reverse_iter(&self) -> ReverseIterator<'_, P, Tag> {
        ReverseIterator::new(self.get_tail())
    }

    /// Returns a unidirectional reverse mutable iterator over the elements of the list.
    pub fn reverse_iter_mut(&mut self) -> ReverseIteratorMut<'_, P, Tag> {
        ReverseIteratorMut::new(self.get_tail())
    }
}

impl<P, Tag> DoublyLinkedList<P, Tag, TrackingSize>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    /// Returns the number of elements in the list.
    pub fn len(&self) -> usize {
        self.size.get()
    }
}

#[pinned_drop]
impl<P, Tag, S> PinnedDrop for DoublyLinkedList<P, Tag, S>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    fn drop(self: Pin<&mut Self>) {
        if P::IS_MANAGED {
            // SAFETY: We are in drop, so the object won't move anymore.
            let me = unsafe { self.get_unchecked_mut() };
            me.clear();
        } else {
            debug_assert!(self.is_empty(), "List must be empty on destruction");
            if S::IS_TRACKING {
                debug_assert_eq!(self.size.get(), 0, "Size must be zero on destruction");
            }
        }
    }
}

/// A cursor that can be used to iterate and modify a `DoublyLinkedList`.
pub struct CursorMut<'a, P, Tag = DefaultObjectTag, S = NonTrackingSize>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    list: &'a mut DoublyLinkedList<P, Tag, S>,
    current: *mut P::Target,
}

impl<'a, P, Tag, S> CursorMut<'a, P, Tag, S>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
    S: SizeTracker,
{
    pub fn get(&self) -> Option<&P::Target> {
        if is_sentinel_ptr(self.current) { None } else { unsafe { Some(&*self.current) } }
    }

    pub fn get_mut(&mut self) -> Option<&mut P::Target> {
        if is_sentinel_ptr(self.current) { None } else { unsafe { Some(&mut *self.current) } }
    }

    pub fn move_next(&mut self) {
        if !is_sentinel_ptr(self.current) {
            // SAFETY: `self.current` is valid current node (not sentinel).
            let node = unsafe { self.list.get_node_ref(self.current) };
            self.current = node.get_next();
        }
    }

    pub fn move_prev(&mut self) {
        if !is_sentinel_ptr(self.current) {
            // SAFETY: `self.current` is valid current node (not sentinel).
            let node = unsafe { self.list.get_node_ref(self.current) };
            let prev = node.get_prev();
            if self.current == self.list.head {
                self.current = self.list.get_sentinel(); // Move to end.
            } else {
                self.current = prev;
            }
        } else {
            // If we are at end (sentinel), moving prev should take us to tail.
            self.current = self.list.get_tail();
        }
    }

    /// Inserts a new element after the current position.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container, or if the cursor is positioned
    /// at the end sentinel.
    pub fn insert_after(&mut self, ptr: P)
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this list. `self.current` is checked to not be
        // a sentinel.
        unsafe { self.insert_after_raw(ptr) }
    }

    /// Inserts a new element after the current position.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the list.
    pub unsafe fn insert_after_raw(&mut self, ptr: P) {
        assert!(!is_sentinel_ptr(self.current), "Cannot insert after end sentinel");
        let raw = P::into_raw(ptr);
        // SAFETY: `raw` is valid.
        let node = unsafe { self.list.get_node_ref(raw) };
        assert!(!node.in_container());

        // SAFETY: `self.current` is valid current node (not sentinel).
        let current_node = unsafe { self.list.get_node_ref(self.current) };
        let next = current_node.get_next();

        let current_save = self.current;
        self.current = next;
        // SAFETY: `raw` is a single node, and we are inserting it before `next`
        // (which is equivalent to inserting after `current_save`).
        unsafe {
            self.insert_chain_before(raw, raw, 1);
        }
        self.current = current_save;
    }

    /// Replaces the element at the current position with `replacement`. Returns the replaced
    /// element.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    pub fn replace(&mut self, replacement: P) -> Option<P>
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this list.
        unsafe { self.replace_raw(replacement) }
    }

    /// Replaces the element at the current position with `replacement`. Returns the replaced
    /// element.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `replacement` is a valid pointer to a `T` and that the object
    /// outlives the reference from the list.
    pub unsafe fn replace_raw(&mut self, replacement: P) -> Option<P> {
        if is_sentinel_ptr(self.current) {
            return None;
        }
        // SAFETY: `replacement` is not in any list, and we are inserting it before a valid cursor
        // position.
        unsafe {
            self.insert_before_raw(replacement);
        }
        self.erase()
    }

    /// Inserts a new element before the current position.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    pub fn insert_before(&mut self, ptr: P)
    where
        P: ManagedPtr,
    {
        // SAFETY: `P` is a `ManagedPtr`, which guarantees that the pointer is valid and that the
        // object will outlive its reference from this list.
        unsafe { self.insert_before_raw(ptr) }
    }

    /// Inserts a new element before the current position.
    ///
    /// # Panics
    ///
    /// Panics if the object is already in a container.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer to a `T` and that the object outlives
    /// the reference from the list.
    pub unsafe fn insert_before_raw(&mut self, ptr: P) {
        let raw = P::into_raw(ptr);
        // SAFETY: `raw` is valid.
        let node = unsafe { self.list.get_node_ref(raw) };
        assert!(!node.in_container());

        // SAFETY: `raw` is a single node, so it is a valid chain of 1 element.
        unsafe {
            self.insert_chain_before(raw, raw, 1);
        }
    }

    /// Private helper to insert a chain of nodes before the current position.
    ///
    /// # Safety
    ///
    /// The caller must ensure:
    /// - `chain_head` and `chain_tail` are valid pointers to elements.
    /// - They form a valid doubly linked chain.
    /// - The chain is not empty.
    /// - The elements in the chain are NOT currently in any list.
    /// - `count` is the exact number of elements in the chain.
    unsafe fn insert_chain_before(
        &mut self,
        chain_head: *mut P::Target,
        chain_tail: *mut P::Target,
        count: usize,
    ) {
        // SAFETY: `chain_tail` is valid from caller.
        let chain_tail_node = unsafe { self.list.get_node_ref(chain_tail) };
        chain_tail_node.set_next(self.current);

        if self.list.is_empty() {
            // SAFETY: `chain_head` is valid from caller.
            let chain_head_node = unsafe { self.list.get_node_ref(chain_head) };
            chain_head_node.set_prev(chain_tail);
            self.list.head = chain_head;
        } else {
            let prev = if self.current == self.list.head || is_sentinel_ptr(self.current) {
                self.list.get_tail()
            } else {
                // SAFETY: `self.current` is valid.
                let current_node = unsafe { self.list.get_node_ref(self.current) };
                current_node.get_prev()
            };

            // SAFETY: `chain_head` is valid from caller.
            let chain_head_node = unsafe { self.list.get_node_ref(chain_head) };
            chain_head_node.set_prev(prev);

            // 1. Update predecessor's next if we are not inserting at head
            if self.current != self.list.head {
                // SAFETY: `prev` is valid predecessor.
                let prev_node = unsafe { self.list.get_node_ref(prev) };
                prev_node.set_next(chain_head);
            }

            // 2. Update successor's prev if we are not inserting at sentinel
            if !is_sentinel_ptr(self.current) {
                // SAFETY: `self.current` is valid.
                let current_node = unsafe { self.list.get_node_ref(self.current) };
                current_node.set_prev(chain_tail);
            }

            // 3. Update head if we are inserting at head
            if self.current == self.list.head {
                self.list.head = chain_head;
            }

            // 4. Update tail if we are inserting at sentinel
            if is_sentinel_ptr(self.current) {
                // SAFETY: `chain_tail` becomes the new tail.
                unsafe {
                    self.list.set_tail(chain_tail);
                }
            }
        }

        if S::IS_TRACKING {
            self.list.size.set(self.list.size.get() + count);
        }
    }

    /// Splices the elements of `other` into the list at the current cursor position.
    ///
    /// All elements from `other` are moved into `self.list` and inserted immediately
    /// *before* the element currently pointed to by the cursor.
    ///
    /// - If the cursor is positioned at a valid element, `other` is inserted before it.
    /// - If the cursor is positioned at the end sentinel (i.e., `cursor.get()` returns `None`),
    ///   `other` is appended to the end of the list (after the current tail).
    /// - If the list is empty, `other` becomes the new content of the list.
    ///
    /// Upon completion, `other` is left empty.
    ///
    /// This operation is O(1).
    pub fn splice(&mut self, other: &mut DoublyLinkedList<P, Tag, S>) {
        if other.is_empty() {
            return;
        }

        let other_head = other.head;
        let other_tail = other.get_tail();
        let count = if S::IS_TRACKING { other.size.get() } else { 0 };

        // SAFETY: We are moving elements from `other` which is a valid list,
        // so they are valid and not in any other list.
        unsafe {
            self.insert_chain_before(other_head, other_tail, count);
        }

        other.head = other.get_sentinel();
        if S::IS_TRACKING {
            other.size.set(0);
        }
    }

    /// Splits the list immediately after the current cursor position, moving all elements
    /// after the cursor into `dest`, inserted before `dest`'s current position.
    ///
    /// The current list retains all elements up to and including the current element.
    ///
    /// If the cursor is positioned at the tail of the list, no elements are moved.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is positioned at the end sentinel.
    ///
    /// This operation is O(1) for lists with non-tracking size.
    pub fn split_after(&mut self, dest: &mut CursorMut<'_, P, Tag, S>) {
        assert!(!is_sentinel_ptr(self.current), "Cannot split after end sentinel");

        // SAFETY: `self.current` is not sentinel.
        let curr_node = unsafe { self.list.get_node_ref(self.current) };
        let next_ptr = curr_node.get_next();

        // If cursor is at the tail, there are no elements after it.
        if is_sentinel_ptr(next_ptr) {
            return;
        }

        let chain_head = next_ptr;
        let chain_tail = self.list.get_tail();

        let mut count = 0;
        if S::IS_TRACKING {
            let mut p = chain_head;
            let sentinel = self.list.get_sentinel();
            while p != sentinel {
                count += 1;
                // SAFETY: `p` is in the chain.
                p = unsafe { self.list.get_node_ref(p) }.get_next();
            }
            self.list.size.set(self.list.size.get() - count);
        }

        // Update self.list: `self.current` becomes the new tail.
        curr_node.set_next(self.list.get_sentinel());
        unsafe {
            self.list.set_tail(self.current);
        }

        // Insert the extracted chain before dest's current position.
        // SAFETY: The chain [chain_head .. chain_tail] is valid, detached from self.list,
        // and count is exact.
        unsafe {
            dest.insert_chain_before(chain_head, chain_tail, count);
        }
    }

    /// Splits the list immediately before the current cursor position, moving all elements
    /// before the cursor into `dest`, inserted before `dest`'s current position.
    ///
    /// The current list retains all elements from the current position to the end of the list.
    ///
    /// - If the cursor is positioned at the head of the list, no elements are moved.
    /// - If the cursor is positioned at the end sentinel, all elements of the list are moved
    ///   into `dest`, leaving this list empty.
    /// - If the cursor is positioned at an element between head and sentinel, all elements from
    ///   head up to (and excluding) the current element are moved into `dest`.
    ///
    /// Upon completion, the cursor remains positioned at the same element (which is now the new
    /// head of this list, or the sentinel).
    ///
    /// This operation is O(1) for lists with non-tracking size.
    pub fn split_before(&mut self, dest: &mut CursorMut<'_, P, Tag, S>) {
        // Case 1: Cursor is at head. No elements before head.
        if self.current == self.list.head {
            return;
        }

        // Case 2: Cursor is at sentinel. All elements in list are before sentinel.
        if is_sentinel_ptr(self.current) {
            if self.list.is_empty() {
                return;
            }
            let chain_head = self.list.head;
            let chain_tail = self.list.get_tail();
            let count = if S::IS_TRACKING {
                let c = self.list.size.get();
                self.list.size.set(0);
                c
            } else {
                0
            };

            self.list.head = self.list.get_sentinel();

            // SAFETY: The chain [chain_head .. chain_tail] is valid, detached from self.list,
            // and count is exact.
            unsafe {
                dest.insert_chain_before(chain_head, chain_tail, count);
            }
            return;
        }

        // Case 3: Cursor is at an interior element B.
        // Elements before B are [head .. B.prev].
        let b = self.current;
        let b_node = unsafe { self.list.get_node_ref(b) };
        let chain_head = self.list.head;
        let chain_tail = b_node.get_prev();
        let old_tail = self.list.get_tail();

        let mut count = 0;
        if S::IS_TRACKING {
            let mut p = chain_head;
            while p != b {
                count += 1;
                // SAFETY: `p` is in the chain.
                p = unsafe { self.list.get_node_ref(p) }.get_next();
            }
            self.list.size.set(self.list.size.get() - count);
        }

        // Update self.list: B becomes the new head, and its prev points to old_tail.
        self.list.head = b;
        b_node.set_prev(old_tail);

        // Insert the extracted chain before dest's current position.
        // SAFETY: The chain [chain_head .. chain_tail] is valid, detached from self.list,
        // and count is exact.
        unsafe {
            dest.insert_chain_before(chain_head, chain_tail, count);
        }
    }

    pub fn erase(&mut self) -> Option<P> {
        if is_sentinel_ptr(self.current) {
            return None;
        }
        let ptr = self.current;
        // SAFETY: `ptr` is valid current node.
        let node = unsafe { self.list.get_node_ref(ptr) };
        let next = node.get_next();
        let prev = node.get_prev();

        self.list.size.decrement();

        if self.list.head == ptr && is_sentinel_ptr(next) {
            self.list.head = self.list.get_sentinel();
        } else {
            // 1. Update predecessor's next if we are not erasing head
            if self.current != self.list.head {
                // SAFETY: `prev` is valid predecessor.
                let prev_node = unsafe { self.list.get_node_ref(prev) };
                prev_node.set_next(next);
            }

            // 2. Update successor's prev if we are not erasing tail
            if !is_sentinel_ptr(next) {
                // SAFETY: `next` is valid successor.
                let next_node = unsafe { self.list.get_node_ref(next) };
                next_node.set_prev(prev);
            }

            // 3. Update head if we are erasing head
            if self.current == self.list.head {
                self.list.head = next;
            }

            // 4. Update tail if we are erasing tail
            if is_sentinel_ptr(next) {
                // SAFETY: `prev` becomes the new tail.
                unsafe {
                    self.list.set_tail(prev);
                }
            }
        }

        node.set_next(core::ptr::null_mut());
        node.set_prev(core::ptr::null_mut());

        self.current = next;
        // SAFETY: `ptr` was popped, safe to reconstruct.
        Some(unsafe { P::from_raw(ptr) })
    }
}

/// An iterator over the elements of a `DoublyLinkedList`.
pub struct Iterator<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    front: ForwardIterator<'a, P, Tag>,
    back: ReverseIterator<'a, P, Tag>,
}

impl<'a, P, Tag> Iterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn new<S: SizeTracker>(list: &'a DoublyLinkedList<P, Tag, S>) -> Self {
        if list.is_empty() {
            Self {
                front: ForwardIterator::new(crate::make_sentinel_null()),
                back: ReverseIterator::new(crate::make_sentinel_null()),
            }
        } else {
            Self {
                front: ForwardIterator::new(list.head),
                back: ReverseIterator::new(list.get_tail()),
            }
        }
    }
}

impl<'a, P, Tag> core::iter::Iterator for Iterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        let met = self.front.current == self.back.current;
        let item = self.front.next();
        if item.is_some() {
            if met {
                self.front.current = crate::make_sentinel_null();
                self.back.current = crate::make_sentinel_null();
            }
        }
        item
    }
}

impl<'a, P, Tag> core::iter::DoubleEndedIterator for Iterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let met = self.front.current == self.back.current;
        let item = self.back.next();
        if item.is_some() {
            if met {
                self.front.current = crate::make_sentinel_null();
                self.back.current = crate::make_sentinel_null();
            }
        }
        item
    }
}

/// A unidirectional forward iterator over the elements of a `DoublyLinkedList`.
pub struct ForwardIterator<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a (P, Tag)>,
}

impl<'a, P, Tag> ForwardIterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }

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

impl<'a, P, Tag> core::iter::Iterator for ForwardIterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is not a sentinel, so it is a valid, aligned pointer to an
            // element.  The list is guaranteed to be immutable for the lifetime `'a` of the
            // iterator.
            let current = unsafe { &*self.current };
            self.current = current.get_node().get_next();
            Some(current)
        }
    }
}

/// A unidirectional reverse iterator over the elements of a `DoublyLinkedList`.
pub struct ReverseIterator<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a (P, Tag)>,
}

impl<'a, P, Tag> ReverseIterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }

    /// Creates a reverse iterator starting from a specific element.
    ///
    /// # Panics
    ///
    /// Panics if the object is not in a container.
    pub fn from_element(obj: &'a P::Target) -> Self {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Self { current: obj as *const _ as *mut _, _phantom: core::marker::PhantomData }
    }
}

impl<'a, P, Tag> core::iter::Iterator for ReverseIterator<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is not a sentinel, so it is a valid, aligned pointer to an
            // element.  The list is guaranteed to be immutable for the lifetime `'a` of the
            // iterator.
            let current = unsafe { &*self.current };
            let prev = current.get_node().get_prev();

            // SAFETY: `prev` must be a valid pointer because `current` is in the list.  In a
            // circular doubly linked list, prev is never null.
            let prev_node = unsafe { &*prev }.get_node();
            if is_sentinel_ptr(prev_node.get_next()) {
                // We have looped around the head and landed on the tail.
                // Set current to the sentinel to terminate iteration.
                self.current = prev_node.get_next();
            } else {
                self.current = prev;
            }
            Some(current)
        }
    }
}

/// A mutable bidirectional iterator over the elements of a `DoublyLinkedList`.
pub struct IteratorMut<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    front: ForwardIteratorMut<'a, P, Tag>,
    back: ReverseIteratorMut<'a, P, Tag>,
}

impl<'a, P, Tag> IteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn new<S: SizeTracker>(list: &'a mut DoublyLinkedList<P, Tag, S>) -> Self {
        if list.is_empty() {
            Self {
                front: ForwardIteratorMut::new(crate::make_sentinel_null()),
                back: ReverseIteratorMut::new(crate::make_sentinel_null()),
            }
        } else {
            let tail = list.get_tail();
            Self { front: ForwardIteratorMut::new(list.head), back: ReverseIteratorMut::new(tail) }
        }
    }
}

impl<'a, P, Tag> core::iter::Iterator for IteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a mut P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        let met = self.front.current == self.back.current;
        let item = self.front.next();
        if item.is_some() && met {
            self.front.current = crate::make_sentinel_null();
            self.back.current = crate::make_sentinel_null();
        }
        item
    }
}

impl<'a, P, Tag> core::iter::DoubleEndedIterator for IteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let met = self.front.current == self.back.current;
        let item = self.back.next();
        if item.is_some() && met {
            self.front.current = crate::make_sentinel_null();
            self.back.current = crate::make_sentinel_null();
        }
        item
    }
}

/// A unidirectional forward mutable iterator over the elements of a `DoublyLinkedList`.
pub struct ForwardIteratorMut<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a mut (P, Tag)>,
}

impl<'a, P, Tag> ForwardIteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }

    /// Creates an iterator starting from a specific element.
    ///
    /// # Panics
    ///
    /// Panics if the object is not in a container.
    pub fn from_element(obj: &'a mut P::Target) -> Self {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Self { current: obj as *mut _, _phantom: core::marker::PhantomData }
    }
}

impl<'a, P, Tag> core::iter::Iterator for ForwardIteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a mut P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is not a sentinel, so it is a valid, aligned pointer to an
            // element. The list is exclusively borrowed for lifetime `'a`, and each element in the
            // intrusive list is distinct, so yielding `&'a mut P::Target` one-by-one is sound.
            let current = unsafe { &mut *self.current };
            self.current = current.get_node().get_next();
            Some(current)
        }
    }
}

/// A unidirectional reverse mutable iterator over the elements of a `DoublyLinkedList`.
pub struct ReverseIteratorMut<'a, P, Tag = DefaultObjectTag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    current: *mut P::Target,
    _phantom: core::marker::PhantomData<&'a mut (P, Tag)>,
}

impl<'a, P, Tag> ReverseIteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    fn new(current: *mut P::Target) -> Self {
        Self { current, _phantom: core::marker::PhantomData }
    }

    /// Creates a reverse iterator starting from a specific element.
    ///
    /// # Panics
    ///
    /// Panics if the object is not in a container.
    pub fn from_element(obj: &'a mut P::Target) -> Self {
        assert!(obj.get_node().in_container(), "Object must be in a container");
        Self { current: obj as *mut _, _phantom: core::marker::PhantomData }
    }
}

impl<'a, P, Tag> core::iter::Iterator for ReverseIteratorMut<'a, P, Tag>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag>,
{
    type Item = &'a mut P::Target;

    fn next(&mut self) -> Option<Self::Item> {
        if is_sentinel_ptr(self.current) {
            None
        } else {
            // SAFETY: `self.current` is not a sentinel, so it is a valid, aligned pointer to an
            // element. The list is exclusively borrowed for lifetime `'a`, and each element in the
            // intrusive list is distinct, so yielding `&'a mut P::Target` one-by-one is sound.
            let current = unsafe { &mut *self.current };
            let prev = current.get_node().get_prev();

            // SAFETY: `prev` must be a valid pointer because `current` is in the list. In a
            // circular doubly linked list, prev is never null.
            let prev_node = unsafe { &*prev }.get_node();
            if is_sentinel_ptr(prev_node.get_next()) {
                self.current = prev_node.get_next();
            } else {
                self.current = prev;
            }
            Some(current)
        }
    }
}

/// Removes an object from its container without a reference to the container.
///
/// # Safety
///
/// The caller must ensure that `obj` is currently in a valid list instance that does NOT
/// track its size (uses `NonTrackingSize`), and that no other mutable references to that
/// list are active.
pub unsafe fn remove_from_container<T, Tag, P>(obj: &T) -> Option<P>
where
    P: PtrTraits<Target = T>,
    T: DoublyLinkedListContainable<T, Tag>,
{
    let node = obj.get_node();
    if !node.in_container() {
        return None;
    }

    let mut current = obj as *const T as *mut T;
    unsafe {
        while !is_sentinel_ptr(current) {
            current = (*current).get_node().get_next();
        }

        let list_ptr = crate::sentinel::unmake_sentinel::<
            DoublyLinkedList<P, Tag, NonTrackingSize>,
            T,
        >(current);
        let list_ref = &mut *list_ptr;

        list_ref.erase(obj)
    }
}

impl<P, Tag, S> core::fmt::Debug for DoublyLinkedList<P, Tag, S>
where
    P: PtrTraits,
    P::Target: DoublyLinkedListContainable<P::Target, Tag> + core::fmt::Debug,
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
    use core::ffi::c_void;
    use pin_init::stack_pin_init;

    #[derive(crate::DoublyLinkedListContainable, crate::Recyclable)]
    struct TestObject {
        value: i32,
        #[dll_node]
        node: DoublyLinkedListNode<TestObject>,
    }

    impl TestObject {
        fn new(value: i32) -> Self {
            Self { value, node: DoublyLinkedListNode::new() }
        }
    }

    impl TestValue for TestObject {
        fn new(value: i32) -> Self {
            Self::new(value)
        }
    }

    ::zr::static_assert!(
        core::mem::size_of::<DoublyLinkedList<*mut TestObject>>()
            == core::mem::size_of::<*mut TestObject>()
    );
    ::zr::static_assert!(
        core::mem::align_of::<DoublyLinkedList<*mut TestObject>>()
            == core::mem::align_of::<*mut TestObject>()
    );

    ::zr::static_assert!(
        core::mem::size_of::<DoublyLinkedList<*mut TestObject, DefaultObjectTag, TrackingSize>>()
            == 2 * core::mem::size_of::<*mut TestObject>()
    );
    ::zr::static_assert!(
        core::mem::align_of::<DoublyLinkedList<*mut TestObject, DefaultObjectTag, TrackingSize>>()
            == core::mem::align_of::<*mut TestObject>()
    );

    ::zr::static_assert!(
        core::mem::size_of::<ForwardIterator<'_, *mut TestObject>>()
            == core::mem::size_of::<*mut TestObject>()
    );
    ::zr::static_assert!(
        core::mem::align_of::<ForwardIterator<'_, *mut TestObject>>()
            == core::mem::align_of::<*mut TestObject>()
    );

    ::zr::static_assert!(
        core::mem::size_of::<ReverseIterator<'_, *mut TestObject>>()
            == core::mem::size_of::<*mut TestObject>()
    );
    ::zr::static_assert!(
        core::mem::align_of::<ReverseIterator<'_, *mut TestObject>>()
            == core::mem::align_of::<*mut TestObject>()
    );

    #[derive(crate::DoublyLinkedListContainable, crate::Recyclable)]
    struct UniqueTestObject {
        value: i32,
        #[dll_node]
        node: DoublyLinkedListNode<UniqueTestObject>,
    }

    impl UniqueTestObject {
        fn new(value: i32) -> Self {
            Self { value, node: DoublyLinkedListNode::new() }
        }
    }

    impl TestValue for UniqueTestObject {
        fn new(value: i32) -> Self {
            Self::new(value)
        }
    }

    #[fbl::ref_counted]
    #[derive(crate::DoublyLinkedListContainable, crate::Recyclable)]
    #[repr(C)]
    pub struct RefTestObject {
        value: i32,
        #[dll_node]
        node: DoublyLinkedListNode<RefTestObject>,
    }

    impl TestValue for RefTestObject {
        fn new_ref_counted(value: i32) -> RefPtr<Self> {
            crate::make_ref_counted!(RefTestObject {
                value: value,
                node: DoublyLinkedListNode::new()
            })
            .unwrap()
        }
    }

    macro_rules! generate_list_tests {
        ($mod_name:ident, $ptr_type:ty, $factory_type:ty, $get_val:expr, $push:expr) => {
            mod $mod_name {
                use super::*;

                #[test]
                fn test_basic() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    assert!(list.is_empty());

                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(list, obj1);
                    $push(list, obj2);

                    assert!(!list.is_empty());

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 2);
                    assert_eq!(iter.next().unwrap().value, 1);
                    assert!(iter.next().is_none());

                    list.clear();
                    assert!(list.is_empty());
                }

                #[test]
                fn test_double_ended_iterator() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(list, obj1);
                    $push(list, obj2);
                    $push(list, obj3);

                    let mut iter = list.iter();
                    assert_eq!(iter.next().unwrap().value, 3);
                    assert_eq!(iter.next_back().unwrap().value, 1);
                    assert_eq!(iter.next().unwrap().value, 2);
                    assert!(iter.next().is_none());
                    assert!(iter.next_back().is_none());

                    list.clear();
                }

                #[test]
                fn test_explicit_pops() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(list, obj1);
                    $push(list, obj2);

                    let p1 = list.pop_front();
                    assert!(p1.is_some());
                    let p2 = list.pop_front();
                    assert!(p2.is_some());
                    assert!(list.pop_front().is_none());
                }

                #[test]
                fn test_cursor_move_prev() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(list, obj1);
                    $push(list, obj2);
                    $push(list, obj3);

                    let (a, b, c) = {
                        let mut iter = list.iter();
                        (
                            $get_val(iter.next().unwrap()),
                            $get_val(iter.next().unwrap()),
                            $get_val(iter.next().unwrap()),
                        )
                    };

                    let mut cursor = list.cursor_front_mut();
                    assert_eq!($get_val(cursor.get().unwrap()), a);

                    cursor.move_prev();
                    assert!(cursor.get().is_none()); // Sentinel

                    cursor.move_prev();
                    assert_eq!($get_val(cursor.get().unwrap()), c);

                    cursor.move_prev();
                    assert_eq!($get_val(cursor.get().unwrap()), b);

                    cursor.move_prev();
                    assert_eq!($get_val(cursor.get().unwrap()), a);

                    list.clear();
                }

                #[test]
                fn test_cursor_insert_after() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(list, obj1);

                    let mut cursor = list.cursor_front_mut();
                    unsafe {
                        cursor.insert_after_raw(obj3);
                        cursor.insert_after_raw(obj2);
                    }

                    let mut iter = list.iter();
                    assert_eq!($get_val(iter.next().unwrap()), 1);
                    assert_eq!($get_val(iter.next().unwrap()), 2);
                    assert_eq!($get_val(iter.next().unwrap()), 3);
                    assert!(iter.next().is_none());

                    list.clear();
                }

                #[test]
                fn test_pop_back() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(list, obj1);
                    $push(list, obj2);

                    let p1 = list.pop_back();
                    assert!(p1.is_some());
                    let p2 = list.pop_back();
                    assert!(p2.is_some());
                    assert!(list.pop_back().is_none());
                }

                #[test]
                fn test_erase() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(list, obj1);
                    $push(list, obj2);
                    $push(list, obj3);

                    let mut cursor = list.cursor_front_mut();
                    cursor.move_next();
                    let erased = cursor.erase();
                    assert!(erased.is_some());
                    factory.cleanup(erased.unwrap());

                    let mut iter = list.iter();
                    assert!(iter.next().is_some());
                    assert!(iter.next().is_some());
                    assert!(iter.next().is_none());

                    list.clear();
                }

                #[test]
                fn test_erase_if() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(list, obj1);
                    $push(list, obj2);
                    $push(list, obj3);

                    let erased = list.erase_if(|o| o.value == 2);
                    assert!(erased.is_some());
                    assert_eq!($get_val(erased.unwrap().get_ref()), 2);

                    let mut iter = list.iter();
                    assert_eq!($get_val(iter.next().unwrap()), 3);
                    assert_eq!($get_val(iter.next().unwrap()), 1);
                    assert!(iter.next().is_none());

                    list.clear();
                }

                #[test]
                fn test_find_if() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);

                    $push(list, obj1);
                    $push(list, obj2);

                    let found = list.find_if(|o| o.value == 1);
                    assert!(found.is_some());
                    assert_eq!(found.unwrap().value, 1);

                    let found = list.find_if(|o| o.value == 3);
                    assert!(found.is_none());

                    list.clear();
                }

                #[test]
                fn test_complete_reverse_iteration() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list = DoublyLinkedList::<$ptr_type>::new());
                    let list = unsafe { list.get_unchecked_mut() };
                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);

                    $push(list, obj1);
                    $push(list, obj2);
                    $push(list, obj3);

                    let (a, b, c) = {
                        let mut iter = list.iter();
                        (
                            $get_val(iter.next().unwrap()),
                            $get_val(iter.next().unwrap()),
                            $get_val(iter.next().unwrap()),
                        )
                    };

                    let mut iter = list.iter();
                    assert_eq!($get_val(iter.next_back().unwrap()), c);
                    assert_eq!($get_val(iter.next_back().unwrap()), b);
                    assert_eq!($get_val(iter.next_back().unwrap()), a);
                    assert!(iter.next_back().is_none());

                    list.clear();
                }

                #[test]
                fn test_split_after() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list1 = DoublyLinkedList::<$ptr_type>::new());
                    let list1 = unsafe { list1.get_unchecked_mut() };
                    stack_pin_init!(let list2 = DoublyLinkedList::<$ptr_type>::new());
                    let list2 = unsafe { list2.get_unchecked_mut() };

                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);
                    let obj4 = factory.create(4);

                    $push(list1, obj4);
                    $push(list1, obj3);
                    $push(list1, obj2);
                    $push(list1, obj1);

                    let mut cursor = list1.cursor_front_mut();
                    cursor.move_next(); // points to 2

                    let mut dest_cursor = list2.cursor_back_mut();
                    cursor.split_after(&mut dest_cursor);

                    let mut iter1 = list1.iter();
                    assert_eq!($get_val(iter1.next().unwrap()), 1);
                    assert_eq!($get_val(iter1.next().unwrap()), 2);
                    assert!(iter1.next().is_none());

                    let mut iter2 = list2.iter();
                    assert_eq!($get_val(iter2.next().unwrap()), 3);
                    assert_eq!($get_val(iter2.next().unwrap()), 4);
                    assert!(iter2.next().is_none());

                    list1.clear();
                    list2.clear();
                }

                #[test]
                fn test_split_before() {
                    let mut factory = <$factory_type>::new();
                    stack_pin_init!(let list1 = DoublyLinkedList::<$ptr_type>::new());
                    let list1 = unsafe { list1.get_unchecked_mut() };
                    stack_pin_init!(let list2 = DoublyLinkedList::<$ptr_type>::new());
                    let list2 = unsafe { list2.get_unchecked_mut() };

                    let obj1 = factory.create(1);
                    let obj2 = factory.create(2);
                    let obj3 = factory.create(3);
                    let obj4 = factory.create(4);

                    $push(list1, obj4);
                    $push(list1, obj3);
                    $push(list1, obj2);
                    $push(list1, obj1);

                    let mut cursor = list1.cursor_front_mut();
                    cursor.move_next();
                    cursor.move_next(); // points to 3

                    let mut dest_cursor = list2.cursor_back_mut();
                    cursor.split_before(&mut dest_cursor);

                    // list2 should have elements before cursor (1, 2)
                    let mut iter2 = list2.iter();
                    assert_eq!($get_val(iter2.next().unwrap()), 1);
                    assert_eq!($get_val(iter2.next().unwrap()), 2);
                    assert!(iter2.next().is_none());

                    // list1 should have elements from cursor onwards (3, 4)
                    let mut iter1 = list1.iter();
                    assert_eq!($get_val(iter1.next().unwrap()), 3);
                    assert_eq!($get_val(iter1.next().unwrap()), 4);
                    assert!(iter1.next().is_none());

                    list1.clear();
                    list2.clear();
                }
            }
        };
    }

    generate_list_tests!(
        raw_ptr_tests,
        *mut TestObject,
        RawFactory<TestObject>,
        |p: &TestObject| p.value,
        |list: &mut DoublyLinkedList<*mut TestObject>, obj| unsafe {
            list.push_front_raw(obj);
        }
    );

    generate_list_tests!(
        unique_ptr_tests,
        UniquePtr<UniqueTestObject>,
        UniqueFactory<UniqueTestObject>,
        |p: &UniqueTestObject| p.value,
        |list: &mut DoublyLinkedList<UniquePtr<UniqueTestObject>>, obj| list.push_front(obj)
    );

    generate_list_tests!(
        ref_ptr_tests,
        RefPtr<RefTestObject>,
        RefFactory<RefTestObject>,
        |p: &RefTestObject| p.value,
        |list: &mut DoublyLinkedList<RefPtr<RefTestObject>>, obj| list.push_front(obj)
    );

    #[test]
    fn test_tracking_size() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        assert_eq!(list.len(), 0);
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        assert_eq!(list.len(), 1);
        list.push_front(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        assert_eq!(list.len(), 2);
        list.pop_front();
        assert_eq!(list.len(), 1);
        list.clear();
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_insert_before() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        let mut cursor = list.cursor_front_mut();
        cursor.insert_before(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        assert_eq!(list.len(), 1);
        assert_eq!(list.front().unwrap().value, 1);

        let mut cursor = list.cursor_front_mut();
        cursor.insert_before(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        assert_eq!(list.len(), 2);
        assert_eq!(list.front().unwrap().value, 2);

        let mut cursor = list.cursor_front_mut();
        cursor.move_next(); // point to obj1
        cursor.insert_before(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        assert_eq!(list.len(), 3);

        let mut cursor = list.cursor_front_mut();
        while cursor.get().unwrap().value != 1 {
            cursor.move_next();
        }
        cursor.move_next(); // point to sentinel
        cursor.insert_before(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());
        assert_eq!(list.len(), 4);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 2);
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 4);
        assert!(iter.next().is_none());

        list.clear();
    }

    #[test]
    fn test_front_mut_and_back_mut() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        assert!(list.front_mut().is_none());
        assert!(list.back_mut().is_none());

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(10)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(20)).unwrap());

        assert_eq!(list.front_mut().unwrap().value, 10);
        assert_eq!(list.back_mut().unwrap().value, 20);

        list.front_mut().unwrap().value = 100;
        list.back_mut().unwrap().value = 200;

        assert_eq!(list.front().unwrap().value, 100);
        assert_eq!(list.back().unwrap().value, 200);

        list.clear();
    }

    #[test]
    fn test_cursor_front_and_back_mut() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        assert!(list.cursor_front_mut().get().is_none());
        assert!(list.cursor_back_mut().get().is_none());

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(10)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(20)).unwrap());

        let mut front_cursor = list.cursor_front_mut();
        assert_eq!(front_cursor.get().unwrap().value, 10);
        front_cursor.move_next();
        assert_eq!(front_cursor.get().unwrap().value, 20);

        let mut back_cursor = list.cursor_back_mut();
        assert!(back_cursor.get().is_none());
        back_cursor.move_prev();
        assert_eq!(back_cursor.get().unwrap().value, 20);

        let mut back_cursor = list.cursor_back_mut();
        back_cursor.insert_before(UniquePtr::try_new(UniqueTestObject::new(30)).unwrap());
        assert_eq!(list.len(), 3);
        assert_eq!(list.back().unwrap().value, 30);

        list.clear();
    }

    #[test]
    fn test_splice_middle() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());

        let mut cursor = list1.cursor_front_mut();
        cursor.move_next(); // point to obj2

        cursor.splice(list2);

        assert!(list2.is_empty());
        assert_eq!(list1.len(), 4);

        let mut iter = list1.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 4);
        assert_eq!(iter.next().unwrap().value, 2);
        assert!(iter.next().is_none());

        list1.clear();
    }

    #[test]
    fn test_splice_head() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());

        let mut cursor = list1.cursor_front_mut();

        cursor.splice(list2);

        assert!(list2.is_empty());
        assert_eq!(list1.len(), 4);

        let mut iter = list1.iter();
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 4);
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 2);
        assert!(iter.next().is_none());

        list1.clear();
    }

    #[test]
    fn test_splice_tail() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());

        let mut cursor = list1.cursor_front_mut();
        cursor.move_next();
        cursor.move_next(); // point to sentinel

        cursor.splice(list2);

        assert!(list2.is_empty());
        assert_eq!(list1.len(), 4);

        let mut iter = list1.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 2);
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 4);
        assert!(iter.next().is_none());

        list1.clear();
    }

    #[test]
    fn test_splice_empty() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());

        let mut cursor = list1.cursor_front_mut();
        cursor.splice(list2);

        assert!(list2.is_empty());
        assert_eq!(list1.len(), 2);

        let mut iter = list1.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 2);
        assert!(iter.next().is_none());

        list1.clear();
    }

    #[test]
    fn test_splice_non_tracking() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, NonTrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, NonTrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());

        let mut cursor = list1.cursor_front_mut();
        cursor.move_next(); // point to obj2

        cursor.splice(list2);

        assert!(list2.is_empty());

        let mut iter = list1.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 4);
        assert_eq!(iter.next().unwrap().value, 2);
        assert!(iter.next().is_none());

        list1.clear();
    }

    #[test]
    fn test_list_splice() {
        // Test splicing non-empty other into non-empty self.
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());

        list1.splice(list2);

        assert!(list2.is_empty());
        assert_eq!(list2.len(), 0);
        assert_eq!(list1.len(), 4);

        let mut iter = list1.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 2);
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 4);
        assert!(iter.next().is_none());

        // Test splicing into empty self.
        stack_pin_init!(let list3 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list3 = unsafe { list3.get_unchecked_mut() };

        list3.splice(list1);

        assert!(list1.is_empty());
        assert_eq!(list1.len(), 0);
        assert_eq!(list3.len(), 4);

        let mut iter = list3.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 2);
        assert_eq!(iter.next().unwrap().value, 3);
        assert_eq!(iter.next().unwrap().value, 4);
        assert!(iter.next().is_none());

        // Test splicing empty list into non-empty self (no-op).
        list3.splice(list1);
        assert_eq!(list3.len(), 4);
        assert!(list1.is_empty());

        list3.clear();
    }

    #[test]
    fn test_split_after_at_tail() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());

        let mut cursor = list1.cursor_front_mut();
        cursor.move_next(); // points to 2 (tail)

        let mut dest_cursor = list2.cursor_back_mut();
        cursor.split_after(&mut dest_cursor);

        assert!(list2.is_empty());
        assert_eq!(list2.len(), 0);
        assert_eq!(list1.len(), 2);

        list1.clear();
    }

    #[test]
    fn test_split_after_at_head() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());

        let mut cursor = list1.cursor_front_mut(); // points to 1 (head)

        let mut dest_cursor = list2.cursor_back_mut();
        cursor.split_after(&mut dest_cursor);

        assert_eq!(list1.len(), 1);
        assert_eq!(list2.len(), 2);

        let mut iter1 = list1.iter();
        assert_eq!(iter1.next().unwrap().value, 1);
        assert!(iter1.next().is_none());

        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 2);
        assert_eq!(iter2.next().unwrap().value, 3);
        assert!(iter2.next().is_none());

        list1.clear();
        list2.clear();
    }

    #[test]
    fn test_split_before_at_head() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());

        let mut cursor = list1.cursor_front_mut(); // points to 1 (head)

        let mut dest_cursor = list2.cursor_back_mut();
        cursor.split_before(&mut dest_cursor);

        assert!(list2.is_empty());
        assert_eq!(list2.len(), 0);
        assert_eq!(list1.len(), 2);

        list1.clear();
    }

    #[test]
    fn test_split_before_at_tail() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());

        let mut cursor = list1.cursor_front_mut();
        cursor.move_next();
        cursor.move_next(); // points to 3 (tail)

        let mut dest_cursor = list2.cursor_back_mut();
        cursor.split_before(&mut dest_cursor);

        assert_eq!(list2.len(), 2);
        assert_eq!(list1.len(), 1);

        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 1);
        assert_eq!(iter2.next().unwrap().value, 2);
        assert!(iter2.next().is_none());

        let mut iter1 = list1.iter();
        assert_eq!(iter1.next().unwrap().value, 3);
        assert!(iter1.next().is_none());

        list1.clear();
        list2.clear();
    }

    #[test]
    fn test_split_before_at_sentinel() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());

        let mut cursor = list1.cursor_back_mut(); // points to sentinel

        let mut dest_cursor = list2.cursor_back_mut();
        cursor.split_before(&mut dest_cursor);

        assert!(list1.is_empty());
        assert_eq!(list1.len(), 0);
        assert_eq!(list2.len(), 2);

        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 1);
        assert_eq!(iter2.next().unwrap().value, 2);
        assert!(iter2.next().is_none());

        list2.clear();
    }

    #[test]
    fn test_split_into_middle_of_dest_list() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());
        list1.push_back(UniquePtr::try_new(UniqueTestObject::new(4)).unwrap());

        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(10)).unwrap());
        list2.push_back(UniquePtr::try_new(UniqueTestObject::new(20)).unwrap());

        // Split after 2 from list1, and insert before 20 in list2
        let mut cursor1 = list1.cursor_front_mut();
        cursor1.move_next(); // points to 2

        let mut cursor2 = list2.cursor_front_mut();
        cursor2.move_next(); // points to 20

        cursor1.split_after(&mut cursor2);

        // list1 should retain [1, 2]
        assert_eq!(list1.len(), 2);
        let mut iter1 = list1.iter();
        assert_eq!(iter1.next().unwrap().value, 1);
        assert_eq!(iter1.next().unwrap().value, 2);
        assert!(iter1.next().is_none());

        // list2 should now be [10, 3, 4, 20]
        assert_eq!(list2.len(), 4);
        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 10);
        assert_eq!(iter2.next().unwrap().value, 3);
        assert_eq!(iter2.next().unwrap().value, 4);
        assert_eq!(iter2.next().unwrap().value, 20);
        assert!(iter2.next().is_none());

        list1.clear();
        list2.clear();
    }

    #[test]
    fn test_list_split_methods() {
        let mut obj1 = TestObject::new(1);
        let mut obj2 = TestObject::new(2);
        let mut obj3 = TestObject::new(3);

        stack_pin_init!(let list1 = DoublyLinkedList::<*mut TestObject>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 = DoublyLinkedList::<*mut TestObject>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        unsafe {
            list1.push_back_raw(&mut obj1);
            list1.push_back_raw(&mut obj2);
            list1.push_back_raw(&mut obj3);

            // Split after obj1
            let mut cursor1 = list1.cursor_at(&obj1);
            let mut cursor2 = list2.cursor_back_mut();
            cursor1.split_after(&mut cursor2);

            assert_eq!(list1.iter().count(), 1);
            assert_eq!(list2.iter().count(), 2);

            let mut iter1 = list1.iter();
            assert_eq!(iter1.next().unwrap().value, 1);
            assert!(iter1.next().is_none());

            let mut iter2 = list2.iter();
            assert_eq!(iter2.next().unwrap().value, 2);
            assert_eq!(iter2.next().unwrap().value, 3);
            assert!(iter2.next().is_none());

            // Splice back
            list1.splice(list2);
            assert_eq!(list1.iter().count(), 3);

            // Split before obj3
            let mut cursor1 = list1.cursor_at(&obj3);
            let mut cursor2 = list2.cursor_back_mut();
            cursor1.split_before(&mut cursor2);

            assert_eq!(list2.iter().count(), 2);
            assert_eq!(list1.iter().count(), 1);

            let mut iter2 = list2.iter();
            assert_eq!(iter2.next().unwrap().value, 1);
            assert_eq!(iter2.next().unwrap().value, 2);
            assert!(iter2.next().is_none());

            let mut iter1 = list1.iter();
            assert_eq!(iter1.next().unwrap().value, 3);
            assert!(iter1.next().is_none());

            list1.clear();
            list2.clear();
        }
    }

    #[test]
    fn test_cursor_get() {
        stack_pin_init!(let list = DoublyLinkedList::<UniquePtr<UniqueTestObject>>::new());
        let list = unsafe { list.get_unchecked_mut() };

        let cursor = list.cursor_front_mut();
        assert!(cursor.get().is_none());

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(42)).unwrap());

        let mut cursor = list.cursor_front_mut();
        assert!(cursor.get().is_some());
        assert_eq!(cursor.get().unwrap().value, 42);

        cursor.move_next();
        assert!(cursor.get().is_none());

        list.clear();
    }

    #[test]
    fn test_pop_back() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());

        assert_eq!(list.len(), 2);
        let popped = list.pop_back();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().value, 2);
        assert_eq!(list.len(), 1);

        let popped = list.pop_back();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().value, 1);
        assert_eq!(list.len(), 0);

        assert!(list.pop_back().is_none());
    }

    #[test]
    fn test_erase() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());

        assert_eq!(list.len(), 3);

        // 1. Erase middle (obj2)
        let mut cursor = list.cursor_front_mut();
        cursor.move_next(); // point to obj2
        let erased = cursor.erase();
        assert!(erased.is_some());
        assert_eq!(erased.unwrap().value, 2);
        assert_eq!(list.len(), 2);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        // 2. Erase head (obj1)
        let mut cursor = list.cursor_front_mut();
        let erased = cursor.erase(); // current is head (obj1)
        assert!(erased.is_some());
        assert_eq!(erased.unwrap().value, 1);
        assert_eq!(list.len(), 1);
        assert_eq!(list.front().unwrap().value, 3); // obj3 is now head!

        // 3. Erase last element (obj3)
        let mut cursor = list.cursor_front_mut();
        let erased = cursor.erase(); // current is head/tail (obj3)
        assert!(erased.is_some());
        assert_eq!(erased.unwrap().value, 3);
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());

        list.clear();
    }

    #[test]
    fn test_erase_if() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());

        let erased = list.erase_if(|obj| obj.value % 2 == 0);
        assert!(erased.is_some());
        assert_eq!(erased.unwrap().value, 2);

        assert_eq!(list.len(), 2);
        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        list.clear();
    }

    #[test]
    fn test_erase_by_reference() {
        stack_pin_init!(let list =
            DoublyLinkedList::<*mut TestObject, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };
        let mut obj1 = TestObject::new(1);
        let mut obj2 = TestObject::new(2);
        let mut obj3 = TestObject::new(3);

        unsafe {
            list.push_back_raw(&mut obj1);
            list.push_back_raw(&mut obj2);
            list.push_back_raw(&mut obj3);
        }

        assert_eq!(list.len(), 3);

        // Erase obj2 directly (safe because it's unmanaged raw pointer to stack)
        let erased = unsafe { list.erase(&obj2) };
        assert!(erased.is_some());
        assert_eq!(unsafe { &*erased.unwrap() }.value, 2);
        assert_eq!(list.len(), 2);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        list.clear();
    }

    #[test]
    fn test_remove_from_container() {
        stack_pin_init!(let list = DoublyLinkedList::<*mut TestObject>::new());
        let list = unsafe { list.get_unchecked_mut() };
        let mut obj1 = TestObject::new(1);
        let mut obj2 = TestObject::new(2);
        let mut obj3 = TestObject::new(3);

        // Case 1: Attempt to remove an element not in any container
        let removed = unsafe {
            remove_from_container::<TestObject, DefaultObjectTag, *mut TestObject>(&obj1)
        };
        assert!(removed.is_none());

        unsafe {
            list.push_back_raw(&mut obj1);
            list.push_back_raw(&mut obj2);
            list.push_back_raw(&mut obj3);
        }

        // Case 2: Remove middle (obj2)
        let removed = unsafe {
            remove_from_container::<TestObject, DefaultObjectTag, *mut TestObject>(&obj2)
        };
        assert!(removed.is_some());
        assert_eq!(unsafe { &*removed.unwrap() }.value, 2);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        // Case 3: Remove head (obj1)
        let removed = unsafe {
            remove_from_container::<TestObject, DefaultObjectTag, *mut TestObject>(&obj1)
        };
        assert!(removed.is_some());
        assert_eq!(unsafe { &*removed.unwrap() }.value, 1);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        // Case 4: Remove last remaining element (obj3 -> leaves empty!)
        let removed = unsafe {
            remove_from_container::<TestObject, DefaultObjectTag, *mut TestObject>(&obj3)
        };
        assert!(removed.is_some());
        assert_eq!(unsafe { &*removed.unwrap() }.value, 3);
        assert!(list.is_empty());

        list.clear();
    }

    #[test]
    fn test_replace() {
        stack_pin_init!(let list =
            DoublyLinkedList::<*mut TestObject, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };
        let mut obj1 = TestObject::new(1);
        let mut obj2 = TestObject::new(2);
        let mut obj3 = TestObject::new(3);

        unsafe {
            list.push_back_raw(&mut obj1);
            list.push_back_raw(&mut obj2);
        }

        assert_eq!(list.len(), 2);

        let old = unsafe { list.replace_raw(&obj2, &mut obj3) };
        assert!(old.is_some());
        assert_eq!(unsafe { &*old.unwrap() }.value, 2);
        assert_eq!(list.len(), 2);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        list.clear();
    }

    #[test]
    fn test_cursor_replace() {
        stack_pin_init!(let list = DoublyLinkedList::<UniquePtr<UniqueTestObject>>::new());
        let list = unsafe { list.get_unchecked_mut() };

        let obj1 = UniquePtr::try_new(UniqueTestObject::new(1)).unwrap();
        let obj2 = UniquePtr::try_new(UniqueTestObject::new(2)).unwrap();
        let obj3 = UniquePtr::try_new(UniqueTestObject::new(3)).unwrap();

        list.push_back(obj1);
        list.push_back(obj2);

        let mut cursor = list.cursor_front_mut();
        cursor.move_next(); // point to obj2

        let old = cursor.replace(obj3);
        assert!(old.is_some());
        assert_eq!(old.unwrap().value, 2);

        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        list.clear();
    }

    struct Tag2;

    #[fbl::ref_counted]
    #[derive(crate::DoublyLinkedListContainable, crate::Recyclable)]
    #[repr(C)]
    struct MultiListObject {
        value: i32,
        #[dll_node]
        node1: DoublyLinkedListNode<MultiListObject>,
        #[dll_node(tag = Tag2)]
        node2: DoublyLinkedListNode<MultiListObject>,
    }

    #[test]
    fn test_multiple_containers() {
        stack_pin_init!(let list1 =
            DoublyLinkedList::<RefPtr<MultiListObject>, DefaultObjectTag>::new());
        let list1 = unsafe { list1.get_unchecked_mut() };
        stack_pin_init!(let list2 = DoublyLinkedList::<RefPtr<MultiListObject>, Tag2>::new());
        let list2 = unsafe { list2.get_unchecked_mut() };

        let obj1 = fbl::make_ref_counted!(MultiListObject {
            value: 1,
            node1: DoublyLinkedListNode::new(),
            node2: DoublyLinkedListNode::new(),
        })
        .unwrap();

        let obj2 = fbl::make_ref_counted!(MultiListObject {
            value: 2,
            node1: DoublyLinkedListNode::new(),
            node2: DoublyLinkedListNode::new(),
        })
        .unwrap();

        list1.push_back(obj1.clone());
        list1.push_back(obj2.clone());

        list2.push_back(obj2); // obj2 is now in both lists!

        let mut iter1 = list1.iter();
        assert_eq!(iter1.next().unwrap().value, 1);
        assert_eq!(iter1.next().unwrap().value, 2);
        assert!(iter1.next().is_none());

        let mut iter2 = list2.iter();
        assert_eq!(iter2.next().unwrap().value, 2);
        assert!(iter2.next().is_none());

        list1.clear();
        list2.clear();
    }

    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};

    #[derive(crate::DoublyLinkedListContainable, crate::Recyclable)]
    struct LifecycleObject {
        destroyed: Arc<AtomicBool>,
        #[dll_node]
        node: DoublyLinkedListNode<LifecycleObject>,
    }

    impl LifecycleObject {
        fn new(destroyed: Arc<AtomicBool>) -> Self {
            Self { destroyed, node: DoublyLinkedListNode::new() }
        }
    }

    impl Drop for LifecycleObject {
        fn drop(&mut self) {
            self.destroyed.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_lifecycle_on_drop() {
        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        {
            stack_pin_init!(let list = DoublyLinkedList::<UniquePtr<LifecycleObject>>::new());
            let list = unsafe { list.get_unchecked_mut() };

            let obj1 = UniquePtr::try_new(LifecycleObject::new(destroyed1.clone())).unwrap();
            let obj2 = UniquePtr::try_new(LifecycleObject::new(destroyed2.clone())).unwrap();

            list.push_back(obj1);
            list.push_back(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));
        } // list drops here

        assert!(destroyed1.load(Ordering::Relaxed));
        assert!(destroyed2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_sized_managed_list() {
        stack_pin_init!(let list =
            DoublyLinkedList::<UniquePtr<UniqueTestObject>, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };

        assert_eq!(list.len(), 0);

        let obj1 = UniquePtr::try_new(UniqueTestObject::new(1)).unwrap();
        let obj2 = UniquePtr::try_new(UniqueTestObject::new(2)).unwrap();

        list.push_back(obj1);
        assert_eq!(list.len(), 1);

        list.push_back(obj2);
        assert_eq!(list.len(), 2);

        let popped = list.pop_front();
        assert!(popped.is_some());
        assert_eq!(list.len(), 1);

        list.clear();
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_unidirectional_iterators() {
        stack_pin_init!(let list = DoublyLinkedList::<UniquePtr<UniqueTestObject>>::new());
        let list = unsafe { list.get_unchecked_mut() };

        list.push_back(UniquePtr::try_new(UniqueTestObject::new(1)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(2)).unwrap());
        list.push_back(UniquePtr::try_new(UniqueTestObject::new(3)).unwrap());

        // 1. Test ForwardIterator from beginning
        let mut f_iter = list.forward_iter();
        assert_eq!(f_iter.next().unwrap().value, 1);
        let obj2_ref = f_iter.next().unwrap();
        assert_eq!(obj2_ref.value, 2);
        assert_eq!(f_iter.next().unwrap().value, 3);
        assert!(f_iter.next().is_none());

        // 2. Test ForwardIterator from element in the middle (obj2_ref)
        let mut f_element_iter =
            ForwardIterator::<UniquePtr<UniqueTestObject>>::from_element(obj2_ref);
        assert_eq!(f_element_iter.next().unwrap().value, 2);
        assert_eq!(f_element_iter.next().unwrap().value, 3);
        assert!(f_element_iter.next().is_none());

        // 3. Test ReverseIterator from end
        let mut r_iter = list.reverse_iter();
        assert_eq!(r_iter.next().unwrap().value, 3);
        let obj2_ref_r = r_iter.next().unwrap();
        assert_eq!(obj2_ref_r.value, 2);
        assert_eq!(r_iter.next().unwrap().value, 1);
        assert!(r_iter.next().is_none());

        // 4. Test ReverseIterator from element in the middle (obj2_ref_r)
        let mut r_element_iter =
            ReverseIterator::<UniquePtr<UniqueTestObject>>::from_element(obj2_ref_r);
        assert_eq!(r_element_iter.next().unwrap().value, 2);
        assert_eq!(r_element_iter.next().unwrap().value, 1);
        assert!(r_element_iter.next().is_none());

        list.clear();
    }

    #[test]
    fn test_cursor_at() {
        stack_pin_init!(let list =
            DoublyLinkedList::<*mut TestObject, DefaultObjectTag, TrackingSize>::new());
        let list = unsafe { list.get_unchecked_mut() };
        let mut obj1 = TestObject::new(1);
        let mut obj2 = TestObject::new(2);
        let mut obj3 = TestObject::new(3);

        unsafe {
            list.push_back_raw(&mut obj1);
            list.push_back_raw(&mut obj2);
            list.push_back_raw(&mut obj3);
        }

        // Create a cursor at the second element (obj2).
        // SAFETY: `obj2` is a member of `list`.
        let mut cursor = unsafe { list.cursor_at(&obj2) };
        assert_eq!(cursor.get().unwrap().value, 2);

        // Verify we can move next.
        cursor.move_next();
        assert_eq!(cursor.get().unwrap().value, 3);

        // Verify we can move prev from the original position.
        // SAFETY: `obj2` is a member of `list`.
        let mut cursor = unsafe { list.cursor_at(&obj2) };
        cursor.move_prev();
        assert_eq!(cursor.get().unwrap().value, 1);

        // Verify we can erase via the cursor created at the element.
        // SAFETY: `obj2` is a member of `list`.
        let mut cursor = unsafe { list.cursor_at(&obj2) };
        let erased = cursor.erase().unwrap();
        assert_eq!(unsafe { &*erased }.value, 2);

        // Verify list contents after erase.
        let mut iter = list.iter();
        assert_eq!(iter.next().unwrap().value, 1);
        assert_eq!(iter.next().unwrap().value, 3);
        assert!(iter.next().is_none());

        list.clear();
    }

    // FFI Declarations
    unsafe extern "C" {
        // UniqueList Helpers
        fn cpp_create_unique_list() -> *mut c_void;
        fn cpp_destroy_unique_list(list: *mut c_void);
        fn cpp_unique_list_push_back(list: *mut c_void, item: *mut c_void);
        fn cpp_unique_list_pop_front(list: *mut c_void) -> *mut c_void;
        fn cpp_unique_list_is_empty(list: *mut c_void) -> bool;

        // RefList Helpers
        fn cpp_create_ref_list() -> *mut c_void;
        fn cpp_destroy_ref_list(list: *mut c_void);
        fn cpp_ref_list_push_back(list: *mut c_void, item: *mut c_void);
        fn cpp_ref_list_pop_front(list: *mut c_void) -> *mut c_void;
        fn cpp_ref_list_is_empty(list: *mut c_void) -> bool;

        // SharedUniqueObject Helpers
        fn cpp_create_unique_object(value: i32, destruction_flag: *mut bool) -> *mut c_void;
        fn cpp_get_unique_object_value(obj: *mut c_void) -> i32;

        // SharedRefObject Helpers
        fn cpp_create_ref_object(value: i32, destruction_flag: *mut bool) -> *mut c_void;
        fn cpp_get_ref_object_value(obj: *mut c_void) -> i32;
    }

    #[test]
    fn test_interop_rust_list_cpp_unique_objects() {
        let destroyed1 = AtomicBool::new(false);
        let destroyed2 = AtomicBool::new(false);

        unsafe {
            stack_pin_init!(let list = DoublyLinkedList::<UniquePtr<SharedUniqueObject>>::new());
            let list = list.get_unchecked_mut();

            let cpp_raw1 = cpp_create_unique_object(1, destroyed1.as_ptr() as *mut bool);
            let cpp_raw2 = cpp_create_unique_object(2, destroyed2.as_ptr() as *mut bool);

            let obj1 = UniquePtr::from_raw(cpp_raw1 as *mut SharedUniqueObject);
            let obj2 = UniquePtr::from_raw(cpp_raw2 as *mut SharedUniqueObject);

            list.push_back(obj1);
            list.push_back(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one
            let popped = list.pop_front();
            assert!(popped.is_some());
            assert_eq!(popped.as_ref().unwrap().value, 1);

            // Drop popped -> should destroy in C++!
            drop(popped);
            assert!(destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Drop list -> should destroy remaining in C++!
        }
        assert!(destroyed2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_interop_cpp_list_rust_unique_objects() {
        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        unsafe {
            let cpp_list = cpp_create_unique_list();
            assert!(cpp_unique_list_is_empty(cpp_list));

            let obj1 = UniquePtr::try_new(SharedUniqueObject::new(1)).unwrap();
            let obj2 = UniquePtr::try_new(SharedUniqueObject::new(2)).unwrap();

            // Set destruction flags
            let raw1 = UniquePtr::as_ptr(&obj1) as *mut SharedUniqueObject;
            (*raw1).destruction_flag = destroyed1.as_ptr() as *mut bool;
            let raw2 = UniquePtr::as_ptr(&obj2) as *mut SharedUniqueObject;
            (*raw2).destruction_flag = destroyed2.as_ptr() as *mut bool;

            // Push to C++ list (transfers ownership)
            cpp_unique_list_push_back(cpp_list, UniquePtr::into_raw(obj1) as *mut c_void);
            cpp_unique_list_push_back(cpp_list, UniquePtr::into_raw(obj2) as *mut c_void);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one from C++
            let popped = cpp_unique_list_pop_front(cpp_list);
            assert!(!popped.is_null());
            assert_eq!(cpp_get_unique_object_value(popped), 1);

            // Convert back to Rust UniquePtr and drop -> should free in Rust!
            let popped_rust = UniquePtr::from_raw(popped as *mut SharedUniqueObject);
            drop(popped_rust);
            assert!(destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Destroy C++ list -> should destroy remaining in Rust!
            cpp_destroy_unique_list(cpp_list);
        }
        assert!(destroyed2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_interop_rust_list_cpp_ref_objects() {
        let destroyed1 = AtomicBool::new(false);
        let destroyed2 = AtomicBool::new(false);

        unsafe {
            stack_pin_init!(let list = DoublyLinkedList::<RefPtr<SharedRefObject>>::new());
            let list = list.get_unchecked_mut();

            let cpp_raw1 = cpp_create_ref_object(1, destroyed1.as_ptr() as *mut bool);
            let cpp_raw2 = cpp_create_ref_object(2, destroyed2.as_ptr() as *mut bool);

            let obj1 = RefPtr::from_raw(cpp_raw1 as *mut SharedRefObject);
            let obj2 = RefPtr::from_raw(cpp_raw2 as *mut SharedRefObject);

            list.push_back(obj1);
            list.push_back(obj2);

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one
            let popped = list.pop_front();
            assert!(popped.is_some());
            assert_eq!(popped.as_ref().unwrap().value, 1);

            // Drop popped -> should destroy in C++!
            drop(popped);
            assert!(destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Drop list -> should destroy remaining in C++!
        }
        assert!(destroyed2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_interop_cpp_list_rust_ref_objects() {
        let destroyed1 = Arc::new(AtomicBool::new(false));
        let destroyed2 = Arc::new(AtomicBool::new(false));

        unsafe {
            let cpp_list = cpp_create_ref_list();
            assert!(cpp_ref_list_is_empty(cpp_list));

            let obj1 = SharedRefObject::new_ref_counted(1);
            let obj2 = SharedRefObject::new_ref_counted(2);

            // Set destruction flags
            let raw1 = RefPtr::as_ptr(&obj1) as *mut SharedRefObject;
            (*raw1).destruction_flag = destroyed1.as_ptr() as *mut bool;
            let raw2 = RefPtr::as_ptr(&obj2) as *mut SharedRefObject;
            (*raw2).destruction_flag = destroyed2.as_ptr() as *mut bool;

            // Push to C++ list (transfers ownership)
            cpp_ref_list_push_back(
                cpp_list,
                RefPtr::into_raw(obj1) as *mut SharedRefObject as *mut c_void,
            );
            cpp_ref_list_push_back(
                cpp_list,
                RefPtr::into_raw(obj2) as *mut SharedRefObject as *mut c_void,
            );

            assert!(!destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Pop one from C++
            let popped = cpp_ref_list_pop_front(cpp_list);
            assert!(!popped.is_null());
            assert_eq!(cpp_get_ref_object_value(popped), 1);

            // Convert back to Rust RefPtr and drop -> should free in Rust!
            let popped_rust = RefPtr::from_raw(popped as *mut SharedRefObject);
            drop(popped_rust);
            assert!(destroyed1.load(Ordering::Relaxed));
            assert!(!destroyed2.load(Ordering::Relaxed));

            // Destroy C++ list -> should destroy remaining in Rust!
            cpp_destroy_ref_list(cpp_list);
        }
        assert!(destroyed2.load(Ordering::Relaxed));
    }
}
