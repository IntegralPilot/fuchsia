// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A helper macro to initialize a pinned object in-place using a raw FFI constructor.
///
/// This generates a `PinInit` block that passes a type-cast pointer of the allocated
/// slot to the FFI function, returning `Ok(())`.
#[macro_export]
macro_rules! pin_init_ffi {
    ($ffi_fn:expr) => {
        unsafe {
            pin_init::pin_init_from_closure(|slot| {
                $ffi_fn(slot as *mut _);
                Ok(())
            })
        }
    };
    ($ffi_fn:expr, $($args:expr),+ $(,)?) => {
        unsafe {
            pin_init::pin_init_from_closure(move |slot| {
                $ffi_fn(slot as *mut _, $($args),*);
                Ok(())
            })
        }
    };
}

/// A helper macro to implement `PinnedDrop` using a raw FFI destructor.
///
/// This generates a `PinnedDrop` implementation that calls the FFI function
/// with a type-cast pointer of the object.
///
/// # Safety
///
/// The FFI function must take a pointer to the object and correctly clean it up.
#[macro_export]
macro_rules! unsafe_pinned_drop_ffi {
    ($type:ty, $ffi_fn:expr) => {
        #[pin_init::pinned_drop]
        impl pin_init::PinnedDrop for $type {
            fn drop(self: core::pin::Pin<&mut Self>) {
                unsafe {
                    let me = self.get_unchecked_mut();
                    $ffi_fn($crate::ToMutPtr::to_mut_ptr(me) as *mut _);
                }
            }
        }
    };
}
