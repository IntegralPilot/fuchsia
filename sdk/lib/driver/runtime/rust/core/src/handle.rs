// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for driver runtime handles and collections of mixed driver and zircon
//! handles.

use fdf_sys::*;

use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::num::NonZero;
use core::ops::Deref;

use zx::sys::zx_handle_t;
pub use zx::{HandleRef as ZirconHandleRef, NullableHandle as ZirconHandle};

pub use fdf_sys::fdf_handle_t;

/// A handle representing some resource managed by the driver runtime.
#[repr(C)]
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct DriverHandle(NonZero<fdf_handle_t>);

impl DriverHandle {
    /// Constructs a [`DriverHandle`] for the given [`fdf_handle_t`]
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the handle given is a valid driver handle:
    ///
    /// - It has the marker bits set correctly
    /// - It is not "owned" elsewhere, or that it will not call [`Drop::drop`] on this new
    ///   object if it is.
    pub unsafe fn new_unchecked(handle: NonZero<fdf_handle_t>) -> Self {
        Self(handle)
    }

    /// Gets a [`DriverHandleRef`] of this handle
    pub fn as_handle_ref(&self) -> DriverHandleRef<'_> {
        DriverHandleRef(ManuallyDrop::new(Self(self.0)), PhantomData)
    }

    /// Gets the raw handle object
    ///
    /// # Safety
    ///
    /// The caller must be sure to not let this handle outlive the lifetime of the object it
    /// came from.
    pub unsafe fn get_raw(&self) -> NonZero<fdf_handle_t> {
        self.0
    }

    /// Turns this handle into its raw handle number, without dropping the handle.
    /// The caller is responsible for ensuring that the handle is released or reconstituted
    /// into a [`DriverHandle`].
    pub fn into_raw(self) -> NonZero<fdf_handle_t> {
        let handle = self.0;
        // prevent this from dropping and invalidating the handle
        core::mem::forget(self);
        handle
    }
}

impl Drop for DriverHandle {
    fn drop(&mut self) {
        // SAFETY: We require a nonzero handle to construct this and we should own the
        // handle, so it should be safe to close it.
        unsafe { fdf_handle_close(self.0.get()) };
    }
}

/// An unowned reference to a driver handle type
#[derive(Debug)]
pub struct DriverHandleRef<'a>(ManuallyDrop<DriverHandle>, PhantomData<&'a DriverHandle>);

impl<'a> Deref for DriverHandleRef<'a> {
    type Target = DriverHandle;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An enum of the two types of handles that can be represented in a [`MixedHandle`].
#[derive(Debug)]
pub enum MixedHandleType<Driver, Zircon> {
    /// A driver runtime managed handle
    Driver(Driver),
    /// A kernel handle
    Zircon(Zircon),
}

impl From<MixedHandle> for MixedHandleType<DriverHandle, ZirconHandle> {
    fn from(value: MixedHandle) -> Self {
        value.resolve()
    }
}

/// A handle that might be either a [`DriverHandle`] or a [`ZirconHandle`], depending on its
/// bit pattern.
#[derive(Debug)]
#[repr(C)]
pub struct MixedHandle(NonZero<zx_handle_t>);

impl MixedHandle {
    /// Makes a `MixedHandle` from an existing raw handle.
    ///
    /// # Safety
    ///
    /// The handle must be valid and unowned, as this will take ownership
    /// of the handle and drop it when this object drops.
    pub unsafe fn from_raw(handle: NonZero<fdf_handle_t>) -> Self {
        Self(handle)
    }

    /// Makes a `MixedHandle` from an existing raw handle that might be
    /// zeroed (invalid).
    ///
    /// # Safety
    ///
    /// The handle must be valid and unowned, as this will take ownership
    /// of the handle and drop it when this object drops.
    pub unsafe fn try_from_raw(handle: fdf_handle_t) -> Option<Self> {
        NonZero::new(handle).map(|handle| {
            // SAFETY: the caller promises this is valid and unowned
            unsafe { Self::from_raw(handle) }
        })
    }

    /// Makes a `MixedHandle` from an existing [`ZirconHandle`]. Returns
    /// [`None`] if the handle is invalid.
    pub fn from_zircon_handle(handle: ZirconHandle) -> Option<Self> {
        if handle.is_invalid() {
            None
        } else {
            // SAFETY: if `ZirconHandle::is_invalid` returns false, then the
            // handle is `NonZero`.
            Some(Self(unsafe { NonZero::new_unchecked(handle.into_raw()) }))
        }
    }

    /// Evaluates whether the contained handle is a driver handle or not
    pub fn is_driver(&self) -> bool {
        self.0.get() & 0b1 == 0b0
    }

    /// Resolves the handle to the appropriate real handle type
    pub fn resolve(self) -> MixedHandleType<DriverHandle, ZirconHandle> {
        let res = if self.is_driver() {
            MixedHandleType::Driver(DriverHandle(self.0))
        } else {
            // SAFETY: any non-zero handle that isn't a driver handle must be a
            // zircon handle of some sort.
            MixedHandleType::Zircon(unsafe { ZirconHandle::from_raw(self.0.get()) })
        };
        // forget self so we don't try to drop the handle we just put
        // in the enum
        core::mem::forget(self);
        res
    }

    /// Resolves the handle to an appropriate real unowned handle type
    pub fn resolve_ref(&self) -> MixedHandleType<DriverHandleRef<'_>, ZirconHandleRef<'_>> {
        if self.is_driver() {
            MixedHandleType::Driver(DriverHandleRef(
                ManuallyDrop::new(DriverHandle(self.0)),
                PhantomData,
            ))
        } else {
            // SAFETY: any non-zero handle that isn't a driver handle must
            // be a zircon handle of some sort.
            MixedHandleType::Zircon(unsafe { ZirconHandleRef::from_raw_handle(self.0.get()) })
        }
    }
}

impl From<DriverHandle> for MixedHandle {
    fn from(value: DriverHandle) -> Self {
        let handle = value.0;
        // SAFETY: the handle is valid by construction since it was taken
        // from a correctly created `DriverHandle`, and we `forget` the
        // `DriverHandle` so we can take ownership of the handle.
        unsafe {
            core::mem::forget(value);
            MixedHandle::from_raw(handle)
        }
    }
}

impl Drop for MixedHandle {
    fn drop(&mut self) {
        let handle = if self.is_driver() {
            MixedHandleType::Driver(DriverHandle(self.0))
        } else {
            // SAFETY: any non-zero handle that isn't a driver handle must
            // be a zircon handle of some sort.
            MixedHandleType::Zircon(unsafe { ZirconHandle::from_raw(self.0.get()) })
        };
        drop(handle)
    }
}

#[cfg(test)]
mod tests {
    use zx::{Port, Status};

    use super::*;

    /// Creates a valid `DriverHandle` by creating a driver channel pair and returning one of them.
    fn make_driver_handle() -> DriverHandle {
        let (mut left, mut right) = Default::default();
        Status::ok(unsafe { fdf_channel_create(0, &mut left, &mut right) }).unwrap();
        unsafe { fdf_handle_close(right) };

        DriverHandle(NonZero::new(left).unwrap())
    }

    #[test]
    fn handle_sizes() {
        assert_eq!(size_of::<fdf_handle_t>(), size_of::<Option<DriverHandle>>());
        assert_eq!(size_of::<fdf_handle_t>(), size_of::<Option<MixedHandle>>());
    }

    #[test]
    fn driver_handle_roundtrip() {
        let handle = make_driver_handle();
        let mixed_handle = unsafe { MixedHandle::from_raw(handle.into_raw()) };
        assert!(mixed_handle.is_driver());

        let MixedHandleType::Driver(_handle) = mixed_handle.resolve() else {
            panic!("driver handle did not translate back to a driver handle");
        };
    }

    #[test]
    fn zircon_handle_roundtrip() {
        let handle = Port::create();
        let mixed_handle =
            unsafe { MixedHandle::from_raw(NonZero::new(handle.into_raw()).unwrap()) };
        assert!(!mixed_handle.is_driver());

        let MixedHandleType::Zircon(_handle) = mixed_handle.resolve() else {
            panic!("zircon handle did not translate back to a zircon handle");
        };
    }
}
