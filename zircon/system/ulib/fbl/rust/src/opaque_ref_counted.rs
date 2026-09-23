// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::recyclable::Recyclable;
use crate::ref_counted::{HasRefCount, RefCounted};
use core::marker::{PhantomData, PhantomPinned};
use core::ops::Deref;
use core::ptr::NonNull;
use zr::Opaque;

/// A wrapper for C++ objects that are known to use `fbl::RefCounted`
/// and have their reference count at offset 0.
#[repr(transparent)]
pub struct OpaqueRefCounted<T>(Opaque<T>);

impl<T> OpaqueRefCounted<T> {
    /// Returns a raw pointer to the opaque data.
    pub fn get(&self) -> *mut T {
        self.0.get()
    }
}

impl<T> HasRefCount for OpaqueRefCounted<T> {
    fn ref_count(&self) -> &RefCounted {
        // SAFETY: OpaqueRefCounted guarantees that the ref count is at offset 0.
        unsafe { &*(self.get() as *const RefCounted) }
    }
}

/// A zero-sized facade type for opaque ref-counted C++ objects that derive from a base class `B`.
///
/// This is used as a field in Rust facade structs that represent C++ objects of unknown size.
/// It keeps the facade struct `Sized` (size 0) so it can be used in FFI (thin pointers) and with
/// generic containers like `RefPtr`, while providing `Send`, `Sync`, and `PhantomPinned`.
#[repr(C)]
#[derive(Default)]
pub struct OpaqueRefCountedFacade<B = RefCounted> {
    _marker: PhantomData<(PhantomPinned, fn() -> B)>,
    _facade: zr::OpaqueFacade,
}

unsafe impl<B> Send for OpaqueRefCountedFacade<B> {}
unsafe impl<B> Sync for OpaqueRefCountedFacade<B> {}

impl<B: HasRefCount> HasRefCount for OpaqueRefCountedFacade<B> {
    fn ref_count(&self) -> &RefCounted {
        // SAFETY: OpaqueRefCountedFacade<B> is at offset 0 of the facade struct.
        unsafe {
            let b_ptr = self as *const Self as *const B;
            (*b_ptr).ref_count()
        }
    }
}

unsafe impl<B: Recyclable> Recyclable for OpaqueRefCountedFacade<B> {
    unsafe fn recycle(ptr: NonNull<Self>) {
        unsafe {
            B::recycle(ptr.cast::<B>());
        }
    }
}

/// Trait for facade types that wrap an `OpaqueRefCountedFacade<B>` and derefer to `B`.
///
/// Implementing this trait automatically provides `HasRefCount` and `Recyclable` for `Self`.
///
/// # Safety
///
/// `Self` must be a facade struct for a C++ object that inherits from `TargetBase` and derefers to
/// `TargetBase`.
pub unsafe trait IsOpaqueRefCounted: Deref + Sized {
    type TargetBase: HasRefCount + Recyclable;
}

impl<T: IsOpaqueRefCounted> HasRefCount for T {
    fn ref_count(&self) -> &RefCounted {
        let base_ptr = self.deref() as *const T::Target as *const T::TargetBase;
        // SAFETY: T is a facade struct for a C++ object that inherits from T::TargetBase.
        unsafe { (*base_ptr).ref_count() }
    }
}

unsafe impl<T: IsOpaqueRefCounted> Recyclable for T {
    unsafe fn recycle(ptr: NonNull<Self>) {
        unsafe {
            let base_ptr = ptr.cast::<T::TargetBase>();
            <T::TargetBase as Recyclable>::recycle(base_ptr);
        }
    }
}

/// Declares a zero-sized facade struct for an opaque refcounted C++ object and implements
/// `HasRefCount` and `Recyclable` for it.
///
/// Supported forms:
/// 1. Where `fbl::RefCounted` is at offset 0 of the C++ object:
/// ```rust
/// fbl::impl_opaque_ref_counted_facade!(
///     /// Facade type representing the C++ `iommu::Iommu` object.
///     pub struct Iommu,
///     cpp_iommu_recycle,
/// );
/// ```
/// 2. Where `fbl::RefCounted` is at a non-zero offset or requires a C++ helper function:
/// ```rust
/// fbl::impl_opaque_ref_counted_facade!(
///     /// Facade type representing the C++ `VmObject` object.
///     pub struct VmObject,
///     cpp_vm_object_free,
///     cpp_vm_object_get_ref_counted,
/// );
/// ```
#[macro_export]
macro_rules! impl_opaque_ref_counted_facade {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident,
        $recycle_fn:path $(,)?
    ) => {
        $(#[$meta])*
        #[repr(C)]
        $vis struct $name {
            _facade: $crate::OpaqueRefCountedFacade,
        }

        impl $crate::HasRefCount for $name {
            fn ref_count(&self) -> &$crate::RefCounted {
                // SAFETY: `$name` represents a C++ `fbl::RefCounted` object whose ref count is at
                // offset 0.
                unsafe { &*(self as *const Self as *const $crate::RefCounted) }
            }
        }

        // SAFETY: `$name` represents a C++ `fbl::RefCounted` object.
        unsafe impl $crate::Recyclable for $name {
            unsafe fn recycle(ptr: core::ptr::NonNull<Self>) {
                // SAFETY: `ptr` was constructed from `RefPtr::into_raw` on a valid `$name` facade.
                unsafe {
                    $recycle_fn(ptr.as_ptr() as *mut Self);
                }
            }
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident,
        $recycle_fn:path,
        $get_ref_counted_fn:path $(,)?
    ) => {
        $(#[$meta])*
        #[repr(C)]
        $vis struct $name {
            _facade: $crate::OpaqueRefCountedFacade,
        }

        impl $crate::HasRefCount for $name {
            fn ref_count(&self) -> &$crate::RefCounted {
                // SAFETY: `$get_ref_counted_fn` returns a valid pointer to the C++
                // `fbl::RefCounted` subobject of `$name`.
                unsafe {
                    &*($get_ref_counted_fn(self as *const Self as *mut Self)
                        as *const $crate::RefCounted)
                }
            }
        }

        // SAFETY: `$name` represents a C++ `fbl::RefCounted` object.
        unsafe impl $crate::Recyclable for $name {
            unsafe fn recycle(ptr: core::ptr::NonNull<Self>) {
                // SAFETY: `ptr` was constructed from `RefPtr::into_raw` on a valid `$name` facade.
                unsafe {
                    $recycle_fn(ptr.as_ptr() as *mut Self);
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recyclable::Recyclable;
    use crate::ref_ptr::RefPtr;
    use core::ffi::c_void;
    use core::ptr::NonNull;

    unsafe extern "C" {
        fn create_cpp_ref_counted_object(destroyed: *mut bool) -> *mut c_void;
        fn destroy_cpp_ref_counted_object(ptr: *mut c_void);
    }

    pub struct TestCppRefCountedObject;

    unsafe impl Recyclable for OpaqueRefCounted<TestCppRefCountedObject> {
        unsafe fn recycle(ptr: NonNull<Self>) {
            unsafe {
                destroy_cpp_ref_counted_object(ptr.as_ptr() as *mut c_void);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_cross_lang_ref_ptr() {
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed = AtomicBool::new(false);
        unsafe {
            let raw_ptr = create_cpp_ref_counted_object(destroyed.as_ptr());
            assert!(!destroyed.load(Ordering::Relaxed));

            {
                let ref_ptr =
                    RefPtr::from_raw(raw_ptr as *mut OpaqueRefCounted<TestCppRefCountedObject>);
                assert!(!destroyed.load(Ordering::Relaxed));

                let ref_ptr_clone = ref_ptr.clone();
                assert!(!destroyed.load(Ordering::Relaxed));

                // Drop clone
                drop(ref_ptr_clone);
                assert!(!destroyed.load(Ordering::Relaxed));
            } // Drop ref_ptr -> count becomes 0 -> calls recycle -> calls C++ release!

            assert!(destroyed.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn test_opaque_ref_counted_allocate_fails() {
        let val = OpaqueRefCounted(Opaque::uninit());
        let res = OpaqueRefCounted::<TestCppRefCountedObject>::allocate(val);
        assert!(res.is_err());
    }

    pub struct TestCppFacadeBase;
    unsafe impl Recyclable for TestCppFacadeBase {
        unsafe fn recycle(ptr: NonNull<Self>) {
            unsafe {
                destroy_cpp_ref_counted_object(ptr.as_ptr() as *mut c_void);
            }
        }
    }
    impl HasRefCount for TestCppFacadeBase {
        fn ref_count(&self) -> &RefCounted {
            unsafe { &*(self as *const Self as *const RefCounted) }
        }
    }

    #[repr(C)]
    pub struct TestSubtypeFacade {
        _facade: OpaqueRefCountedFacade<TestCppFacadeBase>,
    }
    impl Deref for TestSubtypeFacade {
        type Target = TestCppFacadeBase;
        fn deref(&self) -> &Self::Target {
            unsafe { &*(self as *const Self as *const TestCppFacadeBase) }
        }
    }
    unsafe impl IsOpaqueRefCounted for TestSubtypeFacade {
        type TargetBase = TestCppFacadeBase;
    }

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_facade_ref_ptr() {
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed = AtomicBool::new(false);
        unsafe {
            let raw_ptr = create_cpp_ref_counted_object(destroyed.as_ptr());
            assert!(!destroyed.load(Ordering::Relaxed));

            {
                let ref_ptr = RefPtr::from_raw(raw_ptr as *mut TestSubtypeFacade);
                assert!(!destroyed.load(Ordering::Relaxed));

                let ref_ptr_clone = ref_ptr.clone();
                assert!(!destroyed.load(Ordering::Relaxed));

                drop(ref_ptr_clone);
                assert!(!destroyed.load(Ordering::Relaxed));
            }

            assert!(destroyed.load(Ordering::Relaxed));
        }
    }

    unsafe extern "C" fn test_release_fn(ptr: *mut TestMacroFacade) {
        unsafe {
            destroy_cpp_ref_counted_object(ptr as *mut c_void);
        }
    }

    impl_opaque_ref_counted_facade!(
        pub struct TestMacroFacade,
        test_release_fn,
    );

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_macro_facade_ref_ptr() {
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed = AtomicBool::new(false);
        unsafe {
            let raw_ptr = create_cpp_ref_counted_object(destroyed.as_ptr());
            assert!(!destroyed.load(Ordering::Relaxed));

            {
                let ref_ptr = RefPtr::from_raw(raw_ptr as *mut TestMacroFacade);
                assert!(!destroyed.load(Ordering::Relaxed));

                let ref_ptr_clone = ref_ptr.clone();
                assert!(!destroyed.load(Ordering::Relaxed));

                drop(ref_ptr_clone);
                assert!(!destroyed.load(Ordering::Relaxed));
            }

            assert!(destroyed.load(Ordering::Relaxed));
        }
    }

    unsafe extern "C" fn test_get_ref_counted_fn(
        ptr: *mut TestMacroFacadeWithGetter,
    ) -> *mut RefCounted {
        ptr as *mut RefCounted
    }

    unsafe extern "C" fn test_release_fn_with_getter(ptr: *mut TestMacroFacadeWithGetter) {
        unsafe {
            destroy_cpp_ref_counted_object(ptr as *mut c_void);
        }
    }

    impl_opaque_ref_counted_facade!(
        pub struct TestMacroFacadeWithGetter,
        test_release_fn_with_getter,
        test_get_ref_counted_fn,
    );

    #[test]
    #[cfg_attr(miri, ignore = "miri does not support calling foreign functions")]
    fn test_macro_facade_with_getter_ref_ptr() {
        use core::sync::atomic::{AtomicBool, Ordering};

        let destroyed = AtomicBool::new(false);
        unsafe {
            let raw_ptr = create_cpp_ref_counted_object(destroyed.as_ptr());
            assert!(!destroyed.load(Ordering::Relaxed));

            {
                let ref_ptr = RefPtr::from_raw(raw_ptr as *mut TestMacroFacadeWithGetter);
                assert!(!destroyed.load(Ordering::Relaxed));

                let ref_ptr_clone = ref_ptr.clone();
                assert!(!destroyed.load(Ordering::Relaxed));

                drop(ref_ptr_clone);
                assert!(!destroyed.load(Ordering::Relaxed));
            }

            assert!(destroyed.load(Ordering::Relaxed));
        }
    }
}
