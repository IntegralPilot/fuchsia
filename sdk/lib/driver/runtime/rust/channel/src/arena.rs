// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for the driver runtime arena stable ABI

use core::alloc::Layout;
use core::cmp::max;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr::{NonNull, null_mut, slice_from_raw_parts_mut};
use std::sync::{Arc, Weak};

use zx::Status;

use fdf_sys::*;

pub use fdf_sys::fdf_arena_t;

/// Implements a memory arena allocator to be used with the Fuchsia Driver
/// Runtime when sending and receiving from channels.
#[derive(Debug)]
pub struct Arena(pub(crate) NonNull<fdf_arena_t>);

// SAFETY: The api for `fdf_arena_t` is thread safe
unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

impl Arena {
    /// Allocates a new arena for use with the driver runtime
    pub fn new() -> Self {
        let mut arena = null_mut();
        // SAFETY: the address we pass to fdf_arena_create is allocated on
        // the stack and appropriately sized.
        // This call cannot fail as the only reason it would fail is due to invalid
        // option flags, and 0 is a valid option.
        Status::ok(unsafe { fdf_arena_create(0, 0, &mut arena) }).expect("Failed to create arena");
        // SAFETY: if fdf_arena_create returned ZX_OK, it will have placed
        // a non-null pointer.
        Arena(unsafe { NonNull::new_unchecked(arena) })
    }

    /// Creates an arena from a raw pointer to the arena object.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that only one [`Arena`]
    /// is constructed from this pointer, and that is has not previously
    /// been freed.
    pub unsafe fn from_raw(ptr: NonNull<fdf_arena_t>) -> Self {
        Self(ptr)
    }

    /// Returns true if the allocation pointed to was made by this arena
    pub fn contains_ptr<T: ?Sized>(&self, ptr: &T) -> bool {
        // SAFETY: self.0 is valid as constructed, and `fdf_arena_contains` does not access data at the
        // pointer but just compares its pointer value to the buffers in the arena.
        unsafe {
            fdf_arena_contains(self.0.as_ptr(), ptr as *const _ as *const _, size_of_val(ptr))
        }
    }

    /// Returns true if the allocation was made by this arena
    pub fn contains<T: ?Sized>(&self, item: &ArenaBox<'_, T>) -> bool {
        self.contains_ptr(ArenaBox::deref(item))
    }

    /// Allocates the appropriate amount of memory for the given layout and
    /// returns a pointer to `T` at the start of that memory.
    ///
    /// # Safety
    ///
    /// The caller is responsible for making sure that the `Layout` is laid out
    /// properly for one or more `T` to be stored at it. This may be a single
    /// object or a slice of them, but it must be a multiple of it.
    unsafe fn alloc_bytes_for<T>(&self, layout: Layout) -> NonNull<T> {
        // We make sure we allocate at least one byte so we return a unique
        // pointer that is within the arena, which will ensure that subsequent
        // verifications that the memory location is in the arena will pass.
        let bytes = max(layout.size(), 1);
        // SAFETY: Allocating a block of memory in the arena large enough to store
        // the object we're allocating.
        let storage =
            unsafe { NonNull::new_unchecked(fdf_arena_allocate(self.0.as_ptr(), bytes) as *mut T) };
        // TODO(b/352119228): when the arena allocator allows specifying alignment, use that
        // instead of asserting the alignment after the fact.
        assert_eq!(
            storage.align_offset(layout.align()),
            0,
            "Arena returned an improperly aligned pointer: {}",
            core::any::type_name::<T>(),
        );
        storage
    }

    /// Inserts a [`MaybeUninit`] object and returns the [`ArenaBox`] of it.
    pub fn insert_uninit<T: Sized>(&self) -> ArenaBox<'_, MaybeUninit<T>> {
        let layout = Layout::new::<MaybeUninit<T>>();
        // SAFETY: The layout we're passing to `alloc_bytes_for` is for zero or
        // more objects of type `T`, which is the pointer type we get back from
        // it.
        unsafe { ArenaBox::new(self.alloc_bytes_for(layout)) }
    }

    /// Inserts a slice of [`MaybeUninit`] objects of len `len`
    ///
    /// # Panics
    ///
    /// Panics if an array `[T; n]` is too large to be allocated.
    pub fn insert_uninit_slice<T: Sized>(&self, len: usize) -> ArenaBox<'_, [MaybeUninit<T>]> {
        let layout = Layout::array::<MaybeUninit<T>>(len).expect("allocation too large");
        // SAFETY: The layout we're passing to `alloc_bytes_for` is for zero or
        // more objects of type `T`, which is the pointer type we get back from
        // it.
        let storage = unsafe { self.alloc_bytes_for(layout) };
        // At this point we have a `*mut T` but we need to return a `[T]`,
        // which is unsized. We need to use [`slice_from_raw_parts_mut`]
        // to construct the unsized pointer from the data and its length.
        let ptr = slice_from_raw_parts_mut(storage.as_ptr(), len);
        // SAFETY: alloc_bytes_for is expected to return a valid pointer.
        unsafe { ArenaBox::new(NonNull::new_unchecked(ptr)) }
    }

    /// Moves `obj` of type `T` into the arena and returns an [`ArenaBox`]
    /// containing the moved value.
    pub fn insert<T: Sized>(&self, obj: T) -> ArenaBox<'_, T> {
        let mut uninit = self.insert_uninit();
        uninit.write(obj);
        // SAFETY: we wrote `obj` to the object
        unsafe { uninit.assume_init() }
    }

    /// Moves a [`Box`]ed slice into the arena and returns an [`ArenaBox`]
    /// containing the moved value.
    pub fn insert_boxed_slice<T: Sized>(&self, slice: Box<[T]>) -> ArenaBox<'_, [T]> {
        let layout = Layout::for_value(&*slice);
        let len = slice.len();
        // SAFETY: The layout we give `alloc_bytes_for` is for storing 0 or more
        // objects of type `T`, which is the pointer type we get from it.
        let storage = unsafe { self.alloc_bytes_for(layout) };
        let original_storage = Box::into_raw(slice);
        // SAFETY: Moving the object into the arena memory we just allocated by
        // first copying the bytes over and then deallocating the raw memory
        // we took from the box.
        let slice_box = unsafe {
            core::ptr::copy_nonoverlapping(original_storage as *mut T, storage.as_ptr(), len);
            let slice_ptr = slice_from_raw_parts_mut(storage.as_ptr(), len);
            ArenaBox::new(NonNull::new_unchecked(slice_ptr))
        };
        if layout.size() != 0 {
            // SAFETY: Since we have decomposed the Box we have to deallocate it,
            // but only if it's not dangling.
            unsafe {
                std::alloc::dealloc(original_storage as *mut u8, layout);
            }
        }
        slice_box
    }

    /// Copies the slice into the arena and returns an [`ArenaBox`] containing
    /// the copied values.
    pub fn insert_slice<T: Sized + Clone>(&self, slice: &[T]) -> ArenaBox<'_, [T]> {
        let len = slice.len();
        let mut uninit_slice = self.insert_uninit_slice(len);
        for (from, to) in slice.iter().zip(uninit_slice.iter_mut()) {
            to.write(from.clone());
        }

        // SAFETY: we wrote `from.clone()` to each item of the slice.
        unsafe { uninit_slice.assume_init_slice() }
    }

    /// Inserts a slice of [`Default`]-initialized objects of type `T` to the
    /// arena and returns an [`ArenaBox`] of it.
    ///
    /// # Panics
    ///
    /// Panics if an array `[T; n]` is too large to be allocated.
    pub fn insert_default_slice<T: Sized + Default>(&self, len: usize) -> ArenaBox<'_, [T]> {
        let mut uninit_slice = self.insert_uninit_slice(len);
        for i in uninit_slice.iter_mut() {
            i.write(T::default());
        }
        // SAFETY: we wrote `T::default()` to each item of the slice.
        unsafe { uninit_slice.assume_init_slice() }
    }

    /// Returns an ArenaBox for the pointed to object, assuming that it is part
    /// of this arena.
    ///
    /// # Safety
    ///
    /// This does not verify that the pointer came from this arena,
    /// so the caller is responsible for verifying that.
    pub unsafe fn assume_unchecked<T: ?Sized>(&self, ptr: NonNull<T>) -> ArenaBox<'_, T> {
        // SAFETY: Caller is responsible for ensuring this per safety doc section.
        unsafe { ArenaBox::new(ptr) }
    }

    /// Returns an [`ArenaBox`] for the pointed to object, verifying that it
    /// is a part of this arena in the process.
    ///
    /// # Panics
    ///
    /// This function panics if the given pointer is not in this [`Arena`].
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that only one [`ArenaBox`] is constructed
    /// for a given pointer, and that the pointer originated from an `ArenaBox<T>` or
    /// a direct allocation with the arena through [`fdf_arena_allocate`], and is:
    /// - initialized to a value of `T`.
    /// - properly aligned for `T`.
    /// - pointing to the beginning of the object, and not to a subfield of another
    ///   [`ArenaBox`]ed object.
    pub unsafe fn assume<T: ?Sized>(&self, ptr: NonNull<T>) -> ArenaBox<'_, T> {
        // SAFETY: caller promises the pointer is initialized and valid
        assert!(
            self.contains_ptr(unsafe { ptr.as_ref() }),
            "Arena can't assume ownership over a pointer not allocated from within it"
        );
        // SAFETY: we will verify the provenance below
        unsafe { self.assume_unchecked(ptr) }
    }

    /// Moves the given [`ArenaBox`] into an [`ArenaRc`] with an owned
    /// reference to this [`Arena`], allowing for it to be used in `'static`
    /// contexts.
    ///
    /// # Panics
    ///
    /// Panics if the given [`ArenaBox`] is not allocated from this arena.
    pub fn make_rc<T: ?Sized>(&self, data: ArenaBox<'_, T>) -> ArenaRc<T> {
        assert!(self.contains(&data), "Arena doesn't own the ArenaBox");
        // SAFETY: we just checked the box is owned by this arena.
        unsafe { ArenaRc::new_unchecked(self.clone(), data) }
    }

    /// Moves the given [`ArenaBox`] into an [`ArenaStaticBox`] with an owned
    /// reference to this [`Arena`], allowing for it to be used in `'static`
    /// contexts.
    ///
    /// # Panics
    ///
    /// Panics if the given [`ArenaBox`] is not allocated from this arena.
    pub fn make_static<T: ?Sized>(&self, data: ArenaBox<'_, T>) -> ArenaStaticBox<T> {
        assert!(self.contains(&data), "Arena doesn't own the ArenaBox");
        // SAFETY: we just checked the box is owned by this arena.
        unsafe { ArenaStaticBox::new_unchecked(self.clone(), data) }
    }

    /// Creates an [`ArenaBox`]ed slice from an iterator implementing [`ExactSizeIterator`]. Note
    /// that if [`ExactSizeIterator::len`] returns an incorrect value, the returned [`ArenaBox`]
    /// will be no more than the length returned, and may be less.
    pub fn insert_from_iter<I: IntoIterator>(&self, source: I) -> ArenaBox<'_, [I::Item]>
    where
        I::IntoIter: ExactSizeIterator,
    {
        let iter = source.into_iter();
        let len = iter.len();
        let mut actual_len = 0;
        let mut storage = self.insert_uninit_slice(len);
        for (output, input) in storage.iter_mut().zip(iter) {
            output.write(input);
            actual_len += 1;
        }
        // SAFETY: we wrote to `actual_len` elements of the storage
        unsafe { ArenaBox::assume_init_slice_len(storage, actual_len) }
    }

    /// Tries to create an [`ArenaBox`]ed slice from an iterator implementing [`ExactSizeIterator`].
    /// Note that if [`ExactSizeIterator::len`] returns an incorrect value, the returned
    /// [`ArenaBox`] will be no more than the length returned, and may be less.
    ///
    /// If any item returned by the iterator returns an Err() result, results so far are discarded
    pub fn try_insert_from_iter<I, T, E>(&self, source: I) -> Result<ArenaBox<'_, [T]>, E>
    where
        I: IntoIterator<Item = Result<T, E>>,
        I::IntoIter: ExactSizeIterator,
    {
        let iter = source.into_iter();
        let len = iter.len();
        let mut actual_len = 0;
        let mut storage = self.insert_uninit_slice(len);
        for (output, input) in storage.iter_mut().zip(iter) {
            match input {
                Ok(input) => {
                    output.write(input);
                    actual_len += 1;
                }
                Err(e) => {
                    // `assume_init` the slice so far so that drop handlers are properly called on the
                    // items already moved. This will be dropped immediately.
                    // SAFETY: `actual_len` will be the length of moved values into the slice so far.
                    unsafe { ArenaBox::assume_init_slice_len(storage, actual_len) };
                    return Err(e);
                }
            }
        }
        // SAFETY: we wrote to `actual_len` elements of the storage
        Ok(unsafe { ArenaBox::assume_init_slice_len(storage, actual_len) })
    }

    /// Transforms this Arena into an fdf_arena_t without dropping the reference.
    ///
    /// If the caller drops the returned fdf_arena_t, the memory allocated by the
    /// arena will never be freed.
    pub fn into_raw(self) -> NonNull<fdf_arena_t> {
        let res = self.0;
        core::mem::forget(self);
        res
    }
}

impl Clone for Arena {
    fn clone(&self) -> Self {
        // SAFETY: We own this arena reference and so we can add ref it
        unsafe { fdf_arena_add_ref(self.0.as_ptr()) }
        Self(self.0)
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // SAFETY: We own this arena reference and so we can drop it.
        unsafe { fdf_arena_drop_ref(self.0.as_ptr()) }
    }
}

/// Holds a reference to data of type `T` in an [`Arena`] with lifetime `'a`,
/// and ensures that the object is properly dropped before the [`Arena`] goes
/// out of scope.
#[derive(Debug)]
pub struct ArenaBox<'a, T: ?Sized>(NonNull<T>, PhantomData<&'a Arena>);

/// SAFETY: [`ArenaBox`] impls [`Send`] and [`Sync`] if `T` impls them.
unsafe impl<'a, T: ?Sized> Send for ArenaBox<'a, T> where T: Send {}
unsafe impl<'a, T: ?Sized> Sync for ArenaBox<'a, T> where T: Sync {}

impl<'a, T> ArenaBox<'a, T> {
    /// Moves the inner value of this ArenaBox out to owned storage.
    pub fn take(value: Self) -> T {
        // SAFETY: `Self::into_ptr` will forget `value` and prevent
        // calling its `drop`.
        unsafe { core::ptr::read(Self::into_ptr(value).as_ptr()) }
    }

    /// Moves the inner value of this ArenaBox out into a [`Box`] using the
    /// global allocator. Using this instead of `Box::new(ArenaBox::take(v))`
    /// helps to avoid any additional copies of the storage on its way to the
    /// box.
    ///
    /// Note: if you want to take a slice, you will need to use
    /// [`Self::take_boxed_slice`].
    pub fn take_boxed(value: Self) -> Box<T> {
        // SAFETY: we are allocating space for `T` with the layout of `T`, so
        // this is simple.
        let storage = unsafe { global_alloc(Layout::for_value(&*value)) };
        // SAFETY: storage is sufficiently large to store the value in `value`
        // and we used Layout to make sure that Box will be happy with its
        // layout.
        unsafe {
            core::ptr::write(storage.as_ptr(), Self::take(value));
            Box::from_raw(storage.as_ptr())
        }
    }
}

impl<'a, T> ArenaBox<'a, MaybeUninit<T>> {
    /// Assumes the contents of this [`MaybeUninit`] box are initialized now.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the value is initialized
    /// properly. See [`MaybeUninit::assume_init`] for more details on the
    /// safety requirements of this.
    pub unsafe fn assume_init(self) -> ArenaBox<'a, T> {
        // SAFETY: This pointer came from an `ArenaBox` we just leaked,
        // and casting `*MaybeUninit<T>` to `*T` is safe.
        unsafe { ArenaBox::new(ArenaBox::into_ptr(self).cast()) }
    }
}

impl<'a, T> ArenaBox<'a, [MaybeUninit<T>]> {
    /// Assumes the contents of this box of `[MaybeUninit<T>]` are initialized now.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the value is initialized
    /// properly. See [`MaybeUninit::assume_init`] for more details on the
    /// safety requirements of this.
    pub unsafe fn assume_init_slice(self) -> ArenaBox<'a, [T]> {
        let len = self.len();
        // SAFETY: We are about to reconstitute this pointer back into
        // a new `ArenaBox` with the same lifetime, and casting
        // `MaybeUninit<T>` to `T` is safe.
        let data: NonNull<T> = unsafe { ArenaBox::into_ptr(self) }.cast();
        let slice_ptr = NonNull::slice_from_raw_parts(data, len);

        // SAFETY: We just got this pointer from an `ArenaBox` we decomposed.
        unsafe { ArenaBox::new(slice_ptr) }
    }

    /// Assumes the contents of this box of `[MaybeUninit<T>]` are initialized now,
    /// up to `len` elements and ignores the rest.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the value is initialized
    /// properly. See [`MaybeUninit::assume_init`] for more details on the
    /// safety requirements of this.
    pub unsafe fn assume_init_slice_len(self, len: usize) -> ArenaBox<'a, [T]> {
        // only use up to `len` elements of the slice.
        let len = self.len().min(len);
        // SAFETY: We are about to reconstitute this pointer back into
        // a new `ArenaBox` with the same lifetime, and casting
        // `MaybeUninit<T>` to `T` is safe.
        let data: NonNull<T> = unsafe { ArenaBox::into_ptr(self) }.cast();
        let slice_ptr = NonNull::slice_from_raw_parts(data, len);

        // SAFETY: We just got this pointer from an `ArenaBox` we decomposed.
        unsafe { ArenaBox::new(slice_ptr) }
    }
}

impl<'a, T> ArenaBox<'a, [T]> {
    /// Like [`Self::take_boxed`], this moves the inner value of this ArenaBox
    /// out into a [`Box`] using the global allocator, and using it avoids
    /// additional copies of the data, but this function works on slices of `T`,
    /// which are unsized and so require special handling.
    pub fn take_boxed_slice(value: Self) -> Box<[T]> {
        let len = value.len();
        // SAFETY: we are using the layout of the slice value of type `[T]` to
        // allocate a pointer to the first element of the storage for the new
        // slice, which is of type `T`.
        let storage = unsafe { global_alloc(Layout::for_value(&*value)) };
        // SAFETY: storage is sufficiently large to store the slice in `value`
        let slice_ptr = unsafe {
            core::ptr::copy_nonoverlapping(
                Self::into_ptr(value).as_ptr() as *mut T,
                storage.as_ptr(),
                len,
            );
            core::ptr::slice_from_raw_parts_mut(storage.as_ptr(), len)
        };
        // SAFETY: we used Layout to make sure that Box will be happy with the
        // layout of the stored value.
        unsafe { Box::from_raw(slice_ptr) }
    }
}

impl<'a, T: ?Sized> ArenaBox<'a, T> {
    /// Creates a new [`ArenaBox`] from the given non-null pointer to an object of type `T`.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the object pointed to came from an [`Arena`]
    /// and that the lifetime of this box is less than the lifetime of that [`Arena`].
    pub unsafe fn new(obj: NonNull<T>) -> ArenaBox<'a, T> {
        Self(obj, PhantomData)
    }

    /// Decomposes this [`ArenaBox`] into its pointer.
    ///
    /// # Safety
    ///
    /// This is unsafe because it loses the lifetime of the [`Arena`] it
    /// came from. The caller must make sure to not let the pointer outlive the
    /// arena. The caller is also responsible for making sure the object is
    /// dropped before the [`Arena`], or it may leak resources.
    pub unsafe fn into_ptr(value: Self) -> NonNull<T> {
        let res = value.0;
        core::mem::forget(value);
        res
    }

    /// Turns this [`ArenaBox`] into one with the given lifetime.
    ///
    /// # Safety
    ///
    /// This is unsafe because it loses the lifetime of the [`Arena`] it
    /// came from. The caller must make sure to not let the
    /// [`ArenaBox`] outlive the [`Arena`] it was created from. The caller
    /// is also responsible for making sure the object is dropped before
    /// the [`Arena`], or it may leak resources.
    pub unsafe fn erase_lifetime(value: Self) -> ArenaBox<'static, T> {
        // SAFETY: the caller promises to ensure this object does not
        // outlive the arena.
        unsafe { ArenaBox::new(ArenaBox::into_ptr(value)) }
    }

    /// Consumes and leaks this [`ArenaBox`], returning a mutable reference
    /// to its contents.
    pub fn leak(mut this: Self) -> &'a mut T {
        let res = unsafe { this.0.as_mut() };
        core::mem::forget(this);
        res
    }
}

impl<'a> ArenaBox<'a, [MaybeUninit<u8>]> {
    /// Transforms the [`ArenaBox`] into an `ArenaBox<T>`.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the contents of this
    /// [`ArenaBox`] originated from a source with a properly allocated `T` with correct
    /// alignment
    pub unsafe fn cast_unchecked<T>(this: Self) -> ArenaBox<'a, T> {
        let ptr = this.0.cast();
        // Ensure we don't drop the original `ArenaBox`.
        core::mem::forget(this);
        // SAFETY: caller promises this is the correct type
        unsafe { ArenaBox::new(ptr) }
    }
}

impl<'a, T: ?Sized> Drop for ArenaBox<'a, T> {
    fn drop(&mut self) {
        // SAFETY: Since this value is allocated in the arena, and the arena
        // will not drop the value, and ArenaBox can't be cloned, this ArenaBox
        // owns the value and can drop it.
        unsafe { core::ptr::drop_in_place(self.0.as_ptr()) }
    }
}

impl<T: ?Sized> Deref for ArenaBox<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        // SAFETY: As these methods are the only way to get a reference to the
        // contents of the ArenaBox, rust will enforce the aliasing rules
        // of the contents of the inner `NonZero` object.
        unsafe { self.0.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for ArenaBox<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: As these methods are the only way to get a reference to the
        // contents of the ArenaBox, rust will enforce the aliasing rules
        // of the contents of the inner `NonZero` object.
        unsafe { self.0.as_mut() }
    }
}

impl<'a, T: 'a> IntoIterator for ArenaBox<'a, [T]> {
    type IntoIter = IntoIter<T, PhantomData<&'a Arena>>;
    type Item = T;

    fn into_iter(self) -> Self::IntoIter {
        let len = self.len();
        let ptr = self.0.cast();
        // SAFETY: we will never dereference `end`
        let end = unsafe { ptr.add(len) };
        // the IntoIter now owns the data, so we don't want to drop them here.
        core::mem::forget(self);
        IntoIter { ptr, end, _arena: PhantomData }
    }
}

/// The implementation for an [`IntoIterator`] of an [`ArenaBox`] of a slice that manages the
/// memory behind it and ensures that it's cleaned up.
pub struct IntoIter<T, A> {
    ptr: NonNull<T>,
    end: NonNull<T>,
    _arena: A,
}

impl<T, A> Iterator for IntoIter<T, A> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            return None;
        }
        // SAFETY: all items from `ptr` to `end-1` are valid until moved out.
        unsafe {
            let res = self.ptr.read();
            self.ptr = self.ptr.add(1);
            Some(res)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(self.len()))
    }
}

impl<T, A> ExactSizeIterator for IntoIter<T, A> {
    fn len(&self) -> usize {
        // SAFETY: end is always >= ptr
        unsafe { self.end.offset_from(self.ptr) as usize }
    }
}

impl<T, A> Drop for IntoIter<T, A> {
    fn drop(&mut self) {
        // go through and read all remaining items to drop them
        while self.ptr != self.end {
            // SAFETY: all items from `ptr` to `end-1` are valid until moved out.
            unsafe {
                drop(self.ptr.read());
                self.ptr = self.ptr.add(1);
            }
        }
    }
}

/// An equivalent to [`ArenaBox`] that holds onto a reference to the
/// arena to allow it to have static lifetime.
#[derive(Debug)]
pub struct ArenaStaticBox<T: ?Sized> {
    data: ArenaBox<'static, T>,
    // Safety Note: it is important that this be last in the struct so that it is
    // guaranteed to be freed after the [`ArenaBox`].
    arena: Arena,
}

/// SAFETY: [`ArenaStaticBox`] impls [`Send`] and [`Sync`] if `T` impls them.
unsafe impl<T: ?Sized> Send for ArenaStaticBox<T> where T: Send {}
unsafe impl<T: ?Sized> Sync for ArenaStaticBox<T> where T: Sync {}

impl<T: ?Sized> ArenaStaticBox<T> {
    /// Transforms the given [`ArenaBox`] into an [`ArenaStaticBox`] with an owned
    /// reference to the given [`Arena`], allowing for it to be used in `'static`
    /// contexts.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the given [`ArenaBox`] is owned by this
    /// arena, or it may result in use-after-free.
    pub unsafe fn new_unchecked(arena: Arena, data: ArenaBox<'_, T>) -> ArenaStaticBox<T> {
        // SAFETY: The `ArenaBox` will not outlive the `Arena` as it is owned
        // by the current struct and can't be moved out.
        let data = unsafe { ArenaBox::erase_lifetime(data) };
        Self { data, arena }
    }

    /// Takes ownership over the arena and data backing the given
    /// [`ArenaStaticBox`].
    ///
    /// This returns an [`ArenaBox`] tied to the lifetime of the `&mut Option<Arena>`
    /// given, and places the arena in that space.
    pub fn unwrap(this: Self, arena: &mut Option<Arena>) -> ArenaBox<'_, T> {
        let ArenaStaticBox { data, arena: inner_arena } = this;
        arena.replace(inner_arena);
        data
    }

    /// Takes ownership of the arena and data backing the given
    /// [`ArenaStaticBox`] as raw pointers.
    ///
    /// Note that while this is safe, care must be taken to ensure that
    /// the raw pointer to the data is not accessed after the arena pointer has
    /// been released.
    pub fn into_raw(this: Self) -> (NonNull<fdf_arena_t>, NonNull<T>) {
        let res = (this.arena.0, this.data.0);
        // make sure that drop handlers aren't called for the arena
        // or box
        core::mem::forget(this);
        res
    }
}

impl<T: 'static> IntoIterator for ArenaStaticBox<[T]> {
    type IntoIter = IntoIter<T, Arena>;
    type Item = T;

    fn into_iter(self) -> Self::IntoIter {
        let len = self.len();
        let ptr = self.data.0.cast();
        // SAFETY: we will never dereference `end`
        let end = unsafe { ptr.add(len) };
        IntoIter { ptr, end, _arena: self.arena }
    }
}

impl<T: ?Sized> Deref for ArenaStaticBox<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        ArenaBox::deref(&self.data)
    }
}

impl<T: ?Sized> DerefMut for ArenaStaticBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        ArenaBox::deref_mut(&mut self.data)
    }
}

/// An equivalent to [`ArenaBox`] that holds onto a reference to the
/// arena to allow it to have static lifetime, and implements [`Clone`]
/// allowing it to be shared. Since it's shared, you can't get a mutable
/// reference to it back without using [`Self::try_unwrap`] to get the
/// inner [`ArenaStaticBox`].
#[derive(Clone, Debug)]
pub struct ArenaRc<T: ?Sized>(Arc<ArenaStaticBox<T>>);

/// A weak reference to an [`ArenaRc`].
#[derive(Clone, Debug)]
pub struct ArenaWeak<T: ?Sized>(Weak<ArenaStaticBox<T>>);

impl<T: ?Sized> ArenaRc<T> {
    /// Transforms the given [`ArenaBox`] into an [`ArenaRc`] with an owned
    /// reference to the given [`Arena`], allowing for it to be used in `'static`
    /// contexts.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the given [`ArenaBox`] is owned by this
    /// arena, or it may result in use-after-free.
    pub unsafe fn new_unchecked(arena: Arena, data: ArenaBox<'_, T>) -> ArenaRc<T> {
        // SAFETY: The `ArenaBox` will not outlive the `Arena` as it is owned
        // by the current struct and can't be moved out.
        let data = unsafe { ArenaBox::erase_lifetime(data) };
        Self(Arc::new(ArenaStaticBox { arena, data }))
    }

    /// Downgrades the given [`ArenaRc`] into an [`ArenaWeak`].
    pub fn downgrade(this: &Self) -> ArenaWeak<T> {
        ArenaWeak(Arc::downgrade(&this.0))
    }

    /// Attempts to take ownership over the arena and data backing the given
    /// [`ArenaRc`] if there is only one strong reference held to it.
    ///
    /// If there is only one strong reference, this returns an [`ArenaBox`]
    /// tied to the lifetime of the `&mut Option<Arena>` given, and places the
    /// arena in that space.
    pub fn try_unwrap(this: Self) -> Result<ArenaStaticBox<T>, Self> {
        Arc::try_unwrap(this.0).map_err(|storage| Self(storage))
    }
}

impl<T: ?Sized> From<ArenaStaticBox<T>> for ArenaRc<T> {
    fn from(value: ArenaStaticBox<T>) -> Self {
        Self(Arc::new(value))
    }
}

impl<T: ?Sized> Deref for ArenaRc<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        ArenaBox::deref(&self.0.data)
    }
}

impl<T: ?Sized> ArenaWeak<T> {
    /// Converts this [`ArenaWeak`] into a strong reference [`ArenaRc`] if there are still any
    /// outstanding strong references to it.
    pub fn upgrade(&self) -> Option<ArenaRc<T>> {
        self.0.upgrade().map(ArenaRc)
    }
}

/// Helper for allocating storage on the global heap appropriate for storing a
/// copy of `val`. This returns a pointer of a different type than the type
/// being referenced so that it can be used to allocate storage for unsized
/// slices of `T`. `ActualType` should either be the same as `T` or an array of
/// `T`.
///
/// This also correctly handles a zero sized type by returning
/// [`NonZero::dangling`].
///
/// # Safety
///
/// In addition to all the safety requirements of [`std::alloc::alloc`], the
/// caller must ensure that `T` is the type of elements of `ActualType`.
unsafe fn global_alloc<T>(layout: Layout) -> NonNull<T> {
    if layout.size() == 0 {
        NonNull::dangling()
    } else {
        let ptr = unsafe { std::alloc::alloc(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        unsafe { NonNull::new_unchecked(ptr as *mut T) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::test_utils::*;

    #[test]
    fn arena_allocations() {
        let arena = Arena::new();
        let _val = arena.insert(());
        let val = arena.insert(1);
        assert_eq!(*val, 1);
        let val = arena.insert(2);
        assert_eq!(*val, 2);
        let val = arena.insert_boxed_slice(Box::new([1, 2, 3, 4]));
        assert_eq!(&*val, &[1, 2, 3, 4]);
        let val: ArenaBox<'_, [()]> = arena.insert_boxed_slice(Box::new([]));
        assert_eq!(&*val, &[]);
        let val = arena.insert_slice(&[5, 6, 7, 8]);
        assert_eq!(&*val, &[5, 6, 7, 8]);
        let val: ArenaBox<'_, [()]> = arena.insert_slice(&[]);
        assert_eq!(&*val, &[]);
        let val: ArenaBox<'_, [u8]> = arena.insert_default_slice(10);
        assert_eq!(&*val, &[0; 10]);
    }

    #[test]
    #[allow(clippy::unit_cmp)]
    fn arena_take() {
        let arena = Arena::new();
        let val = arena.insert(());
        assert_eq!(ArenaBox::take(val), ());
        let val = arena.insert(1);
        assert_eq!(ArenaBox::take(val), 1);
    }

    #[test]
    #[allow(clippy::unit_cmp)]
    fn arena_take_boxed() {
        let arena = Arena::new();
        let val = arena.insert(());
        assert_eq!(*ArenaBox::take_boxed(val), ());
        let val = arena.insert(1);
        assert_eq!(*ArenaBox::take_boxed(val), 1);
    }

    #[test]
    fn arena_take_boxed_slice() {
        let arena = Arena::new();
        let val: ArenaBox<'_, [()]> = arena.insert_slice(&[]);
        assert_eq!(&*ArenaBox::take_boxed_slice(val), &[]);
        let val = arena.insert_slice(&[1, 2, 3, 4]);
        assert_eq!(&*ArenaBox::take_boxed_slice(val), &[1, 2, 3, 4]);
    }

    #[test]
    fn arena_drop() {
        let (tx, rx) = mpsc::channel();
        let arena = Arena::new();
        let val = arena.insert(DropSender::new(1, tx.clone()));
        drop(val);
        assert_eq!(rx.try_recv().unwrap(), 1);

        let val = arena.insert_boxed_slice(Box::new([DropSender::new(2, tx.clone())]));
        drop(val);
        assert_eq!(rx.try_recv().unwrap(), 2);

        let val = arena.insert_slice(&[DropSender::new(3, tx.clone())]);
        drop(val);
        assert_eq!(rx.try_recv().unwrap(), 3);

        rx.try_recv().expect_err("no more drops");
    }

    #[test]
    fn arena_take_drop() {
        let (tx, rx) = mpsc::channel();
        let arena = Arena::new();

        let val = arena.insert(DropSender::new(1, tx.clone()));
        let inner = ArenaBox::take(val);
        rx.try_recv().expect_err("shouldn't have dropped when taken");
        drop(inner);
        assert_eq!(rx.try_recv().unwrap(), 1);

        let val = arena.insert_slice(&[DropSender::new(2, tx.clone())]);
        let inner = ArenaBox::take_boxed_slice(val);
        rx.try_recv().expect_err("shouldn't have dropped when taken");
        drop(inner);
        assert_eq!(rx.try_recv().unwrap(), 2);

        rx.try_recv().expect_err("no more drops");
    }

    #[test]
    fn arena_contains() {
        let arena1 = Arena::new();
        let arena2 = Arena::new();

        let val1 = arena1.insert(1);
        let val2 = arena2.insert(2);

        assert!(arena1.contains(&val1));
        assert!(arena2.contains(&val2));
        assert!(!arena1.contains(&val2));
        assert!(!arena2.contains(&val1));
    }

    #[test]
    fn arena_assume() {
        let arena = Arena::new();

        let val = arena.insert(1);
        let val_leaked = unsafe { ArenaBox::into_ptr(val) };
        let val = unsafe { arena.assume(val_leaked) };

        assert!(arena.contains(&val));
    }

    #[test]
    #[should_panic]
    fn arena_bad_assume() {
        let arena = Arena::new();

        unsafe { arena.assume(NonNull::<()>::dangling()) };
    }

    #[test]
    #[should_panic]
    fn bad_static_box_ownership() {
        let arena1 = Arena::new();
        let arena2 = Arena::new();

        let val = arena1.insert(1);
        arena2.make_static(val);
    }

    #[test]
    #[should_panic]
    fn bad_rc_ownership() {
        let arena1 = Arena::new();
        let arena2 = Arena::new();

        let val = arena1.insert(1);
        arena2.make_rc(val);
    }

    #[test]
    fn box_lifecycle() {
        let arena = Arena::new();

        // create the initial value and modify it
        let mut val = arena.insert(1);
        *val = 2;
        assert_eq!(*val, 2);

        // make it a static box and modify it
        let mut val = arena.make_static(val);
        *val = 3;
        assert_eq!(*val, 3);

        // make it into a refcounted shared pointer and check the value is still the
        // same
        let val = ArenaRc::from(val);
        assert_eq!(*val, 3);

        // clone the refcount and verify that we can't unwrap it back to a static box.
        let val_copied = val.clone();
        assert_eq!(*val_copied, 3);
        let val = ArenaRc::try_unwrap(val).expect_err("Double strong count should fail to unwrap");
        assert_eq!(*val, 3);
        drop(val_copied);

        // now that the cloned rc is gone, unwrap it back to a static box and modify it
        let mut val =
            ArenaRc::try_unwrap(val).expect("strong count should be one so this should unwrap now");
        *val = 4;
        assert_eq!(*val, 4);

        // bring it back to a normal arena box and modify it
        let mut shared_arena = None;
        let mut val = ArenaStaticBox::unwrap(val, &mut shared_arena);
        *val = 5;
        assert_eq!(*val, 5);

        // make it back into an rc but directly rather than from a static box
        let val = arena.make_rc(val);
        assert_eq!(*val, 5);
    }

    #[test]
    fn static_raw_roundtrip() {
        let arena = Arena::new();
        let val = arena.make_static(arena.insert(1));

        // turn it into raw pointers and modify it
        let (arena_ptr, mut data_ptr) = ArenaStaticBox::into_raw(val);
        *unsafe { data_ptr.as_mut() } = 2;
        assert_eq!(*unsafe { data_ptr.as_ref() }, 2);

        // reconstitute it back to an `ArenaBox` and then transform it
        let arena = unsafe { Arena::from_raw(arena_ptr) };
        let val = unsafe { arena.assume(data_ptr) };

        assert_eq!(*val, 2);
    }

    #[test]
    fn arena_into_and_from_iter() {
        let arena = Arena::new();

        // empty slice to vec
        let val: ArenaBox<'_, [()]> = arena.insert_slice(&[]);
        let vec_val = Vec::from_iter(val);
        assert!(vec_val.is_empty());

        // filled slice to vec
        let val = arena.insert_slice(&[1, 2, 3, 4]);
        let vec_val = Vec::from_iter(val);
        assert_eq!(&[1, 2, 3, 4], &*vec_val);

        // filled static slice to vec
        let val = arena.make_static(arena.insert_slice(&[1, 2, 3, 4]));
        let vec_val = Vec::from_iter(val);
        assert_eq!(&[1, 2, 3, 4], &*vec_val);

        // empty vec to arena box
        let val: Vec<()> = vec![];
        let arena_val = arena.insert_from_iter(val.clone());
        assert_eq!(val, &*arena_val);

        // filled vec to arena box
        let val = vec![1, 2, 3, 4];
        let arena_val = arena.insert_from_iter(val);
        assert_eq!(&[1, 2, 3, 4], &*arena_val);
    }

    #[test]
    fn arena_try_from_iter() {
        let arena = Arena::new();

        let val: Vec<Result<_, ()>> = vec![Ok(1), Ok(2), Ok(3), Ok(4)];
        let arena_val = arena.try_insert_from_iter(val).unwrap();
        assert_eq!(&[1, 2, 3, 4], &*arena_val);

        let (tx, rx) = mpsc::channel();
        let val = vec![Ok(DropSender::new(0, tx.clone())), Err(-1), Ok(DropSender::new(1, tx))];
        let Err(-1) = arena.try_insert_from_iter(val) else {
            panic!("early exit from try_insert_from_iter")
        };
        let Ok(0) = rx.try_recv() else {
            panic!("expected drop of leading ok value to have happened")
        };
        let Ok(1) = rx.try_recv() else {
            panic!("expected drop of trailing ok value to have happened")
        };
    }
}
