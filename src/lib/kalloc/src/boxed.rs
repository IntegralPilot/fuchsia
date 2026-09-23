// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::allocator::{AllocError, Allocator, DefaultAllocator};
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use zerocopy::FromZeros;

/// Helper to construct a dangling slice of a given length.
fn dangling_slice<T>(len: usize) -> NonNull<[MaybeUninit<T>]> {
    let dangling = NonNull::<T>::dangling();
    NonNull::slice_from_raw_parts(dangling.cast::<MaybeUninit<T>>(), len)
}

/// Helper to allocate an uninitialized slice.
fn allocate_slice<T, A: Allocator>(
    allocator: &A,
    len: usize,
) -> Result<NonNull<[MaybeUninit<T>]>, AllocError> {
    let layout = core::alloc::Layout::array::<T>(len).map_err(|_| AllocError)?;
    if layout.size() == 0 {
        return Ok(dangling_slice::<T>(len));
    }
    let ptr = allocator.allocate(layout)?;
    let casted_thin = ptr.cast::<MaybeUninit<T>>();
    Ok(NonNull::slice_from_raw_parts(casted_thin, len))
}

/// Helper to allocate a zeroed slice.
fn allocate_zeroed_slice<T, A: Allocator>(
    allocator: &A,
    len: usize,
) -> Result<NonNull<[MaybeUninit<T>]>, AllocError> {
    let layout = core::alloc::Layout::array::<T>(len).map_err(|_| AllocError)?;
    if layout.size() == 0 {
        return Ok(dangling_slice::<T>(len));
    }
    let ptr = allocator.allocate_zeroed(layout)?;
    let casted_thin = ptr.cast::<MaybeUninit<T>>();
    Ok(NonNull::slice_from_raw_parts(casted_thin, len))
}

/// Helper to deallocate a slice.
///
/// # Safety
///
/// The caller must guarantee that `ptr` was allocated by this allocator
/// with a layout matching `ptr.len()` elements of `T`.
unsafe fn deallocate_slice<T, A: Allocator>(allocator: &A, ptr: NonNull<[MaybeUninit<T>]>) {
    let len = ptr.len();
    // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
    unsafe {
        let layout = core::alloc::Layout::array::<T>(len).unwrap_unchecked();
        if layout.size() == 0 {
            return;
        }
        allocator.deallocate(ptr.cast::<u8>(), layout);
    }
}

/// Helper to grow a slice.
///
/// # Safety
///
/// The caller must guarantee that `ptr` was allocated by this allocator
/// with a layout matching `ptr.len()` elements of `T`.
unsafe fn grow_slice<T, A: Allocator>(
    allocator: &A,
    ptr: NonNull<[MaybeUninit<T>]>,
    new_len: usize,
) -> Result<NonNull<[MaybeUninit<T>]>, AllocError> {
    let old_len = ptr.len();
    assert!(new_len > old_len);

    let old_layout = core::alloc::Layout::array::<T>(old_len).map_err(|_| AllocError)?;
    let new_layout = core::alloc::Layout::array::<T>(new_len).map_err(|_| AllocError)?;

    if old_layout.size() == 0 {
        return allocate_slice(allocator, new_len);
    }

    // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
    let new_ptr = unsafe { allocator.grow(ptr.cast::<u8>(), old_layout, new_layout)? };
    Ok(NonNull::slice_from_raw_parts(new_ptr.cast::<MaybeUninit<T>>(), new_len))
}

/// Helper to shrink a slice.
///
/// # Safety
///
/// The caller must guarantee that `ptr` was allocated by this allocator
/// with a layout matching `ptr.len()` elements of `T`.
unsafe fn shrink_slice<T, A: Allocator>(
    allocator: &A,
    ptr: NonNull<[MaybeUninit<T>]>,
    new_len: usize,
) -> Result<NonNull<[MaybeUninit<T>]>, AllocError> {
    let old_len = ptr.len();
    assert!(new_len < old_len);

    let old_layout = core::alloc::Layout::array::<T>(old_len).map_err(|_| AllocError)?;
    let new_layout = core::alloc::Layout::array::<T>(new_len).map_err(|_| AllocError)?;

    if new_layout.size() == 0 {
        // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
        unsafe {
            deallocate_slice::<T, A>(allocator, ptr);
        }
        return Ok(dangling_slice::<T>(new_len));
    }

    // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
    let new_ptr = unsafe { allocator.shrink(ptr.cast::<u8>(), old_layout, new_layout)? };
    Ok(NonNull::slice_from_raw_parts(new_ptr.cast::<MaybeUninit<T>>(), new_len))
}

/// A custom Box type appropriate for fallible allocation.
pub struct Box<T: ?Sized, A: Allocator = DefaultAllocator> {
    /// The pointer to the value.
    ///
    /// # Invariants
    ///
    /// This pointer may originate from:
    /// 1. An allocation from `allocator`.
    /// 2. A sentinel value for empty slices (obtained via `NonNull::from_ref(&[])`).
    /// 3. A dangling pointer for Zero-Sized Types (ZSTs) (obtained via `NonNull::dangling()`).
    ///
    /// The implementation must ensure that pointers not originating from `allocator`
    /// (cases 2 and 3) are never passed to the allocator's `deallocate`, `grow`, or `shrink` methods.
    ptr: NonNull<T>,
    allocator: A,
}

impl<T: ?Sized, A: Allocator> Box<T, A> {
    /// Creates a Box from a raw pointer.
    ///
    /// This does NOT allocate any memory. It simply takes ownership of the memory
    /// pointed to by `ptr`.
    ///
    /// # Safety
    ///
    /// - For non-zero-sized types, the pointer must be valid and have been allocated
    ///   by the same allocator.
    /// - For zero-sized types (ZSTs), the pointer must be non-null and properly aligned.
    ///   Consider using `NonNull::dangling()` to obtain such a pointer.
    pub const unsafe fn from_raw_in(ptr: *mut T, allocator: A) -> Self {
        // SAFETY: The caller must guarantee `ptr` is valid for its type, which implies
        // non-null for non-zero-sized types, and properly aligned for zero-sized types.
        unsafe { Self::from_non_null_in(NonNull::new_unchecked(ptr), allocator) }
    }

    /// Constructs a box from a NonNull pointer.
    ///
    /// # Safety
    ///
    /// - For non-zero-sized types, the pointer must be valid and have been allocated
    ///   by the same allocator.
    /// - For zero-sized types (ZSTs), the pointer must be non-null and properly aligned.
    ///   Consider using `NonNull::dangling()` to obtain such a pointer.
    pub const unsafe fn from_non_null_in(ptr: NonNull<T>, allocator: A) -> Self {
        Self { ptr, allocator }
    }

    /// Returns the raw pointer.
    pub fn as_ptr(this: &Self) -> *mut T {
        this.ptr.as_ptr()
    }

    /// Consumes the `Box`, returning a mutable reference to `T`.
    ///
    /// The memory will be leaked, and never deallocated.
    ///
    /// # Note
    ///
    /// The allocator `A` is also leaked. This is intentional and matches the
    /// behavior of `std::boxed::Box::leak`, ensuring that a stateful allocator
    /// remains valid as long as the leaked reference.
    ///
    /// Consumes the `Box`, returning a raw pointer and the allocator.
    ///
    /// The memory will be leaked, and never deallocated unless reconstructed.
    pub fn into_raw_with_allocator(this: Self) -> (*mut T, A) {
        let me = core::mem::ManuallyDrop::new(this);
        let ptr = me.ptr.as_ptr();
        // SAFETY: We are moving the allocator out of `me`, and `me` is ManuallyDrop
        // so its Drop implementation (which would deallocate) will not run.
        let allocator = unsafe { core::ptr::read(&me.allocator) };
        (ptr, allocator)
    }
}

impl<T: ?Sized> Box<T, DefaultAllocator> {
    /// Creates a Box from a raw pointer using the default allocator.
    ///
    /// # Safety
    ///
    /// - For non-zero-sized types, the pointer must be valid and have been allocated
    ///   by the default allocator.
    /// - For zero-sized types (ZSTs), the pointer must be non-null and properly aligned.
    ///   Consider using `NonNull::dangling()` to obtain such a pointer.
    pub const unsafe fn from_raw(ptr: *mut T) -> Self {
        unsafe { Self::from_raw_in(ptr, DefaultAllocator) }
    }

    /// Constructs a box from a NonNull pointer using the default allocator.
    ///
    /// # Safety
    ///
    /// - For non-zero-sized types, the pointer must be valid and have been allocated
    ///   by the default allocator.
    /// - For zero-sized types (ZSTs), the pointer must be non-null and properly aligned.
    ///   Consider using `NonNull::dangling()` to obtain such a pointer.
    pub const unsafe fn from_non_null(ptr: NonNull<T>) -> Self {
        unsafe { Self::from_non_null_in(ptr, DefaultAllocator) }
    }

    /// Consumes the `Box`, returning a raw pointer.
    ///
    /// The memory will be leaked, and never deallocated unless reconstructed.
    pub fn into_raw(this: Self) -> *mut T {
        let (ptr, _) = Box::into_raw_with_allocator(this);
        ptr
    }
}

impl<T, A: Allocator> Box<[T], A> {
    /// Creates an empty slice Box.
    ///
    /// Infallible because it doesn't allocate memory.
    pub const fn empty_slice_in(allocator: A) -> Self {
        // SAFETY: `NonNull::from_ref(&[])` creates a non-null pointer with the
        // proper alignment.
        unsafe { Self::from_non_null_in(NonNull::from_ref(&[]), allocator) }
    }

    /// Tries to allocate a new slice of the given length.
    pub fn try_new_uninit_slice_in(
        len: usize,
        allocator: A,
    ) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        let fat_ptr = allocate_slice::<T, A>(&allocator, len)?;
        // SAFETY: `fat_ptr` points to a valid allocation.
        Ok(unsafe { Box::from_non_null_in(fat_ptr, allocator) })
    }

    /// Tries to allocate a new zeroed slice of the given length.
    pub fn try_new_zeroed_uninit_slice_in(
        len: usize,
        allocator: A,
    ) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        let fat_ptr = allocate_zeroed_slice::<T, A>(&allocator, len)?;
        // SAFETY: `fat_ptr` points to a valid allocation.
        Ok(unsafe { Box::from_non_null_in(fat_ptr, allocator) })
    }
}

impl<T> Box<[T], DefaultAllocator> {
    pub const fn empty_slice() -> Self {
        Self::empty_slice_in(DefaultAllocator)
    }

    pub fn try_new_uninit_slice(
        len: usize,
    ) -> Result<Box<[MaybeUninit<T>], DefaultAllocator>, AllocError> {
        Self::try_new_uninit_slice_in(len, DefaultAllocator)
    }

    /// Tries to allocate a new zeroed slice of the given length.
    pub fn try_new_zeroed_uninit_slice(
        len: usize,
    ) -> Result<Box<[MaybeUninit<T>], DefaultAllocator>, AllocError> {
        Self::try_new_zeroed_uninit_slice_in(len, DefaultAllocator)
    }
}

impl<T: FromZeros, A: Allocator> Box<[T], A> {
    /// Tries to allocate a new zeroed slice of the given length.
    pub fn try_new_zeroed_slice_in(len: usize, allocator: A) -> Result<Self, AllocError> {
        let fat_ptr = allocate_zeroed_slice::<T, A>(&allocator, len)?;
        // SAFETY: `fat_ptr` points to a valid zero-initialized allocation.
        // Since T implements FromZeros, all zeroes is a valid bit pattern for T.
        let ptr = fat_ptr.as_ptr() as *mut [T];
        // SAFETY: `ptr` is non-null because it comes from a successful non-null allocation.
        Ok(unsafe { Self::from_non_null_in(NonNull::new_unchecked(ptr), allocator) })
    }
}

impl<T: FromZeros> Box<[T], DefaultAllocator> {
    /// Tries to allocate a new zeroed slice of the given length.
    pub fn try_new_zeroed_slice(len: usize) -> Result<Self, AllocError> {
        Self::try_new_zeroed_slice_in(len, DefaultAllocator)
    }
}

impl<T, A: Allocator> Box<[MaybeUninit<T>], A> {
    /// Converts to `Box<[T], A>`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the values are initialized.
    pub unsafe fn assume_init(self) -> Box<[T], A> {
        let (ptr, allocator) = Box::into_raw_with_allocator(self);
        let ptr = ptr as *mut [core::mem::MaybeUninit<T>] as *mut [T];
        // SAFETY: The caller must guarantee that the values are initialized.
        unsafe { Box::from_raw_in(ptr, allocator) }
    }

    /// Tries to grow the slice to a new length.
    /// Returns Err if allocation fails, and the box is unchanged.
    pub fn try_grow(this: &mut Self, new_len: usize) -> Result<(), AllocError> {
        // SAFETY: `this.ptr` was allocated by this allocator.
        this.ptr = unsafe { grow_slice::<T, A>(&this.allocator, this.ptr, new_len)? };
        Ok(())
    }

    /// Tries to shrink the slice to a new length.
    /// If `new_len` is 0, the box becomes empty and memory is freed.
    /// Returns Err if allocation fails, and the box is unchanged.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that any elements above `new_len` are either
    /// uninitialized or have already been dropped. This method discards them
    /// without running their destructors.
    pub unsafe fn try_shrink(this: &mut Self, new_len: usize) -> Result<(), AllocError> {
        // SAFETY: `this.ptr` was allocated by this allocator.
        this.ptr = unsafe { shrink_slice::<T, A>(&this.allocator, this.ptr, new_len)? };
        Ok(())
    }
}

impl<T, A: Allocator> Box<T, A> {
    /// Constructs a box for a Zero Sized Type (ZST).
    ///
    /// # Panics
    ///
    /// Panics at compile time if the type is not a ZST.
    const fn new_zst_in(allocator: A) -> Self {
        assert!(core::mem::size_of::<T>() == 0);
        // SAFETY: `NonNull::dangling()` creates a non-null pointer with the proper alignment.
        Self { ptr: NonNull::<T>::dangling(), allocator }
    }

    /// Tries to allocate a new instance of T and move the value into it.
    pub fn try_new_in(value: T, allocator: A) -> Result<Self, AllocError> {
        let mut b = Self::try_new_uninit_in(allocator)?;
        b.write(value);
        Ok(unsafe { b.assume_init() })
    }

    /// Constructs a new box with uninitialized contents on the heap.
    pub fn try_new_uninit_in(allocator: A) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        if core::mem::size_of::<T>() == 0 {
            return Ok(Box::<MaybeUninit<T>, A>::new_zst_in(allocator));
        }
        let layout = core::alloc::Layout::new::<T>();
        let ptr = allocator.allocate(layout)?.cast::<MaybeUninit<T>>();
        // SAFETY: `ptr` points to a valid allocation.
        Ok(unsafe { Box::from_non_null_in(ptr, allocator) })
    }

    /// Constructs a new box with uninitialized contents, filled with 0 bytes.
    pub fn try_new_zeroed_in(allocator: A) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        if core::mem::size_of::<T>() == 0 {
            return Ok(Box::<MaybeUninit<T>, A>::new_zst_in(allocator));
        }
        let layout = core::alloc::Layout::new::<T>();
        let ptr = allocator.allocate_zeroed(layout)?.cast::<MaybeUninit<T>>();
        // SAFETY: `ptr` points to a valid allocation.
        Ok(unsafe { Box::from_non_null_in(ptr, allocator) })
    }
}

impl<T> Box<T, DefaultAllocator> {
    pub fn try_new(value: T) -> Result<Self, AllocError> {
        Self::try_new_in(value, DefaultAllocator)
    }

    pub fn try_new_uninit() -> Result<Box<MaybeUninit<T>>, AllocError> {
        Self::try_new_uninit_in(DefaultAllocator)
    }

    pub fn try_new_zeroed() -> Result<Box<MaybeUninit<T>>, AllocError> {
        Self::try_new_zeroed_in(DefaultAllocator)
    }
}

impl<T, A: Allocator> Box<MaybeUninit<T>, A> {
    /// Converts to `Box<T, A>`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the value is initialized.
    pub unsafe fn assume_init(self) -> Box<T, A> {
        let (ptr, allocator) = Box::into_raw_with_allocator(self);
        // SAFETY: The caller must guarantee that the value is initialized.
        unsafe { Box::from_raw_in(ptr as *mut T, allocator) }
    }
}

impl<T> Default for Box<[T]> {
    fn default() -> Self {
        Self::empty_slice()
    }
}

impl<T: ?Sized, A: Allocator> Deref for Box<T, A> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        // SAFETY: `self.ptr` is valid as long as the `Box` is alive.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized, A: Allocator> DerefMut for Box<T, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `self.ptr` is valid as long as the `Box` is alive.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized, A: Allocator> Drop for Box<T, A> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is valid and was allocated by this allocator.
        unsafe {
            let value = self.ptr.as_mut();
            let layout = core::alloc::Layout::for_value(value);
            core::ptr::drop_in_place(value);
            if layout.size() > 0 {
                self.allocator.deallocate(self.ptr.cast::<u8>(), layout);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    #[test]
    fn test_box_default_slice() {
        let b = Box::<[u32]>::default();
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn test_box_empty_slice() {
        let b = Box::<[u32]>::empty_slice();
        assert_eq!(b.len(), 0);
        assert!(Box::as_ptr(&b) as *mut u8 == NonNull::<u32>::dangling().as_ptr() as *mut u8);
    }

    #[test]
    fn test_box_try_new() {
        let b = Box::<u32>::try_new(42).unwrap();
        assert_eq!(*b, 42);
    }

    #[test]
    fn test_box() {
        let b = Box::<[u32]>::try_new_uninit_slice(10).unwrap();
        assert_eq!(b.len(), 10);
    }

    #[test]
    fn test_box_deref() {
        let b = Box::<[u32]>::try_new_uninit_slice(1).unwrap();
        let mut b = unsafe { b.assume_init() };
        b[0] = 42;
        assert_eq!(b[0], 42);
    }

    #[test]
    fn test_box_as_ptr() {
        let b = Box::<[u32]>::try_new_uninit_slice(10).unwrap();
        let ptr = Box::as_ptr(&b);
        assert!(!ptr.is_null());
    }

    #[test]
    fn test_box_from_raw() {
        let b = Box::<[u32]>::try_new_uninit_slice(10).unwrap();
        let raw_ptr = Box::into_raw(b);
        let fat_ptr = raw_ptr as *mut [u32];

        // Create a new box from the pointer
        let b2: Box<[u32]> = unsafe { Box::from_raw(fat_ptr) };
        assert_eq!(b2.len(), 10);
        // b2 will free the memory on drop.
    }

    struct DropObserver<'a> {
        dropped: &'a core::cell::Cell<bool>,
    }

    impl<'a> Drop for DropObserver<'a> {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }

    #[test]
    fn test_box_drops_content() {
        use core::cell::Cell;
        let dropped = Cell::new(false);
        {
            let observer = DropObserver { dropped: &dropped };
            let _b: Box<DropObserver<'_>> = Box::try_new(observer).unwrap();
            assert_eq!(dropped.get(), false);
        } // b drops here
        assert_eq!(dropped.get(), true);
    }

    #[test]
    #[should_panic]
    fn test_box_slice_out_of_bounds() {
        let b = Box::<[u32]>::try_new_uninit_slice(5).unwrap();
        let _ = b[5]; // Should panic
    }

    #[derive(Clone, Default)]
    struct AlwaysFailingAllocator;

    impl Allocator for AlwaysFailingAllocator {
        fn allocate(&self, _layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            Err(AllocError)
        }

        unsafe fn deallocate(&self, _ptr: NonNull<u8>, _layout: Layout) {
            panic!("Deallocate called on AlwaysFailingAllocator");
        }

        unsafe fn grow(
            &self,
            _ptr: NonNull<u8>,
            _old_layout: Layout,
            _new_layout: Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            Err(AllocError)
        }

        unsafe fn shrink(
            &self,
            _ptr: NonNull<u8>,
            _old_layout: Layout,
            _new_layout: Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            Err(AllocError)
        }

        fn allocate_zeroed(&self, _layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            Err(AllocError)
        }
    }

    #[test]
    fn test_box_try_new_failing() {
        let b =
            Box::<u32, AlwaysFailingAllocator>::try_new_in(42, AlwaysFailingAllocator::default());
        assert!(b.is_err());
    }

    #[test]
    fn test_box_try_new_slice_failing() {
        let b = Box::<[u32], AlwaysFailingAllocator>::try_new_uninit_slice_in(
            10,
            AlwaysFailingAllocator::default(),
        );
        assert!(b.is_err());
    }

    #[test]
    fn test_box_try_new_zeroed() {
        let b = Box::<u32>::try_new_zeroed().unwrap();
        let b = unsafe { b.assume_init() };
        assert_eq!(*b, 0);
    }

    #[test]
    fn test_box_try_new_zeroed_slice() {
        let b = Box::<[u32]>::try_new_zeroed_slice(3).unwrap();
        assert_eq!(*b, [0, 0, 0]);
    }

    #[test]
    fn test_box_try_grow() {
        let mut b = Box::<[u32]>::try_new_uninit_slice(2).unwrap();
        unsafe {
            b[0].as_mut_ptr().write(10);
            b[1].as_mut_ptr().write(20);
        }

        Box::try_grow(&mut b, 5).unwrap();
        assert_eq!(b.len(), 5);
        assert_eq!(unsafe { b[0].assume_init() }, 10);
        assert_eq!(unsafe { b[1].assume_init() }, 20);
    }

    #[test]
    fn test_box_try_shrink() {
        let mut b = Box::<[u32]>::try_new_uninit_slice(5).unwrap();
        unsafe {
            b[0].as_mut_ptr().write(10);
            b[1].as_mut_ptr().write(20);
        }

        unsafe {
            Box::try_shrink(&mut b, 2).unwrap();
        }
        assert_eq!(b.len(), 2);
        assert_eq!(unsafe { b[0].assume_init() }, 10);
        assert_eq!(unsafe { b[1].assume_init() }, 20);
    }

    #[test]
    fn test_box_from_non_null() {
        use core::alloc::Layout;
        let layout = Layout::new::<u32>();
        let ptr = DefaultAllocator::default().allocate(layout).unwrap();
        let thin_ptr = unsafe { NonNull::new_unchecked(ptr.as_ptr() as *mut u8) };
        let casted = thin_ptr.cast::<u32>();
        unsafe {
            casted.as_ptr().write(42);
        }
        let b: Box<u32, DefaultAllocator> = unsafe { Box::from_non_null(casted) };
        assert_eq!(*b, 42);
    }

    #[test]
    fn test_box_into_raw() {
        let b = Box::try_new(42u32).unwrap();
        let ptr = Box::into_raw(b);
        assert_eq!(unsafe { *ptr }, 42);
        unsafe {
            *ptr = 100;
        }
        assert_eq!(unsafe { *ptr }, 100);

        // Reconstruct the box to avoid leaking memory in tests.
        let b = unsafe { Box::from_raw(ptr) };
        assert_eq!(*b, 100);
    }

    #[test]
    fn test_box_into_raw_with_allocator() {
        let b = Box::try_new(42u32).unwrap();
        let (ptr, allocator) = Box::into_raw_with_allocator(b);
        assert_eq!(unsafe { *ptr }, 42);

        // Reconstruct the box to avoid leaking memory in tests.
        let b = unsafe { Box::from_raw_in(ptr, allocator) };
        assert_eq!(*b, 42);
    }

    #[test]
    fn test_box_assume_init_range() {
        let mut b = Box::<[u32]>::try_new_uninit_slice(5).unwrap();
        unsafe {
            b[1].as_mut_ptr().write(10);
            b[2].as_mut_ptr().write(20);
        }

        let slice = unsafe { b[1..3].assume_init_ref() };
        assert_eq!(slice, [10, 20]);

        let slice_mut = unsafe { b[1..3].assume_init_mut() };
        slice_mut[0] = 30;
        assert_eq!(unsafe { b[1].assume_init() }, 30);
    }

    #[derive(Clone)]
    struct TrackingAllocator {
        allocated: alloc::sync::Arc<core::cell::RefCell<alloc::collections::BTreeSet<usize>>>,
    }

    impl TrackingAllocator {
        fn new() -> Self {
            Self {
                allocated: alloc::sync::Arc::new(core::cell::RefCell::new(
                    alloc::collections::BTreeSet::new(),
                )),
            }
        }
    }

    impl Allocator for TrackingAllocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            let ptr = DefaultAllocator::default().allocate(layout)?;
            let addr = ptr.as_ptr() as *mut u8 as usize;
            self.allocated.borrow_mut().insert(addr);
            Ok(ptr)
        }

        fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            let ptr = DefaultAllocator::default().allocate_zeroed(layout)?;
            let addr = ptr.as_ptr() as *mut u8 as usize;
            self.allocated.borrow_mut().insert(addr);
            Ok(ptr)
        }

        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
            let addr = ptr.as_ptr() as usize;
            let mut allocated = self.allocated.borrow_mut();
            if !allocated.remove(&addr) {
                panic!("Deallocate called on address not produced by allocator: {:p}", ptr);
            }
            // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
            unsafe {
                DefaultAllocator::default().deallocate(ptr, layout);
            }
        }

        unsafe fn grow(
            &self,
            ptr: NonNull<u8>,
            old_layout: Layout,
            new_layout: Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let addr = ptr.as_ptr() as usize;
            let mut allocated = self.allocated.borrow_mut();
            if !allocated.remove(&addr) {
                panic!("Grow called on address not produced by allocator: {:p}", ptr);
            }
            // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
            let new_ptr = unsafe { DefaultAllocator::default().grow(ptr, old_layout, new_layout)? };
            let new_addr = new_ptr.as_ptr() as *mut u8 as usize;
            allocated.insert(new_addr);
            Ok(new_ptr)
        }

        unsafe fn shrink(
            &self,
            ptr: NonNull<u8>,
            old_layout: Layout,
            new_layout: Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let addr = ptr.as_ptr() as usize;
            let mut allocated = self.allocated.borrow_mut();
            if !allocated.remove(&addr) {
                panic!("Shrink called on address not produced by allocator: {:p}", ptr);
            }
            // SAFETY: The caller must guarantee that `ptr` was allocated by this allocator.
            let new_ptr =
                unsafe { DefaultAllocator::default().shrink(ptr, old_layout, new_layout)? };
            let new_addr = new_ptr.as_ptr() as *mut u8 as usize;
            allocated.insert(new_addr);
            Ok(new_ptr)
        }
    }

    #[test]
    fn test_empty_slice_and_zst_allocator_interactions() {
        let alloc = TrackingAllocator::new();

        // Test empty slice
        {
            let b = Box::<[u32], TrackingAllocator>::empty_slice_in(alloc.clone());
            assert_eq!(b.len(), 0);
            // Drop should NOT call deallocate because length is 0.
        }

        // Test ZST
        struct Zst;
        {
            let _b = Box::<Zst, TrackingAllocator>::try_new_in(Zst, alloc.clone()).unwrap();
            // Drop should NOT call deallocate because size is 0.
        }

        // Test growing an empty slice
        {
            let mut b = Box::<[core::mem::MaybeUninit<u32>], TrackingAllocator>::empty_slice_in(
                alloc.clone(),
            );
            Box::try_grow(&mut b, 5).unwrap();
            // Now it should be in the tracking allocator.
            let addr = Box::as_ptr(&b) as *mut u8 as usize;
            assert!(alloc.allocated.borrow().contains(&addr));
        } // Drops here, should call deallocate and succeed.

        // Test shrinking to empty
        {
            let mut b =
                Box::<[u32], TrackingAllocator>::try_new_uninit_slice_in(5, alloc.clone()).unwrap();
            let addr = Box::as_ptr(&b) as *mut u8 as usize;
            assert!(alloc.allocated.borrow().contains(&addr));

            unsafe {
                Box::try_shrink(&mut b, 0).unwrap();
            }
            assert_eq!(b.len(), 0);
            // The old memory should have been deallocated by try_shrink calling deallocate_slice.
            assert!(!alloc.allocated.borrow().contains(&addr));
        }
    }

    #[test]
    fn test_try_new_zeroed_uninit_slice() {
        // Zeroed slice of u32 (which implements FromZeros)
        let b = Box::<[u32]>::try_new_zeroed_uninit_slice(4).unwrap();
        assert_eq!(b.len(), 4);
        for x in b.as_ref() {
            assert_eq!(unsafe { x.assume_init() }, 0);
        }

        // Zeroed slice of a struct that does not implement FromZeros
        struct NotZeroable {
            _a: u32,
            _b: u32,
        }
        let b = Box::<[NotZeroable]>::try_new_zeroed_uninit_slice(4).unwrap();
        assert_eq!(b.len(), 4);
    }
}
