// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::recyclable::Recyclable;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use kalloc::AllocError;

/// `UniquePtr<T>` holds a unique pointer to an object of type T and deletes the
/// object when the `UniquePtr` goes out of scope.
///
/// T must implement the `Recyclable` trait to define how the object is destroyed.
/// This allows `UniquePtr` to manage both Rust-allocated objects and C++-allocated
/// objects.
#[repr(C)]
pub struct UniquePtr<T>
where
    T: Recyclable,
{
    ptr: NonNull<T>,
}

impl<T: Recyclable> UniquePtr<T> {
    /// Constructs a `UniquePtr` from a raw pointer.
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid and the sole owning reference to the object pointed to by `ptr`.
    /// - `ptr` must have been allocated in such a way that calling `T::recycle(ptr)` is a
    ///   correct way to deallocate the pointer.
    pub unsafe fn from_raw(ptr: *mut T) -> Self {
        // SAFETY: The caller must ensure that ptr is valid.
        unsafe { UniquePtr { ptr: NonNull::new_unchecked(ptr) } }
    }

    /// Helper function that allocates a new instance of `T` using `T::allocate` and
    /// returns a `UniquePtr` wrapping it.
    ///
    /// For Rust-allocated objects where `T` derives `Recyclable`, this will use
    /// `Box::try_new`.
    pub fn try_new(value: T) -> Result<UniquePtr<T>, AllocError> {
        let ptr = T::allocate(value)?;
        Ok(UniquePtr { ptr })
    }

    /// Returns the raw pointer to the object.
    pub fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr()
    }

    /// Returns a mutable raw pointer to the object.
    pub fn as_mut_ptr(this: &mut Self) -> *mut T {
        this.ptr.as_ptr()
    }

    /// Consume the `UniquePtr` and return the raw pointer without destroying the object.
    ///
    /// The caller is responsible for managing the object's lifetime after this call.
    pub fn into_raw(this: Self) -> *mut T {
        let ptr = this.ptr.as_ptr();
        core::mem::forget(this);
        ptr
    }
}

impl<T: Recyclable> Deref for UniquePtr<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: Recyclable> DerefMut for UniquePtr<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: Recyclable> Drop for UniquePtr<T> {
    fn drop(&mut self) {
        unsafe {
            T::recycle(self.ptr);
        }
    }
}

unsafe impl<T: Recyclable + Send> Send for UniquePtr<T> {}
unsafe impl<T: Recyclable + Sync> Sync for UniquePtr<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicBool, Ordering};
    use kalloc::Box;
    use zr::Opaque;
    extern crate alloc;
    use alloc::sync::Arc;

    #[derive(fbl::Recyclable)]
    pub struct TestRustObject {
        destroyed: Arc<AtomicBool>,
    }

    impl Drop for TestRustObject {
        fn drop(&mut self) {
            self.destroyed.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_unique_ptr_drops() {
        let destroyed = Arc::new(AtomicBool::new(false));
        {
            let obj = TestRustObject { destroyed: destroyed.clone() };
            let _unique_ptr = UniquePtr::try_new(obj).unwrap();
            assert!(!destroyed.load(Ordering::Relaxed));
        } // unique_ptr drops here

        assert!(destroyed.load(Ordering::Relaxed));
    }

    #[test]
    fn test_unique_ptr_into_raw() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let raw_ptr;
        {
            let obj = TestRustObject { destroyed: destroyed.clone() };
            let unique_ptr = UniquePtr::try_new(obj).unwrap();
            assert!(!destroyed.load(Ordering::Relaxed));
            raw_ptr = UniquePtr::into_raw(unique_ptr);
        } // unique_ptr drops here but into_raw called, so no destruction

        assert!(!destroyed.load(Ordering::Relaxed));

        // Clean up manually
        unsafe {
            drop(Box::from_raw(raw_ptr));
        }
        assert!(destroyed.load(Ordering::Relaxed));
    }

    unsafe extern "C" {
        fn create_cpp_object(destroyed: *mut bool) -> *mut c_void;
        fn destroy_cpp_object(ptr: *mut c_void);
    }

    pub struct TestCppObject;

    unsafe impl Recyclable for Opaque<TestCppObject> {
        unsafe fn recycle(ptr: NonNull<Self>) {
            unsafe {
                destroy_cpp_object(ptr.as_ptr() as *mut c_void);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_unique_ptr_cpp_drops() {
        let destroyed = AtomicBool::new(false);
        unsafe {
            let raw_ptr = create_cpp_object(destroyed.as_ptr() as *mut bool);
            assert!(!destroyed.load(Ordering::Relaxed));
            {
                let _unique_ptr = UniquePtr::from_raw(raw_ptr as *mut Opaque<TestCppObject>);
                assert!(!destroyed.load(Ordering::Relaxed));
            } // unique_ptr drops here

            assert!(destroyed.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn test_unique_ptr_as_mut_ptr() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let mut unique_ptr =
            UniquePtr::try_new(TestRustObject { destroyed: destroyed.clone() }).unwrap();
        let mut_ptr = UniquePtr::as_mut_ptr(&mut unique_ptr);
        assert!(!mut_ptr.is_null());
    }

    #[test]
    fn test_unique_ptr_deref_mut() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let mut unique_ptr =
            UniquePtr::try_new(TestRustObject { destroyed: destroyed.clone() }).unwrap();
        let _deref_mut: &mut TestRustObject = &mut unique_ptr;
    }

    #[test]
    fn test_unique_ptr_cpp_alloc_fail() {
        let dummy_val = Opaque::uninit();
        let res = Opaque::<TestCppObject>::allocate(dummy_val);
        assert!(res.is_err());
    }
}
