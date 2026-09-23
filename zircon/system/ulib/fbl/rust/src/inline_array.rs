// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use kalloc::{AllocError, Allocator, Box, DefaultAllocator};

/// Runtime-determined, fixed size arrays that are "inlined" (e.g., on the stack) if the size at
/// most `N` or heap-allocated otherwise.
///
/// All allocations are explicit and fallible.
///
/// # Note on memory layout This type does not precisely match the C++ `InlineArray` because it
/// avoids using a self-referential pointer for the inline case to remain address-insensitive
/// (movable). We can fix this issue to match the C++ layout precisely once we have the `pin_init`
/// crate available.
#[repr(C)]
pub struct InlineArray<T, const N: usize, A: Allocator = DefaultAllocator> {
    /// The number of elements in the array.  Invariant: If `count <= N`, elements are stored in
    /// `inline_storage`.  If `count > N`, elements are stored on the heap, pointed to by `ptr`.
    count: usize,

    /// Pointer to the data.
    ///
    /// Invariant: If `count <= N` (inline mode), this pointer is null.  Unlike C++ which points to
    /// `inline_storage`, we avoid pointing to `inline_storage` to keep the type address-insensitive
    /// (movable).  If `count > N` (heap mode), this points to the heap-allocated storage.
    ptr: *mut T,

    /// Inline storage used when `count <= N`.
    ///
    /// Invariant: The first `count` elements are initialized when `count <= N`.
    inline_storage: [MaybeUninit<T>; N],

    /// The allocator used for heap allocations.
    ///
    /// Note: This takes space if `A` is not a Zero Sized Type (ZST).
    allocator: A,
}

zr::static_assert!(core::mem::size_of::<InlineArray<u32, 4>>() == 32);
zr::static_assert!(core::mem::align_of::<InlineArray<u32, 4>>() == 8);

impl<T, const N: usize, A: Allocator> InlineArray<T, N, A> {
    /// Tries to create a new `InlineArray` of the given length with a specific allocator,
    /// initializing elements with the provided closure.
    pub fn try_new_in_with<F>(count: usize, allocator: A, mut f: F) -> Result<Self, AllocError>
    where
        F: FnMut() -> T,
    {
        if count <= N {
            let mut inline_storage = [const { MaybeUninit::uninit() }; N];
            for i in 0..count {
                inline_storage[i].write(f());
            }
            Ok(InlineArray { count, ptr: core::ptr::null_mut(), inline_storage, allocator })
        } else {
            let mut heap_data = Box::try_new_uninit_slice_in(count, allocator)?;
            for i in 0..count {
                heap_data[i].write(f());
            }
            // SAFETY: We just initialized all the values.
            let (fat_ptr, allocator) =
                Box::into_raw_with_allocator(unsafe { heap_data.assume_init() });
            let ptr = fat_ptr as *mut T;
            Ok(InlineArray {
                count,
                ptr,
                inline_storage: [const { MaybeUninit::uninit() }; N],
                allocator,
            })
        }
    }

    /// Tries to create a new `InlineArray` of the given length with a specific allocator.
    /// Elements are initialized using `T::default()`.
    pub fn try_new_in(count: usize, allocator: A) -> Result<Self, AllocError>
    where
        T: Default,
    {
        Self::try_new_in_with(count, allocator, T::default)
    }

    /// Returns true if the array is stored inline.
    const fn is_inline(&self) -> bool {
        self.count <= N
    }

    /// Returns the number of elements in the array.
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Returns true if the array is empty.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns a shared reference to the array as a slice.
    pub fn as_slice(&self) -> &[T] {
        if self.is_inline() {
            // SAFETY: The first `self.count` elements are initialized.
            unsafe { self.inline_storage[..self.count].assume_init_ref() }
        } else {
            // SAFETY: `self.ptr` points to valid heap data of size `self.count`.
            unsafe { core::slice::from_raw_parts(self.ptr, self.count) }
        }
    }

    /// Returns a mutable reference to the array as a slice.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.is_inline() {
            // SAFETY: The first `self.count` elements are initialized.
            unsafe { self.inline_storage[..self.count].assume_init_mut() }
        } else {
            // SAFETY: `self.ptr` points to valid heap data of size `self.count`.
            unsafe { core::slice::from_raw_parts_mut(self.ptr, self.count) }
        }
    }
}

impl<T, const N: usize> InlineArray<T, N, DefaultAllocator> {
    /// Tries to create a new `InlineArray` of the given length using the default allocator.
    /// Elements are initialized using `T::default()`.
    pub fn try_new(count: usize) -> Result<Self, AllocError>
    where
        T: Default,
    {
        Self::try_new_in(count, DefaultAllocator)
    }

    /// Tries to create a new `InlineArray` of the given length using the default allocator,
    /// initializing elements with the provided closure.
    pub fn try_new_with<F>(count: usize, f: F) -> Result<Self, AllocError>
    where
        F: FnMut() -> T,
    {
        Self::try_new_in_with(count, DefaultAllocator, f)
    }
}

impl<T, const N: usize, A: Allocator> Deref for InlineArray<T, N, A> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T, const N: usize, A: Allocator> DerefMut for InlineArray<T, N, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<T, const N: usize, A: Allocator> Drop for InlineArray<T, N, A> {
    fn drop(&mut self) {
        if self.is_inline() {
            unsafe {
                self.inline_storage[..self.count].assume_init_drop();
            }
        } else {
            // Reconstruct box and drop it
            // SAFETY: In non-inline mode, ptr is guaranteed to be valid and non-null because we
            // only set it on successful allocation in try_new_in.
            unsafe {
                let _ = Box::from_raw_in(
                    core::ptr::slice_from_raw_parts_mut(self.ptr, self.count),
                    self.allocator.clone(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use core::ptr::NonNull;

    #[derive(Debug, PartialEq, Eq)]
    struct TestState {
        live_obj_count: Cell<usize>,
        ctor_count: Cell<usize>,
        dtor_count: Cell<usize>,
        alloc_count: Cell<usize>,
        fail_threshold: Cell<usize>,
    }

    impl Default for TestState {
        fn default() -> Self {
            Self {
                live_obj_count: Cell::new(0),
                ctor_count: Cell::new(0),
                dtor_count: Cell::new(0),
                alloc_count: Cell::new(0),
                fail_threshold: Cell::new(usize::MAX),
            }
        }
    }

    #[derive(Clone)]
    struct TestAllocator<'a> {
        state: &'a TestState,
    }

    impl<'a> Allocator for TestAllocator<'a> {
        fn allocate(&self, layout: core::alloc::Layout) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            DefaultAllocator::default().allocate(layout)
        }

        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: core::alloc::Layout) {
            unsafe { DefaultAllocator::default().deallocate(ptr, layout) }
        }

        unsafe fn grow(
            &self,
            ptr: NonNull<u8>,
            old_layout: core::alloc::Layout,
            new_layout: core::alloc::Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            unsafe { DefaultAllocator::default().grow(ptr, old_layout, new_layout) }
        }

        unsafe fn shrink(
            &self,
            ptr: NonNull<u8>,
            old_layout: core::alloc::Layout,
            new_layout: core::alloc::Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            unsafe { DefaultAllocator::default().shrink(ptr, old_layout, new_layout) }
        }

        fn allocate_zeroed(
            &self,
            layout: core::alloc::Layout,
        ) -> Result<NonNull<[u8]>, AllocError> {
            let current = self.state.alloc_count.get();
            self.state.alloc_count.set(current + 1);
            if current >= self.state.fail_threshold.get() {
                return Err(AllocError);
            }
            DefaultAllocator::default().allocate_zeroed(layout)
        }
    }

    #[derive(Debug)]
    struct TestObject<'a> {
        state: &'a TestState,
    }

    impl<'a> TestObject<'a> {
        fn new(state: &'a TestState) -> Self {
            state.live_obj_count.set(state.live_obj_count.get() + 1);
            state.ctor_count.set(state.ctor_count.get() + 1);
            TestObject { state }
        }
    }

    impl<'a> Drop for TestObject<'a> {
        fn drop(&mut self) {
            self.state.live_obj_count.set(self.state.live_obj_count.get() - 1);
            self.state.dtor_count.set(self.state.dtor_count.get() + 1);
        }
    }

    #[test]
    fn test_inline() {
        let state = TestState::default();

        for sz in 0..=3 {
            state.ctor_count.set(0);
            state.dtor_count.set(0);
            {
                let ia =
                    InlineArray::<TestObject<'_>, 3>::try_new_with(sz, || TestObject::new(&state))
                        .unwrap();
                assert_eq!(ia.len(), sz);
            }
            assert_eq!(state.ctor_count.get(), sz);
            assert_eq!(state.dtor_count.get(), sz);
        }
    }

    #[test]
    fn test_non_inline() {
        let state = TestState::default();

        let test_sizes = [4, 5, 6, 10, 100];

        for &sz in &test_sizes {
            state.ctor_count.set(0);
            state.dtor_count.set(0);
            {
                let ia =
                    InlineArray::<TestObject<'_>, 3>::try_new_with(sz, || TestObject::new(&state))
                        .unwrap();
                assert_eq!(ia.len(), sz);
            }
            assert_eq!(state.ctor_count.get(), sz);
            assert_eq!(state.dtor_count.get(), sz);
        }
    }

    #[test]
    fn test_allocation_failure() {
        let state = TestState::default();
        state.fail_threshold.set(0); // Fail immediately

        // Inline allocation should still work!
        let ia = InlineArray::<u32, 3, TestAllocator<'_>>::try_new_in(
            3,
            TestAllocator { state: &state },
        );
        assert!(ia.is_ok());

        // Heap allocation should fail!
        let ia = InlineArray::<u32, 3, TestAllocator<'_>>::try_new_in(
            4,
            TestAllocator { state: &state },
        );
        assert!(ia.is_err());
    }
}
