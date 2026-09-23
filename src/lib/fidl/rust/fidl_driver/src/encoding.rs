// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::encoding::{
    ALLOC_ABSENT_U32, ALLOC_PRESENT_U32, Context, Decode, Decoder, DefaultFuchsiaResourceDialect,
    Depth, Encode, Encoder, ResourceTypeMarker, TypeMarker,
};

use fdf::{Channel, MixedHandle, MixedHandleType};
use zx::{HandleDisposition, HandleInfo, HandleOp, NullableHandle, ObjectType, Rights, Status};

use std::marker::PhantomData;

use crate::endpoints::DriverClientEnd;

#[derive(Debug, Clone)]
pub struct DriverEndpoint<T>(PhantomData<DriverClientEnd<T>>);

unsafe impl<T: 'static> TypeMarker for DriverEndpoint<T> {
    type Owned = DriverClientEnd<T>;

    fn inline_align(_context: Context) -> usize {
        4
    }

    fn inline_size(_context: Context) -> usize {
        4
    }
}

impl<T: 'static> ResourceTypeMarker for DriverEndpoint<T> {
    type Borrowed<'a> = DriverClientEnd<T>;

    fn take_or_borrow(value: &mut Self::Owned) -> Self::Borrowed<'_> {
        DriverClientEnd(value.0.take(), PhantomData)
    }
}

impl<T: 'static> Decode<DriverEndpoint<T>, DefaultFuchsiaResourceDialect> for DriverClientEnd<T> {
    fn new_empty() -> Self {
        Self(None, PhantomData)
    }

    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, DefaultFuchsiaResourceDialect>,
        offset: usize,
        _depth: Depth,
    ) -> fidl::Result<()> {
        match decoder.read_num::<u32>(offset) {
            ALLOC_PRESENT_U32 => {}
            ALLOC_ABSENT_U32 => return Err(fidl::Error::NotNullable),
            _ => return Err(fidl::Error::InvalidPresenceIndicator),
        }
        // Take what the decoder thinks is a NONE-type zircon handle and validate that it's
        // a driver handle and use that.
        let handle = decoder.take_next_handle(ObjectType::NONE, Rights::empty())?.into_raw();
        let mixed_handle = unsafe { MixedHandle::try_from_raw(handle) };
        match mixed_handle.map(MixedHandle::resolve) {
            None => (),
            Some(MixedHandleType::Driver(driver_handle)) => {
                // SAFETY: we own this handle as the receiver of the fidl message that contained it.
                self.0 = unsafe { Some(Channel::from_driver_handle(driver_handle)) };
            }
            _ => {
                // The handle was present but not a driver handle, so is not valid.
                return Err(fidl::Error::Invalid);
            }
        }
        Ok(())
    }
}

unsafe impl<T: 'static> Encode<DriverEndpoint<T>, DefaultFuchsiaResourceDialect>
    for DriverClientEnd<T>
{
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, DefaultFuchsiaResourceDialect>,
        offset: usize,
        _depth: Depth,
    ) -> fidl::Result<()> {
        let Some(channel) = self.0 else {
            return Err(fidl::Error::NotNullable);
        };
        // SAFETY: we are taking the handle and converting it into what looks like a zircon
        // handle for the benefit of the fidl encoding library. If something tries to close it,
        // it will cause an error and leak the driver handle but it will not cause any other
        // unsoundness.
        let handle = unsafe {
            fidl::NullableHandle::from_raw(channel.into_driver_handle().into_raw().get())
        };

        // SAFETY: the caller is responsible for ensuring that there's enough space for this
        // value.
        unsafe { encoder.write_num(ALLOC_PRESENT_U32, offset) };
        encoder.push_next_handle(HandleDisposition::new(
            HandleOp::Move(handle),
            ObjectType::NONE,
            Rights::empty(),
            Ok(()),
        ));
        Ok(())
    }
}

/// Converts the handle to a [`HandleInfo`], with [`DriverHandle`]s being represented as
/// a valid driver handle but [`ObjectType::NONE`].
///
/// This uses [`Handle::basic_info`] to get the object type and rights for a zircon
/// handle, so may return an error if that fails.
///
/// # Safety
///
/// The caller is responsible for making sure that the handle is either converted
/// back to a [`MixedHandle`] or the correct cleanup happens, or an error or leak
/// may happen when the [`HandleInfo`] is dropped.
pub unsafe fn mixed_into_handle_info(this: Option<MixedHandle>) -> Result<HandleInfo, Status> {
    use MixedHandleType::*;
    let Some(this) = this else {
        return Ok(HandleInfo::new(NullableHandle::invalid(), ObjectType::NONE, Rights::empty()));
    };
    match this.resolve() {
        Zircon(handle) => {
            let basic_info = handle.basic_info()?;
            Ok(HandleInfo::new(handle, basic_info.object_type, basic_info.rights))
        }
        Driver(handle) => {
            // SAFETY: we are wrapping the technically invalid handle in a `ManuallyDrop`
            // to prevent it from being dropped.
            Ok(HandleInfo::new(
                unsafe { NullableHandle::from_raw(handle.into_raw().get()) },
                ObjectType::NONE,
                Rights::empty(),
            ))
        }
    }
}

/// Converts a [`HandleDisposition`] to a [`MixedHandle`], relying on the bit pattern of
/// the handle to correctly identify [`Handle`] vs. [`fdf::DriverHandle`].
///
/// # Panics
///
/// This panics if the handle's op is not [`zx::HandleOp::Move`]
pub fn mixed_from_handle_disposition(
    mut handle: HandleDisposition<'static>,
) -> Option<MixedHandle> {
    use zx::HandleOp::*;
    match handle.take_op() {
        Move(handle) => MixedHandle::from_zircon_handle(handle),
        Duplicate(_) => {
            panic!("tried to convert a duplicate HandleDisposition into a driver MixedHandle")
        }
    }
}

#[cfg(test)]
mod tests {
    use fdf::DriverHandle;
    use zx::Port;

    use super::*;

    /// Creates a valid `DriverHandle` by creating a driver channel pair and returning one of them.
    fn make_driver_handle() -> DriverHandle {
        let (left, right) = fdf::Channel::<()>::create();
        drop(right);
        left.into_driver_handle()
    }

    #[test]
    fn driver_handle_info() {
        let handle = MixedHandle::from(make_driver_handle());
        let handle_info = unsafe { mixed_into_handle_info(Some(handle)).unwrap() };
        assert_eq!(handle_info.object_type, ObjectType::NONE);
        assert_eq!(handle_info.rights, Rights::empty());
        // take the handle back to a mixed handle so it gets dropped properly
        MixedHandle::from_zircon_handle(handle_info.handle).unwrap();
    }

    #[test]
    fn driver_handle_disposition() {
        let handle_disposition = HandleDisposition::new(
            HandleOp::Move(unsafe {
                NullableHandle::from_raw(make_driver_handle().into_raw().get())
            }),
            ObjectType::NONE,
            Rights::empty(),
            Ok(()),
        );
        mixed_from_handle_disposition(handle_disposition).unwrap();
    }

    #[test]
    fn zircon_handle_info() {
        let handle = MixedHandle::from_zircon_handle(Port::create().into()).unwrap();
        let handle_info = unsafe { mixed_into_handle_info(Some(handle)).unwrap() };
        assert_eq!(handle_info.object_type, ObjectType::PORT);
        assert_eq!(
            handle_info.rights,
            Rights::DUPLICATE | Rights::TRANSFER | Rights::READ | Rights::WRITE | Rights::INSPECT
        );
        // take the handle back to a mixed handle so it gets dropped properly
        MixedHandle::from_zircon_handle(handle_info.handle).unwrap();
    }

    #[test]
    fn zircon_handle_disposition() {
        let handle_disposition = HandleDisposition::new(
            HandleOp::Move(Port::create().into()),
            ObjectType::PORT,
            Rights::DUPLICATE | Rights::TRANSFER | Rights::READ | Rights::WRITE | Rights::INSPECT,
            Ok(()),
        );
        mixed_from_handle_disposition(handle_disposition).unwrap();
    }
}
