// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Slab-style allocator for Fuchsia/Zircon objects.
//!
//! `SlabAllocator` is a utility class implementing a slab-style memory allocator
//! for a given object type `T`. It can dispense:
//! - **Managed pointer types** (`UniquePtr<T>`, `RefPtr<T>`): Automatically returned to
//!   the allocator when they are dropped.
//! - **Unmanaged pointer types** (`*mut T`): Must be manually returned to the allocator.
//!
//! # Allocator Flavors
//!
//! In C++, this allocator supported three flavors: `INSTANCED`, `STATIC`, and `MANUAL_DELETE`.
//! In Rust, these flavors are mapped as follows:
//!
//! 1. **Instanced Allocators**:
//!    - Multiple instances of the allocator can coexist, each with independent quotas.
//!    - Objects carry a pointer back to their originating allocator to find their way home on drop.
//!    - Required trait: `InstancedSlabAllocated`. Helper macro: `impl_instanced_slab_allocatable!`.
//!    - Allocated via `new_unique` and `new_ref`.
//!
//! 2. **Static Allocators**:
//!    - A single process-wide global allocator for a given type.
//!    - Objects carry no storage overhead and locate their allocator via trait definitions.
//!    - Required trait: `StaticSlabAllocated`. Helper macro: `impl_static_slab_allocatable!`.
//!    - Allocated via standard constructors (e.g. `UniquePtr::try_new`, `make_ref_counted!`).
//!
//! 3. **Manual Delete (Unmanaged) Allocations**:
//!    - Objects pay no storage overhead for tracking their allocator origin.
//!    - Memory is allocated as raw pointers and must be explicitly returned using `delete` or
//!      `return_to_free_list`.
//!    - Allocated via `alloc_raw`.
//!
//! # Memory Limits and Allocation Behavior
//!
//! Slabs of size `SLAB_SIZE` (default 16KB) are allocated from the heap using `kalloc::alloc`.
//! These slabs are carved into properly aligned regions just large enough to hold an instance
//! of `T` (or a free-list link node).
//!
//! Allocation operations:
//! 1. Reuse nodes from the internal free list.
//! 2. If the free list is empty, carve out memory from the currently active slab.
//! 3. If the active slab is full and `slab_count < max_slabs`, allocate a new slab.
//! 4. If all limits are reached, return `Err(AllocError)`.
//!
//! Allocation is O(1) in the steady state, and O(kalloc::alloc) when a new slab is needed.
//! Setting the slab limit to 1 and passing `alloc_initial = true` during `try_new` ensures
//! O(1) performance for all allocations.
//!
//! # Thread Safety
//!
//! The allocator uses a generic lock parameter `L` (implementing `RawLock`) to synchronize access,
//! which defaults to `RawMutex`.

use crate::recyclable::Recyclable;
use crate::ref_counted::HasRefCount;
use crate::ref_ptr::RefPtr;
use crate::singly_linked_list::{SinglyLinkedList, SinglyLinkedListNode};
use crate::unique_ptr::UniquePtr;
use core::alloc::Layout;
use core::cmp::max;
use core::marker::PhantomData;
use core::mem::{align_of, size_of};
use core::pin::Pin;
use core::ptr::{NonNull, drop_in_place, write};
use kalloc::AllocError;
pub use ksync::RawLock;
use ksync::{KCell, KMutex, RawMutex, guarded, lock};
use pin_init::{pin_data, pin_init, pinned_drop};

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::RawMutex {}
}

/// Helper trait to restrict SlabAllocator lock to RawMutex.
/// TODO(https://fxbug.dev/541903019): Support generic mutex types.
pub trait IsRawMutex: sealed::Sealed {}
impl IsRawMutex for RawMutex {}

/// The default slab size in bytes (16KB).
pub const DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE: usize = 16384;

#[derive(crate::SinglyLinkedListContainable)]
#[repr(C)]
struct SlabHeader {
    #[sll_node]
    node: SinglyLinkedListNode<SlabHeader>,
    bytes_used: usize,
}

impl SlabHeader {
    fn new(bytes_used: usize) -> Self {
        assert!(
            bytes_used >= size_of::<Self>(),
            "bytes_used ({}) must be at least as large as SlabHeader ({})",
            bytes_used,
            size_of::<Self>()
        );
        Self { node: SinglyLinkedListNode::new(), bytes_used }
    }

    fn allocate(&mut self, alloc_size: usize, slab_size: usize) -> Option<NonNull<u8>> {
        if self.bytes_used + alloc_size > slab_size {
            return None;
        }
        let self_ptr = self as *mut SlabHeader as *mut u8;
        // SAFETY: `self.bytes_used` is guaranteed to be within `slab_size`.
        let ret = unsafe { self_ptr.add(self.bytes_used) };
        self.bytes_used += alloc_size;
        // SAFETY: `self_ptr` is derived from `&mut self` which is non-null.
        // `self.bytes_used` is positive, so `ret` is also non-null.
        Some(unsafe { NonNull::new_unchecked(ret) })
    }
}

#[derive(crate::SinglyLinkedListContainable)]
#[repr(C)]
struct FreeListEntry {
    #[sll_node]
    node: SinglyLinkedListNode<FreeListEntry>,
}

/// A slab-style allocator for a given object type `T`.
///
/// # Generics
///
/// * `T`: The type of object allocated by this allocator.
/// * `L`: The synchronization primitive (defaults to `RawMutex`).
/// * `SLAB_SIZE`: The size of each memory slab in bytes (defaults to 16KB).
/// * `TRACK_OBJECT_COUNT`: Enable allocation tracking (e.g. `obj_count`, `max_obj_count`).
///
/// # Examples
///
/// ## Instanced Allocation with `UniquePtr`
///
/// ```rust
/// use fbl::{
///     SlabAllocator, RawMutex, impl_instanced_slab_allocatable, UniquePtr,
///     DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE,
/// };
/// use core::cell::Cell;
///
/// use fbl::SlabOrigin;
/// struct MyObject {
///     value: i32,
///     // Required field for tracking origin
///     slab_origin: SlabOrigin<MyObject, RawMutex, DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE>,
/// }
///
/// impl_instanced_slab_allocatable!(MyObject, RawMutex, DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE);
///
/// fn example() {
///     let allocator = SlabAllocator::<
///         MyObject, RawMutex, DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE
///     >::try_new(4, true, RawMutex::INIT).unwrap();
///     let mut list = std::collections::VecDeque::new();
///
///     for i in 0..10 {
///         let obj = allocator.new_unique(MyObject {
///             value: i,
///             slab_origin: SlabOrigin::new(),
///         }).unwrap();
///         list.push_front(obj);
///     }
///     // Memory is automatically returned to the allocator when elements are dropped.
/// }
/// ```
///
/// ## Static Allocation with `UniquePtr`
///
/// ```rust
/// use fbl::{
///     SlabAllocator, RawMutex, impl_static_slab_allocatable, UniquePtr,
///     DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE
/// };
///
/// struct MyObject {
///     value: i32,
/// }
///
/// static MY_ALLOCATOR: SlabAllocator<MyObject, RawMutex, DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE> =
///     SlabAllocator::const_new(64, RawMutex::INIT);
///
/// impl_static_slab_allocatable!(
///     MyObject, RawMutex, DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE, MY_ALLOCATOR);
///
/// fn example() {
///     let obj = UniquePtr::try_new(MyObject { value: 42 }).unwrap();
///     // obj will automatically recycle back to MY_ALLOCATOR.
/// }
/// ```
#[guarded]
#[pin_data(PinnedDrop)]
pub struct SlabAllocator<
    T,
    L: RawLock + IsRawMutex = RawMutex,
    const SLAB_SIZE: usize = DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE,
    const TRACK_OBJECT_COUNT: bool = false,
> {
    #[mutex]
    // TODO(https://fxbug.dev/541903019): Currently generic mutex types are not supported and so
    // this is forced to be RawMutex. The IsRawMutex trait bound above ensures that no other type is
    // attempted to be used.
    mu: KMutex<RawMutex>,

    #[guarded_by(mu)]
    free_list: SinglyLinkedList<NonNull<FreeListEntry>>,
    #[guarded_by(mu)]
    slab_list: SinglyLinkedList<NonNull<SlabHeader>>,
    #[guarded_by(mu)]
    slab_count: usize,
    // Note: `obj_count` and `max_obj_count` are not used if `TRACK_OBJECT_COUNT` is false,
    // but they are always declared for simplicity of the struct definition.
    #[guarded_by(mu)]
    obj_count: usize,
    #[guarded_by(mu)]
    max_obj_count: usize,

    max_slabs: usize,
    _phantom: PhantomData<(T, L)>,
}

unsafe impl<
    T,
    L: RawLock + IsRawMutex + Sync,
    const SLAB_SIZE: usize,
    const TRACK_OBJECT_COUNT: bool,
> Sync for SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
}
unsafe impl<
    T,
    L: RawLock + IsRawMutex + Send,
    const SLAB_SIZE: usize,
    const TRACK_OBJECT_COUNT: bool,
> Send for SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
}

/// A helper type to store the originating slab allocator for instanced allocations.
///
/// This type wraps `Option<NonNull<SlabAllocator<...>>>` and provides safe `Send` and `Sync`
/// implementations, allowing the containing object to be shared across threads.
pub struct SlabOrigin<
    T,
    L: RawLock + IsRawMutex = RawMutex,
    const SLAB_SIZE: usize = DEFAULT_SLAB_ALLOCATOR_SLAB_SIZE,
    const TRACK_OBJECT_COUNT: bool = false,
> {
    origin: Option<NonNull<SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>>>,
}

impl<T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>
    SlabOrigin<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
    /// Creates a new, uninitialized `SlabOrigin`.
    pub const fn new() -> Self {
        Self { origin: None }
    }

    /// Sets the origin allocator.
    pub fn set(&mut self, origin: NonNull<SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>>) {
        self.origin = Some(origin);
    }

    /// Gets the origin allocator.
    pub fn get(&self) -> Option<NonNull<SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>>> {
        self.origin
    }
}

impl<T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool> Default
    for SlabOrigin<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: SlabOrigin only holds a pointer to SlabAllocator which is Send/Sync if L is Send/Sync.
unsafe impl<
    T,
    L: RawLock + IsRawMutex + Sync,
    const SLAB_SIZE: usize,
    const TRACK_OBJECT_COUNT: bool,
> Sync for SlabOrigin<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
}
unsafe impl<
    T,
    L: RawLock + IsRawMutex + Send,
    const SLAB_SIZE: usize,
    const TRACK_OBJECT_COUNT: bool,
> Send for SlabOrigin<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
}

/// Trait implemented by types that can be allocated from an instanced slab allocator.
///
/// Implementing this trait allows `UniquePtr` and `RefPtr` to automatically return
/// their memory to the originating allocator on drop.
pub trait InstancedSlabAllocated<
    L: RawLock + IsRawMutex,
    const SLAB_SIZE: usize,
    const TRACK_OBJECT_COUNT: bool = false,
>: Sized
{
    /// Returns the address of the originating slab allocator.
    fn slab_origin(&self)
    -> Option<NonNull<SlabAllocator<Self, L, SLAB_SIZE, TRACK_OBJECT_COUNT>>>;
    /// Sets the originating slab allocator.
    fn set_slab_origin(
        &mut self,
        origin: NonNull<SlabAllocator<Self, L, SLAB_SIZE, TRACK_OBJECT_COUNT>>,
    );
}

/// Trait implemented by types that can be allocated from a static slab allocator.
///
/// Implementing this trait allows `UniquePtr` and `RefPtr` to automatically return
/// their memory to the global static allocator on drop.
pub trait StaticSlabAllocated<
    L: RawLock + IsRawMutex,
    const SLAB_SIZE: usize,
    const TRACK_OBJECT_COUNT: bool = false,
>: Sized
{
    /// Returns a static reference to the global slab allocator.
    fn get_allocator() -> &'static SlabAllocator<Self, L, SLAB_SIZE, TRACK_OBJECT_COUNT>;
}

/// Macro to implement `InstancedSlabAllocated` and `Recyclable` for a struct.
///
/// This assumes the struct contains a field `slab_origin` of type
/// `SlabOrigin<Self, Lock, SLAB_SIZE, TRACK_OBJECT_COUNT>`.
#[macro_export]
macro_rules! impl_instanced_slab_allocatable {
    ($ty:ty, $lock:ty, $slab_size:expr) => {
        $crate::impl_instanced_slab_allocatable!($ty, $lock, $slab_size, false);
    };
    ($ty:ty, $lock:ty, $slab_size:expr, $track_obj_count:expr) => {
        impl $crate::InstancedSlabAllocated<$lock, $slab_size, $track_obj_count> for $ty {
            fn slab_origin(
                &self,
            ) -> Option<
                ::core::ptr::NonNull<
                    $crate::SlabAllocator<Self, $lock, $slab_size, $track_obj_count>,
                >,
            > {
                self.slab_origin.get()
            }

            fn set_slab_origin(
                &mut self,
                origin: ::core::ptr::NonNull<
                    $crate::SlabAllocator<Self, $lock, $slab_size, $track_obj_count>,
                >,
            ) {
                self.slab_origin.set(origin);
            }
        }

        unsafe impl $crate::Recyclable for $ty {
            unsafe fn recycle(ptr: ::core::ptr::NonNull<Self>) {
                // SAFETY: The pointer is guaranteed to be non-null and to point to a valid,
                // initialized instance of `Self` allocated from this slab allocator.
                let origin = unsafe { ptr.as_ref().slab_origin() };
                if let Some(origin) = origin {
                    // SAFETY: The allocator instance must outlive the allocated objects.
                    // Dropping in-place before returning the raw memory prevents use-after-free
                    // and ensures proper cleanup of fields.
                    unsafe {
                        ::core::ptr::drop_in_place(ptr.as_ptr());
                        origin.as_ref().return_to_free_list(ptr);
                    }
                }
            }
        }
    };
}

/// Macro to implement `StaticSlabAllocated` and `Recyclable` for a struct.
#[macro_export]
macro_rules! impl_static_slab_allocatable {
    ($ty:ty, $lock:ty, $slab_size:expr, $allocator:expr) => {
        $crate::impl_static_slab_allocatable!($ty, $lock, $slab_size, $allocator, false);
    };
    ($ty:ty, $lock:ty, $slab_size:expr, $allocator:expr, $track_obj_count:expr) => {
        impl $crate::StaticSlabAllocated<$lock, $slab_size, $track_obj_count> for $ty {
            fn get_allocator()
            -> &'static $crate::SlabAllocator<Self, $lock, $slab_size, $track_obj_count> {
                &$allocator
            }
        }

        unsafe impl $crate::Recyclable for $ty {
            fn allocate(value: Self) -> Result<::core::ptr::NonNull<Self>, ::kalloc::AllocError> {
                let allocator = <Self as $crate::StaticSlabAllocated<
                    $lock,
                    $slab_size,
                    $track_obj_count,
                >>::get_allocator();
                let ptr = allocator.alloc_raw()?;
                // SAFETY: `ptr` points to valid, uninitialized memory allocated from the slab.
                // Writing to it initializes the memory.
                unsafe {
                    ::core::ptr::write(ptr.as_ptr(), value);
                }
                Ok(ptr)
            }

            unsafe fn recycle(ptr: ::core::ptr::NonNull<Self>) {
                let allocator = <Self as $crate::StaticSlabAllocated<
                    $lock,
                    $slab_size,
                    $track_obj_count,
                >>::get_allocator();
                // SAFETY: The global static allocator is guaranteed to live forever (static
                // lifetime).  Dropping the object in-place before returning memory prevents
                // use-after-free.
                unsafe {
                    ::core::ptr::drop_in_place(ptr.as_ptr());
                    allocator.return_to_free_list(ptr);
                }
            }
        }
    };
}

impl<T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>
    SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
    pub const ALLOC_ALIGN: usize = if align_of::<FreeListEntry>() > align_of::<T>() {
        align_of::<FreeListEntry>()
    } else {
        align_of::<T>()
    };
    pub const ALLOC_SIZE: usize = {
        let raw_size = if size_of::<FreeListEntry>() > size_of::<T>() {
            size_of::<FreeListEntry>()
        } else {
            size_of::<T>()
        };
        match Layout::from_size_align(raw_size, Self::ALLOC_ALIGN) {
            Ok(layout) => layout.pad_to_align().size(),
            Err(_) => panic!("invalid layout"),
        }
    };
    pub const STORAGE_OFFSET: usize =
        match Layout::from_size_align(size_of::<SlabHeader>(), Self::ALLOC_ALIGN) {
            Ok(layout) => layout.pad_to_align().size(),
            Err(_) => panic!("invalid layout"),
        };

    /// The number of objects of type `T` that can fit in a single slab.
    pub const ALLOCS_PER_SLAB: usize = (SLAB_SIZE - Self::STORAGE_OFFSET) / Self::ALLOC_SIZE;

    const _ASSERT: () = {
        assert!(
            Self::ALLOC_SIZE % Self::ALLOC_ALIGN == 0,
            "Allocation size must be a multiple of alignment"
        );
        assert!(
            size_of::<SlabHeader>() < SLAB_SIZE,
            "SLAB_SIZE too small to hold slab bookkeeping"
        );
        assert!(
            SLAB_SIZE >= Self::STORAGE_OFFSET + Self::ALLOC_SIZE,
            "SLAB_SIZE too small to hold even 1 allocation"
        );
    };

    /// Pre-allocates the first slab.
    ///
    /// This can be used to guarantee O(1) execution times for all future allocations
    /// if `max_slabs` is at least 1.
    pub fn preallocate(&self) -> Result<(), AllocError> {
        let ptr = self.alloc_raw()?;
        // SAFETY: `ptr` was just allocated and is valid.
        unsafe {
            self.return_to_free_list(ptr);
        }
        if TRACK_OBJECT_COUNT {
            lock!(let guard = self.lock_mu());
            let fields = guard.fields_mut();
            *fields.obj_count = 0;
            *fields.max_obj_count = 0;
        }
        Ok(())
    }

    /// Creates a new `SlabAllocator` that can be initialized in const contexts.
    ///
    /// Note: Slabs are not pre-allocated during const-construction.
    pub const fn const_new(max_slabs: usize, lock: RawMutex) -> Self {
        let _ = Self::_ASSERT;
        Self {
            mu: KMutex::new(lock),
            free_list: KCell::new(SinglyLinkedList::new()),
            slab_list: KCell::new(SinglyLinkedList::new()),
            slab_count: KCell::new(0),
            obj_count: KCell::new(0),
            max_obj_count: KCell::new(0),
            max_slabs,
            _phantom: PhantomData,
        }
    }

    /// Creates a new `PinInit` initializer for `SlabAllocator` for dynamic initialization.
    pub fn init(max_slabs: usize) -> impl pin_init::PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            mu <- KMutex::init(),
            free_list: SinglyLinkedList::new().into(),
            slab_list: SinglyLinkedList::new().into(),
            slab_count: 0.into(),
            obj_count: 0.into(),
            max_obj_count: 0.into(),
            max_slabs,
            _phantom: PhantomData,
        })
    }

    #[inline(always)]
    fn slab_layout() -> Layout {
        let alloc_align = Self::ALLOC_ALIGN;
        let slab_align = max(align_of::<SlabHeader>(), alloc_align);
        // SAFETY: SLAB_SIZE is non-zero (checked by static assert), and slab_align
        // is a valid power of 2 (align_of is always a power of 2, and max of two powers
        // of 2 is also a power of 2).
        unsafe { Layout::from_size_align_unchecked(SLAB_SIZE, slab_align) }
    }

    fn alloc_slab() -> Result<NonNull<SlabHeader>, AllocError> {
        let layout = Self::slab_layout();
        // SAFETY: `layout` is guaranteed to have a non-zero size (`SLAB_SIZE`) and valid alignment.
        let slab_mem = unsafe { kalloc::alloc(layout).ok_or(AllocError)? };
        let slab_ptr = slab_mem.cast::<SlabHeader>();

        // SAFETY: `slab_ptr` is validly allocated with appropriate size and alignment.
        unsafe {
            write(slab_ptr.as_ptr(), SlabHeader::new(Self::STORAGE_OFFSET));
        }
        Ok(slab_ptr)
    }

    /// # Safety
    ///
    /// `slab_ptr` must have been allocated by this allocator and not yet deallocated.
    unsafe fn dealloc_slab(slab_ptr: NonNull<SlabHeader>) {
        let layout = Self::slab_layout();
        // SAFETY: The caller guarantees `slab_ptr` is valid.
        unsafe {
            drop_in_place(slab_ptr.as_ptr());
            kalloc::dealloc(slab_ptr.cast::<u8>().as_ptr(), layout);
        }
    }

    /// Allocates raw, uninitialized memory for a single object of type `T`.
    pub fn alloc_raw(&self) -> Result<NonNull<T>, AllocError> {
        lock!(let guard = self.lock_mu());
        let mut fields = guard.fields_mut();

        // 1. Try free list
        if let Some(entry_ptr) = fields.free_list.pop_front() {
            fields.record_allocation();
            return Ok(entry_ptr.cast::<T>());
        }

        // 2. Try active slab
        if let Some(active_slab) = fields.slab_list.front_mut() {
            if let Some(mem) = active_slab.allocate(Self::ALLOC_SIZE, SLAB_SIZE) {
                let ptr = mem.cast::<T>();
                fields.record_allocation();
                return Ok(ptr);
            }
        }

        // 3. Try allocate new slab
        if *fields.slab_count < self.max_slabs {
            let mut slab_ptr = Self::alloc_slab()?;
            *fields.slab_count += 1;
            // SAFETY: `slab_ptr` is newly initialized.
            unsafe {
                fields.slab_list.push_front_raw(slab_ptr);
            }

            // SAFETY: Allocate from this new slab.
            let active_slab = unsafe { slab_ptr.as_mut() };
            let mem = active_slab.allocate(Self::ALLOC_SIZE, SLAB_SIZE).unwrap();
            let ptr = mem.cast::<T>();
            fields.record_allocation();
            return Ok(ptr);
        }

        Err(AllocError)
    }

    /// Returns raw memory to the free list.
    ///
    /// # Safety
    ///
    /// `ptr` must have been previously allocated from this allocator, and must not have
    /// been returned already.
    pub unsafe fn return_to_free_list(&self, ptr: NonNull<T>) {
        let entry_ptr = ptr.cast::<FreeListEntry>();
        // SAFETY: The memory block is large and aligned enough to hold a `FreeListEntry`.
        unsafe {
            write(entry_ptr.as_ptr(), FreeListEntry { node: SinglyLinkedListNode::new() });
        }

        lock!(let guard = self.lock_mu());
        let mut fields = guard.fields_mut();
        // SAFETY: `entry_ptr` is a valid NonNull pointer.
        unsafe {
            fields.free_list.push_front_raw(entry_ptr);
        }
        fields.record_deallocation();
    }

    /// Constructs an object in a `UniquePtr` using memory allocated from this instanced allocator.
    pub fn new_unique(&self, value: T) -> Result<UniquePtr<T>, AllocError>
    where
        T: Recyclable + InstancedSlabAllocated<L, SLAB_SIZE, TRACK_OBJECT_COUNT>,
    {
        let ptr = self.alloc_raw()?;
        // SAFETY: `ptr` points to valid, uninitialized memory suitable for `T`.
        unsafe {
            write(ptr.as_ptr(), value);
            (&mut *ptr.as_ptr()).set_slab_origin(NonNull::from(self));
            Ok(UniquePtr::from_raw(ptr.as_ptr()))
        }
    }

    /// Constructs a ref-counted object in a `RefPtr` using memory allocated from this instanced
    /// allocator.
    pub fn new_ref(&self, value: T) -> Result<RefPtr<T>, AllocError>
    where
        T: HasRefCount + Recyclable + InstancedSlabAllocated<L, SLAB_SIZE, TRACK_OBJECT_COUNT>,
    {
        let ptr = self.alloc_raw()?;
        // SAFETY: `ptr` is valid and uninitialized.
        unsafe {
            write(ptr.as_ptr(), value);
            (&mut *ptr.as_ptr()).set_slab_origin(NonNull::from(self));
            (*ptr.as_ptr()).ref_count().adopt();
            Ok(RefPtr::from_raw(ptr.as_ptr()))
        }
    }

    /// Destructs and deallocates an unmanaged object.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid object allocated from this allocator.
    pub unsafe fn delete(&self, ptr: NonNull<T>) {
        // SAFETY: The caller guarantees that `ptr` points to a valid object.
        unsafe {
            drop_in_place(ptr.as_ptr());
            self.return_to_free_list(ptr);
        }
    }

    /// Returns the number of currently allocated objects.
    pub fn obj_count(&self) -> usize {
        const {
            assert!(TRACK_OBJECT_COUNT, "Error accessing obj_count: Object counter not enabled");
        }
        lock!(let guard = self.lock_mu());
        *guard.fields().obj_count
    }

    /// Returns the maximum number of objects allocated simultaneously over the life of the
    /// allocator.
    pub fn max_obj_count(&self) -> usize {
        const {
            assert!(
                TRACK_OBJECT_COUNT,
                "Error accessing max_obj_count: Object counter not enabled"
            );
        }
        lock!(let guard = self.lock_mu());
        *guard.fields().max_obj_count
    }

    /// Returns the number of slabs allocated.
    pub fn slab_count(&self) -> usize {
        lock!(let guard = self.lock_mu());
        *guard.fields().slab_count
    }

    /// Returns the maximum number of slabs this allocator is allowed to allocate.
    pub fn max_slabs(&self) -> usize {
        self.max_slabs
    }

    /// Resets the maximum object count tracker to the current object count.
    pub fn reset_max_obj_count(&self) {
        const {
            assert!(
                TRACK_OBJECT_COUNT,
                "Error performing reset_max_obj_count: Object counter not enabled"
            );
        }
        lock!(let guard = self.lock_mu());
        let fields = guard.fields_mut();
        *fields.max_obj_count = *fields.obj_count;
    }
}

impl<'b, T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>
    SlabAllocatorMuFieldsMut<'b, T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
    #[inline(always)]
    fn record_allocation(&mut self) {
        if TRACK_OBJECT_COUNT {
            *self.obj_count += 1;
            *self.max_obj_count = max(*self.max_obj_count, *self.obj_count);
        }
    }

    #[inline(always)]
    fn record_deallocation(&mut self) {
        if TRACK_OBJECT_COUNT {
            *self.obj_count -= 1;
        }
    }
}

#[pinned_drop]
impl<T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool> PinnedDrop
    for SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>
{
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: We can safely get the mutable reference to the fields inside drop.
        let me = unsafe { self.get_unchecked_mut() };
        let free_list = me.free_list.get_inner_mut();
        let slab_list = me.slab_list.get_inner_mut();
        // Verify there are no outstanding allocations in debug builds
        #[cfg(debug_assertions)]
        {
            let obj_count = me.obj_count.get_inner_mut();
            if TRACK_OBJECT_COUNT {
                debug_assert_eq!(
                    *obj_count, 0,
                    "SlabAllocator destroyed with outstanding allocations!"
                );
            } else {
                // If tracking is disabled, perform a slow counting check to verify leak-free drop
                let free_list_size = free_list.iter().count();
                let mut allocated_count = 0;
                for slab in slab_list.iter() {
                    let bytes_used = slab.bytes_used - Self::STORAGE_OFFSET;
                    allocated_count += bytes_used / Self::ALLOC_SIZE;
                }
                debug_assert_eq!(
                    free_list_size, allocated_count,
                    "SlabAllocator destroyed with outstanding allocations!"
                );
            }
        }

        // Clear free list first so it doesn't assert on drop.
        // Note: free list entries are raw pointers to slab memory, so popping them is a no-op.
        free_list.clear();

        while let Some(slab_ptr) = slab_list.pop_front() {
            // Drop the SlabHeader and deallocate slab memory
            // SAFETY: `slab_ptr` is a valid pointer to `SlabHeader` from `slab_list`.
            unsafe {
                Self::dealloc_slab(slab_ptr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RefCounted;
    use alloc::vec::Vec;
    use core::cmp::min;
    use core::ptr::write;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use lock_api::RawMutex as _;
    use pin_init::stack_pin_init;
    extern crate alloc;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ConstructType {
        Default,
        LvalueRef,
        RvalueRef,
        LThenRRef,
    }

    trait TestConstructors {
        fn new_default() -> Self;
        fn new_lvalue(val: usize) -> Self;
        fn new_rvalue(val: usize) -> Self;
        fn new_l_then_r(a: usize, b: usize) -> Self;
        fn ctype(&self) -> ConstructType;
    }

    // Helper trait to allow compile-time conditional tracking assertions inside generic test
    // runners.
    trait MaybeTracked {
        fn maybe_obj_count(&self) -> usize;
        fn maybe_max_obj_count(&self) -> usize;
        fn maybe_reset_max_obj_count(&self);
    }

    impl<T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize> MaybeTracked
        for SlabAllocator<T, L, SLAB_SIZE, true>
    {
        fn maybe_obj_count(&self) -> usize {
            self.obj_count()
        }
        fn maybe_max_obj_count(&self) -> usize {
            self.max_obj_count()
        }
        fn maybe_reset_max_obj_count(&self) {
            self.reset_max_obj_count()
        }
    }

    impl<T, L: RawLock + IsRawMutex, const SLAB_SIZE: usize> MaybeTracked
        for SlabAllocator<T, L, SLAB_SIZE, false>
    {
        fn maybe_obj_count(&self) -> usize {
            0
        }
        fn maybe_max_obj_count(&self) -> usize {
            0
        }
        fn maybe_reset_max_obj_count(&self) {}
    }

    // Generic test runners with passed counter reference to avoid cross-test pollution
    fn run_instanced_unique_test<T, L, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>(
        allocated_obj_count: &'static AtomicUsize,
        max_slabs: usize,
        test_allocs: usize,
    ) where
        T: Recyclable
            + InstancedSlabAllocated<L, SLAB_SIZE, TRACK_OBJECT_COUNT>
            + TestConstructors
            + 'static,
        L: RawLock + IsRawMutex + 'static,
        SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>: MaybeTracked,
    {
        allocated_obj_count.store(0, Ordering::SeqCst);
        let init = SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::init(max_slabs);
        stack_pin_init!(let allocator = init);

        assert_eq!(allocator.slab_count(), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }

        let max_allocs =
            SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::ALLOCS_PER_SLAB * max_slabs;
        let mut ref_list = Vec::new();

        for i in 0..test_allocs {
            let expected_count = min(i, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count);
            }

            let val = match i % 4 {
                0 => T::new_default(),
                1 => T::new_lvalue(i),
                2 => T::new_rvalue(i),
                _ => T::new_l_then_r(i, i),
            };

            let ptr = allocator.new_unique(val);

            if i < max_allocs {
                let obj = ptr.expect("Allocation failed when it should not have!");
                ref_list.push(obj);
            } else {
                assert!(ptr.is_err(), "Allocation succeeded when it should not have!");
            }

            let expected_count_after = min(i + 1, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count_after);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count_after);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count_after);
            }
        }

        let mut max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
        let total_allocated = ref_list.len();

        for (i, obj) in ref_list.into_iter().enumerate() {
            let current_expected = total_allocated - i;
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), current_expected);
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }

            match i % 4 {
                0 => assert_eq!(obj.ctype(), ConstructType::Default),
                1 => assert_eq!(obj.ctype(), ConstructType::LvalueRef),
                2 => assert_eq!(obj.ctype(), ConstructType::RvalueRef),
                _ => assert_eq!(obj.ctype(), ConstructType::LThenRRef),
            }

            drop(obj); // This returns memory to the free list

            if TRACK_OBJECT_COUNT {
                if i % 2 == 1 {
                    allocator.maybe_reset_max_obj_count();
                    max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
                }
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }
        }

        assert_eq!(allocated_obj_count.load(Ordering::SeqCst), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), total_allocated % 2);
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }
    }

    fn run_instanced_ref_test<T, L, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>(
        allocated_obj_count: &'static AtomicUsize,
        max_slabs: usize,
        test_allocs: usize,
    ) where
        T: HasRefCount
            + Recyclable
            + InstancedSlabAllocated<L, SLAB_SIZE, TRACK_OBJECT_COUNT>
            + TestConstructors
            + 'static,
        L: RawLock + IsRawMutex + 'static,
        SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>: MaybeTracked,
    {
        allocated_obj_count.store(0, Ordering::SeqCst);
        let init = SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::init(max_slabs);
        stack_pin_init!(let allocator = init);

        assert_eq!(allocator.slab_count(), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }

        let max_allocs =
            SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::ALLOCS_PER_SLAB * max_slabs;
        let mut ref_list = Vec::new();

        for i in 0..test_allocs {
            let expected_count = min(i, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count);
            }

            let val = match i % 4 {
                0 => T::new_default(),
                1 => T::new_lvalue(i),
                2 => T::new_rvalue(i),
                _ => T::new_l_then_r(i, i),
            };

            let ptr = allocator.new_ref(val);

            if i < max_allocs {
                let obj = ptr.expect("Allocation failed when it should not have!");
                ref_list.push(obj);
            } else {
                assert!(ptr.is_err(), "Allocation succeeded when it should not have!");
            }

            let expected_count_after = min(i + 1, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count_after);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count_after);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count_after);
            }
        }

        let mut max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
        let total_allocated = ref_list.len();

        for (i, obj) in ref_list.into_iter().enumerate() {
            let current_expected = total_allocated - i;
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), current_expected);
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }

            match i % 4 {
                0 => assert_eq!(obj.ctype(), ConstructType::Default),
                1 => assert_eq!(obj.ctype(), ConstructType::LvalueRef),
                2 => assert_eq!(obj.ctype(), ConstructType::RvalueRef),
                _ => assert_eq!(obj.ctype(), ConstructType::LThenRRef),
            }

            // Test cloning
            {
                let _clone = obj.clone();
                assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);
            }

            drop(obj); // This returns memory to the free list

            if TRACK_OBJECT_COUNT {
                if i % 2 == 1 {
                    allocator.maybe_reset_max_obj_count();
                    max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
                }
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }
        }

        assert_eq!(allocated_obj_count.load(Ordering::SeqCst), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), total_allocated % 2);
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }
    }

    fn run_unmanaged_test<T, L, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>(
        allocated_obj_count: &'static AtomicUsize,
        max_slabs: usize,
        test_allocs: usize,
    ) where
        T: TestConstructors + 'static,
        L: RawLock + IsRawMutex + 'static,
        SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>: MaybeTracked,
    {
        allocated_obj_count.store(0, Ordering::SeqCst);
        let init = SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::init(max_slabs);
        stack_pin_init!(let allocator = init);

        assert_eq!(allocator.slab_count(), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }

        let max_allocs =
            SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::ALLOCS_PER_SLAB * max_slabs;
        let mut ref_list = Vec::new();

        for i in 0..test_allocs {
            let expected_count = min(i, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count);
            }

            let ptr = allocator.alloc_raw();

            if i < max_allocs {
                let p = ptr.expect("Allocation failed when it should not have!");
                // SAFETY: We initialize the newly allocated raw memory.
                unsafe {
                    let val = match i % 4 {
                        0 => T::new_default(),
                        1 => T::new_lvalue(i),
                        2 => T::new_rvalue(i),
                        _ => T::new_l_then_r(i, i),
                    };
                    write(p.as_ptr(), val);
                }
                ref_list.push(p);
            } else {
                assert!(ptr.is_err(), "Allocation succeeded when it should not have!");
            }

            let expected_count_after = min(i + 1, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count_after);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count_after);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count_after);
            }
        }

        let mut max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
        let total_allocated = ref_list.len();

        for (i, p) in ref_list.into_iter().enumerate() {
            let current_expected = total_allocated - i;
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), current_expected);
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }

            // SAFETY: `p` is a valid pointer to T
            unsafe {
                match i % 4 {
                    0 => assert_eq!(p.as_ref().ctype(), ConstructType::Default),
                    1 => assert_eq!(p.as_ref().ctype(), ConstructType::LvalueRef),
                    2 => assert_eq!(p.as_ref().ctype(), ConstructType::RvalueRef),
                    _ => assert_eq!(p.as_ref().ctype(), ConstructType::LThenRRef),
                }

                allocator.delete(p);
            }

            if TRACK_OBJECT_COUNT {
                if i % 2 == 1 {
                    allocator.maybe_reset_max_obj_count();
                    max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
                }
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }
        }

        assert_eq!(allocated_obj_count.load(Ordering::SeqCst), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), total_allocated % 2);
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }
    }

    fn run_static_unique_test<T, L, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>(
        allocated_obj_count: &'static AtomicUsize,
        allocator: &'static SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>,
        max_slabs: usize,
        test_allocs: usize,
    ) where
        T: Recyclable
            + StaticSlabAllocated<L, SLAB_SIZE, TRACK_OBJECT_COUNT>
            + TestConstructors
            + 'static,
        L: RawLock + IsRawMutex + 'static,
        SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>: MaybeTracked,
    {
        allocated_obj_count.store(0, Ordering::SeqCst);

        if TRACK_OBJECT_COUNT {
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }

        let max_allocs =
            SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::ALLOCS_PER_SLAB * max_slabs;
        let mut ref_list = Vec::new();

        for i in 0..test_allocs {
            let expected_count = min(i, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count);
            }

            let val = match i % 4 {
                0 => T::new_default(),
                1 => T::new_lvalue(i),
                2 => T::new_rvalue(i),
                _ => T::new_l_then_r(i, i),
            };

            let ptr = UniquePtr::try_new(val);

            if i < max_allocs {
                let obj = ptr.expect("Allocation failed when it should not have!");
                ref_list.push(obj);
            } else {
                assert!(ptr.is_err(), "Allocation succeeded when it should not have!");
            }

            let expected_count_after = min(i + 1, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count_after);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count_after);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count_after);
            }
        }

        let mut max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
        let total_allocated = ref_list.len();

        for (i, obj) in ref_list.into_iter().enumerate() {
            let current_expected = total_allocated - i;
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), current_expected);
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }

            match i % 4 {
                0 => assert_eq!(obj.ctype(), ConstructType::Default),
                1 => assert_eq!(obj.ctype(), ConstructType::LvalueRef),
                2 => assert_eq!(obj.ctype(), ConstructType::RvalueRef),
                _ => assert_eq!(obj.ctype(), ConstructType::LThenRRef),
            }

            drop(obj); // This returns memory to the free list

            if TRACK_OBJECT_COUNT {
                if i % 2 == 1 {
                    allocator.maybe_reset_max_obj_count();
                    max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
                }
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }
        }

        assert_eq!(allocated_obj_count.load(Ordering::SeqCst), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), total_allocated % 2);
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }
    }

    fn run_static_ref_test<T, L, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>(
        allocated_obj_count: &'static AtomicUsize,
        allocator: &'static SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>,
        max_slabs: usize,
        test_allocs: usize,
    ) where
        T: HasRefCount
            + Recyclable
            + StaticSlabAllocated<L, SLAB_SIZE, TRACK_OBJECT_COUNT>
            + TestConstructors
            + 'static,
        L: RawLock + IsRawMutex + 'static,
        SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>: MaybeTracked,
    {
        allocated_obj_count.store(0, Ordering::SeqCst);

        if TRACK_OBJECT_COUNT {
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }

        let max_allocs =
            SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::ALLOCS_PER_SLAB * max_slabs;
        let mut ref_list = Vec::new();

        for i in 0..test_allocs {
            let expected_count = min(i, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count);
            }

            let val = match i % 4 {
                0 => T::new_default(),
                1 => T::new_lvalue(i),
                2 => T::new_rvalue(i),
                _ => T::new_l_then_r(i, i),
            };

            // SAFETY: The object is not yet adopted.
            let ptr = unsafe { RefPtr::try_new(val) };

            if i < max_allocs {
                let obj = ptr.expect("Allocation failed when it should not have!");
                ref_list.push(obj);
            } else {
                assert!(ptr.is_err(), "Allocation succeeded when it should not have!");
            }

            let expected_count_after = min(i + 1, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count_after);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count_after);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count_after);
            }
        }

        let mut max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
        let total_allocated = ref_list.len();

        for (i, obj) in ref_list.into_iter().enumerate() {
            let current_expected = total_allocated - i;
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), current_expected);
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }

            match i % 4 {
                0 => assert_eq!(obj.ctype(), ConstructType::Default),
                1 => assert_eq!(obj.ctype(), ConstructType::LvalueRef),
                2 => assert_eq!(obj.ctype(), ConstructType::RvalueRef),
                _ => assert_eq!(obj.ctype(), ConstructType::LThenRRef),
            }

            // Test cloning
            {
                let _clone = obj.clone();
                assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);
            }

            drop(obj); // This returns memory to the free list

            if TRACK_OBJECT_COUNT {
                if i % 2 == 1 {
                    allocator.maybe_reset_max_obj_count();
                    max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
                }
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }
        }

        assert_eq!(allocated_obj_count.load(Ordering::SeqCst), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), total_allocated % 2);
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }
    }

    fn run_static_unmanaged_test<T, L, const SLAB_SIZE: usize, const TRACK_OBJECT_COUNT: bool>(
        allocated_obj_count: &'static AtomicUsize,
        allocator: &'static SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>,
        max_slabs: usize,
        test_allocs: usize,
    ) where
        T: TestConstructors + 'static,
        L: RawLock + IsRawMutex + 'static,
        SlabAllocator<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>: MaybeTracked,
    {
        allocated_obj_count.store(0, Ordering::SeqCst);

        if TRACK_OBJECT_COUNT {
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }

        let max_allocs =
            SlabAllocator::<T, L, SLAB_SIZE, TRACK_OBJECT_COUNT>::ALLOCS_PER_SLAB * max_slabs;
        let mut ref_list = Vec::new();

        for i in 0..test_allocs {
            let expected_count = min(i, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count);
            }

            let ptr = allocator.alloc_raw();

            if i < max_allocs {
                let p = ptr.expect("Allocation failed when it should not have!");
                // SAFETY: We initialize the newly allocated raw memory.
                unsafe {
                    let val = match i % 4 {
                        0 => T::new_default(),
                        1 => T::new_lvalue(i),
                        2 => T::new_rvalue(i),
                        _ => T::new_l_then_r(i, i),
                    };
                    write(p.as_ptr(), val);
                }
                ref_list.push(p);
            } else {
                assert!(ptr.is_err(), "Allocation succeeded when it should not have!");
            }

            let expected_count_after = min(i + 1, max_allocs);
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), expected_count_after);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), expected_count_after);
                assert_eq!(allocator.maybe_max_obj_count(), expected_count_after);
            }
        }

        let mut max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
        let total_allocated = ref_list.len();

        for (i, p) in ref_list.into_iter().enumerate() {
            let current_expected = total_allocated - i;
            assert_eq!(allocated_obj_count.load(Ordering::SeqCst), current_expected);

            if TRACK_OBJECT_COUNT {
                assert_eq!(allocator.maybe_obj_count(), current_expected);
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }

            // SAFETY: `p` is a valid pointer to T
            unsafe {
                match i % 4 {
                    0 => assert_eq!(p.as_ref().ctype(), ConstructType::Default),
                    1 => assert_eq!(p.as_ref().ctype(), ConstructType::LvalueRef),
                    2 => assert_eq!(p.as_ref().ctype(), ConstructType::RvalueRef),
                    _ => assert_eq!(p.as_ref().ctype(), ConstructType::LThenRRef),
                }

                allocator.delete(p);
            }

            if TRACK_OBJECT_COUNT {
                if i % 2 == 1 {
                    allocator.maybe_reset_max_obj_count();
                    max_obj_count = allocated_obj_count.load(Ordering::SeqCst);
                }
                assert_eq!(allocator.maybe_max_obj_count(), max_obj_count);
            }
        }

        assert_eq!(allocated_obj_count.load(Ordering::SeqCst), 0);
        if TRACK_OBJECT_COUNT {
            assert_eq!(allocator.maybe_obj_count(), 0);
            assert_eq!(allocator.maybe_max_obj_count(), total_allocated % 2);
            allocator.maybe_reset_max_obj_count();
            assert_eq!(allocator.maybe_max_obj_count(), 0);
        }
    }

    macro_rules! define_instanced_unique_test {
        ($name:ident, $lock:ty, $lock_init:expr, $slab_size:expr, $track_obj_count:expr, $max_slabs:expr) => {
            #[test]
            fn $name() {
                static ALLOCATED_OBJ_COUNT: AtomicUsize = AtomicUsize::new(0);
                struct Obj {
                    ctype: ConstructType,
                    slab_origin: SlabOrigin<Obj, $lock, $slab_size, $track_obj_count>,
                    _payload: [u8; 13],
                }
                impl TestConstructors for Obj {
                    fn new_default() -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ctype: ConstructType::Default,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn new_lvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ctype: ConstructType::LvalueRef,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn new_rvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ctype: ConstructType::RvalueRef,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn new_l_then_r(_a: usize, _b: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ctype: ConstructType::LThenRRef,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn ctype(&self) -> ConstructType {
                        self.ctype
                    }
                }
                impl Drop for Obj {
                    fn drop(&mut self) {
                        ALLOCATED_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                crate::impl_instanced_slab_allocatable!(Obj, $lock, $slab_size, $track_obj_count);

                let max_allocs =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::ALLOCS_PER_SLAB
                        * $max_slabs;

                run_instanced_unique_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    1,
                );
                run_instanced_unique_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    max_allocs / 2,
                );
                run_instanced_unique_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    max_allocs + 4,
                );
            }
        };
    }

    macro_rules! define_instanced_ref_test {
        ($name:ident, $lock:ty, $lock_init:expr, $slab_size:expr, $track_obj_count:expr, $max_slabs:expr) => {
            #[test]
            fn $name() {
                static ALLOCATED_OBJ_COUNT: AtomicUsize = AtomicUsize::new(0);
                #[crate::ref_counted]
                #[repr(C)]
                struct Obj {
                    ctype: ConstructType,
                    slab_origin: SlabOrigin<Obj, $lock, $slab_size, $track_obj_count>,
                    _payload: [u8; 13],
                }
                impl TestConstructors for Obj {
                    fn new_default() -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::Default,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn new_lvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::LvalueRef,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn new_rvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::RvalueRef,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn new_l_then_r(_a: usize, _b: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::LThenRRef,
                            slab_origin: SlabOrigin::new(),
                            _payload: [0; 13],
                        }
                    }
                    fn ctype(&self) -> ConstructType {
                        self.ctype
                    }
                }
                impl Drop for Obj {
                    fn drop(&mut self) {
                        ALLOCATED_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                crate::impl_instanced_slab_allocatable!(Obj, $lock, $slab_size, $track_obj_count);

                let max_allocs =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::ALLOCS_PER_SLAB
                        * $max_slabs;

                run_instanced_ref_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    1,
                );
                run_instanced_ref_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    max_allocs / 2,
                );
                run_instanced_ref_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    max_allocs + 4,
                );
            }
        };
    }

    macro_rules! define_unmanaged_test {
        ($name:ident, $lock:ty, $lock_init:expr, $slab_size:expr, $track_obj_count:expr, $max_slabs:expr) => {
            #[test]
            fn $name() {
                static ALLOCATED_OBJ_COUNT: AtomicUsize = AtomicUsize::new(0);
                struct Obj {
                    ctype: ConstructType,
                    _payload: [u8; 13],
                }
                impl TestConstructors for Obj {
                    fn new_default() -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::Default, _payload: [0; 13] }
                    }
                    fn new_lvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::LvalueRef, _payload: [0; 13] }
                    }
                    fn new_rvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::RvalueRef, _payload: [0; 13] }
                    }
                    fn new_l_then_r(_a: usize, _b: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::LThenRRef, _payload: [0; 13] }
                    }
                    fn ctype(&self) -> ConstructType {
                        self.ctype
                    }
                }
                impl Drop for Obj {
                    fn drop(&mut self) {
                        ALLOCATED_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                }

                let max_allocs =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::ALLOCS_PER_SLAB
                        * $max_slabs;

                run_unmanaged_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    1,
                );
                run_unmanaged_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    max_allocs / 2,
                );
                run_unmanaged_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    $max_slabs,
                    max_allocs + 4,
                );
            }
        };
    }

    macro_rules! define_static_unique_test {
        ($name:ident, $lock:ty, $lock_init:expr, $slab_size:expr, $track_obj_count:expr, $max_slabs:expr) => {
            #[test]
            fn $name() {
                static ALLOCATED_OBJ_COUNT: AtomicUsize = AtomicUsize::new(0);
                struct Obj {
                    ctype: ConstructType,
                    _payload: [u8; 13],
                }
                impl TestConstructors for Obj {
                    fn new_default() -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::Default, _payload: [0; 13] }
                    }
                    fn new_lvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::LvalueRef, _payload: [0; 13] }
                    }
                    fn new_rvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::RvalueRef, _payload: [0; 13] }
                    }
                    fn new_l_then_r(_a: usize, _b: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::LThenRRef, _payload: [0; 13] }
                    }
                    fn ctype(&self) -> ConstructType {
                        self.ctype
                    }
                }
                impl Drop for Obj {
                    fn drop(&mut self) {
                        ALLOCATED_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                }

                static ALLOCATOR: SlabAllocator<Obj, $lock, $slab_size, $track_obj_count> =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::const_new(
                        $max_slabs, $lock_init,
                    );

                crate::impl_static_slab_allocatable!(
                    Obj,
                    $lock,
                    $slab_size,
                    ALLOCATOR,
                    $track_obj_count
                );

                let max_allocs =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::ALLOCS_PER_SLAB
                        * $max_slabs;

                run_static_unique_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    1,
                );
                run_static_unique_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    max_allocs / 2,
                );
                run_static_unique_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    max_allocs + 4,
                );
            }
        };
    }

    macro_rules! define_static_ref_test {
        ($name:ident, $lock:ty, $lock_init:expr, $slab_size:expr, $track_obj_count:expr, $max_slabs:expr) => {
            #[test]
            fn $name() {
                static ALLOCATED_OBJ_COUNT: AtomicUsize = AtomicUsize::new(0);
                #[crate::ref_counted]
                #[repr(C)]
                struct Obj {
                    ctype: ConstructType,
                    _payload: [u8; 13],
                }
                impl TestConstructors for Obj {
                    fn new_default() -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::Default,
                            _payload: [0; 13],
                        }
                    }
                    fn new_lvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::LvalueRef,
                            _payload: [0; 13],
                        }
                    }
                    fn new_rvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::RvalueRef,
                            _payload: [0; 13],
                        }
                    }
                    fn new_l_then_r(_a: usize, _b: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self {
                            ref_count: RefCounted::new(),
                            __fbl_ref_counted_guard: (),
                            ctype: ConstructType::LThenRRef,
                            _payload: [0; 13],
                        }
                    }
                    fn ctype(&self) -> ConstructType {
                        self.ctype
                    }
                }
                impl Drop for Obj {
                    fn drop(&mut self) {
                        ALLOCATED_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                }

                static ALLOCATOR: SlabAllocator<Obj, $lock, $slab_size, $track_obj_count> =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::const_new(
                        $max_slabs, $lock_init,
                    );

                crate::impl_static_slab_allocatable!(
                    Obj,
                    $lock,
                    $slab_size,
                    ALLOCATOR,
                    $track_obj_count
                );

                let max_allocs =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::ALLOCS_PER_SLAB
                        * $max_slabs;

                // Test make_ref_counted! macro works with static allocator
                {
                    ALLOCATED_OBJ_COUNT.store(1, Ordering::SeqCst);
                    let obj = crate::make_ref_counted!(Obj {
                        ctype: ConstructType::Default,
                        _payload: [0; 13],
                    })
                    .unwrap();
                    assert_eq!(ALLOCATED_OBJ_COUNT.load(Ordering::SeqCst), 1);
                    drop(obj);
                    assert_eq!(ALLOCATED_OBJ_COUNT.load(Ordering::SeqCst), 0);
                }

                run_static_ref_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    1,
                );
                run_static_ref_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    max_allocs / 2,
                );
                run_static_ref_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    max_allocs + 4,
                );
            }
        };
    }

    macro_rules! define_static_unmanaged_test {
        ($name:ident, $lock:ty, $lock_init:expr, $slab_size:expr, $track_obj_count:expr, $max_slabs:expr) => {
            #[test]
            fn $name() {
                static ALLOCATED_OBJ_COUNT: AtomicUsize = AtomicUsize::new(0);
                struct Obj {
                    ctype: ConstructType,
                    _payload: [u8; 13],
                }
                impl TestConstructors for Obj {
                    fn new_default() -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::Default, _payload: [0; 13] }
                    }
                    fn new_lvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::LvalueRef, _payload: [0; 13] }
                    }
                    fn new_rvalue(_val: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::RvalueRef, _payload: [0; 13] }
                    }
                    fn new_l_then_r(_a: usize, _b: usize) -> Self {
                        ALLOCATED_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
                        Self { ctype: ConstructType::LThenRRef, _payload: [0; 13] }
                    }
                    fn ctype(&self) -> ConstructType {
                        self.ctype
                    }
                }
                impl Drop for Obj {
                    fn drop(&mut self) {
                        ALLOCATED_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }
                }

                static ALLOCATOR: SlabAllocator<Obj, $lock, $slab_size, $track_obj_count> =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::const_new(
                        $max_slabs, $lock_init,
                    );

                let max_allocs =
                    SlabAllocator::<Obj, $lock, $slab_size, $track_obj_count>::ALLOCS_PER_SLAB
                        * $max_slabs;

                run_static_unmanaged_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    1,
                );
                run_static_unmanaged_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    max_allocs / 2,
                );
                run_static_unmanaged_test::<Obj, $lock, $slab_size, $track_obj_count>(
                    &ALLOCATED_OBJ_COUNT,
                    &ALLOCATOR,
                    $max_slabs,
                    max_allocs + 4,
                );
            }
        };
    }

    define_unmanaged_test!(unmanaged_single_slab_mutex, RawMutex, RawMutex::INIT, 1024, false, 1);
    define_unmanaged_test!(unmanaged_multi_slab_mutex, RawMutex, RawMutex::INIT, 1024, false, 4);
    define_instanced_unique_test!(
        unique_ptr_single_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        false,
        1
    );
    define_instanced_unique_test!(
        unique_ptr_multi_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        false,
        4
    );
    define_instanced_ref_test!(ref_ptr_single_slab_mutex, RawMutex, RawMutex::INIT, 1024, false, 1);
    define_instanced_ref_test!(ref_ptr_multi_slab_mutex, RawMutex, RawMutex::INIT, 1024, false, 4);

    // Counted versions
    define_unmanaged_test!(
        counted_unmanaged_single_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        1
    );
    define_unmanaged_test!(
        counted_unmanaged_multi_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        4
    );
    define_instanced_unique_test!(
        counted_unique_ptr_single_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        1
    );
    define_instanced_unique_test!(
        counted_unique_ptr_multi_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        4
    );
    define_instanced_ref_test!(
        counted_ref_ptr_single_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        1
    );
    define_instanced_ref_test!(
        counted_ref_ptr_multi_slab_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        4
    );

    // Static versions
    define_static_unmanaged_test!(static_unmanaged_mutex, RawMutex, RawMutex::INIT, 1024, false, 4);
    define_static_unique_test!(static_unique_ptr_mutex, RawMutex, RawMutex::INIT, 1024, false, 4);
    define_static_ref_test!(static_ref_ptr_mutex, RawMutex, RawMutex::INIT, 1024, false, 4);

    // Counted Static versions
    define_static_unmanaged_test!(
        counted_static_unmanaged_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        4
    );
    define_static_unique_test!(
        counted_static_unique_ptr_mutex,
        RawMutex,
        RawMutex::INIT,
        1024,
        true,
        4
    );
    define_static_ref_test!(counted_static_ref_ptr_mutex, RawMutex, RawMutex::INIT, 1024, true, 4);

    #[test]
    fn test_constructor_statistics_fix() {
        // With TRACK_OBJECT_COUNT = true, and preallocate.
        // The obj_count and max_obj_count should remain exactly 0 after construction and preallocation.
        let init = SlabAllocator::<TestObjectDummy, RawMutex, 1024, true>::init(1);
        stack_pin_init!(let allocator = init);
        allocator.preallocate().unwrap();
        assert_eq!(allocator.obj_count(), 0);
        assert_eq!(allocator.max_obj_count(), 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "SlabAllocator destroyed with outstanding allocations!")]
    fn test_leak_detector_counted() {
        let init = SlabAllocator::<u32, RawMutex, 1024, true>::init(1);
        stack_pin_init!(let allocator = init);
        let _ptr = allocator.alloc_raw().unwrap();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "SlabAllocator destroyed with outstanding allocations!")]
    fn test_leak_detector_uncounted() {
        let init = SlabAllocator::<u32, RawMutex, 1024, false>::init(1);
        stack_pin_init!(let allocator = init);
        let _ptr = allocator.alloc_raw().unwrap();
    }

    struct TestObjectDummy {
        _val: u32,
        slab_origin: SlabOrigin<TestObjectDummy, RawMutex, 1024, true>,
    }
    crate::impl_instanced_slab_allocatable!(TestObjectDummy, RawMutex, 1024, true);
}
