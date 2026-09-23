// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon iommu objects.

use crate::{NullableHandle, Resource, Status, ok, sys};

/// An object representing a Zircon iommu
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Iommu(NullableHandle);
impl_handle_based!(Iommu);

// ABI-compatible with zx_iommu_desc_stub_t.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub struct IommuDescStub {
    padding1: u8,
}

// TODO(b/459887557): Remove this alias when this name is ready to be completely retired.
pub type IommuDescDummy = IommuDescStub;

impl Iommu {
    // Create an iommu object.
    //
    // Wraps the
    // [`zx_iommu_create`](https://fuchsia.dev/fuchsia-src/reference/syscalls/iommu_create) system call to create an iommu with type `ZX_IOMMU_TYPE_STUB`
    pub fn create_stub(resource: &Resource, desc: IommuDescStub) -> Result<Iommu, Status> {
        let mut iommu_handle = sys::zx_handle_t::default();
        let status = unsafe {
            // SAFETY:
            //  * desc parameter is a valid pointer (desc).
            //  * desc_size parameter is the size of desc.
            sys::zx_iommu_create(
                resource.raw_handle(),
                sys::ZX_IOMMU_TYPE_STUB,
                std::ptr::from_ref(&desc).cast::<u8>(),
                std::mem::size_of_val(&desc),
                &mut iommu_handle,
            )
        };
        ok(status)?;
        unsafe {
            // SAFETY: The syscall docs claim that upon success, iommu_handle will be a valid
            // handle to an iommu object.
            Ok(Iommu::from(NullableHandle::from_raw(iommu_handle)))
        }
    }

    // TODO(b/459887557): Remove this alias when this name is ready to be completely retired.
    pub fn create_dummy(resource: &Resource, desc: IommuDescStub) -> Result<Iommu, Status> {
        Self::create_stub(resource, desc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectType;
    use fidl_fuchsia_kernel as fkernel;
    use fuchsia_component::client::connect_channel_to_protocol;

    #[test]
    fn iommu_create_valid() {
        use zx::{Channel, MonotonicInstant};
        let (client_end, server_end) = Channel::create();
        connect_channel_to_protocol::<fkernel::IommuResourceMarker>(server_end).unwrap();
        let service = fkernel::IommuResourceSynchronousProxy::new(client_end);
        let resource =
            service.get(MonotonicInstant::INFINITE).expect("couldn't get iommu resource");
        // This test and fuchsia-zircon are different crates, so we need
        // to use from_raw to convert between the zx handle and this test handle.
        // See https://fxbug.dev/42173139 for details.
        let resource = unsafe { Resource::from(NullableHandle::from_raw(resource.into_raw())) };
        let iommu = Iommu::create_stub(&resource, IommuDescStub::default()).unwrap();
        assert!(!iommu.as_handle_ref().is_invalid());

        let info = iommu.as_handle_ref().basic_info().unwrap();
        assert_eq!(info.object_type, ObjectType::IOMMU);
    }
}
