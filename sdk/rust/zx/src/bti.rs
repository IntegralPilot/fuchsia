// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon bti objects.

use bitflags::bitflags;
use zerocopy::{FromBytes, Immutable};
use zx_sys::zx_paddr_t;

use crate::{Iommu, NullableHandle, ObjectQuery, Pmt, Status, Topic, Vmo, ok, sys};

/// An object representing a Zircon Bus Transaction Initiator object.
/// See [BTI Documentation](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/bus_transaction_initiator) for details.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Bti(NullableHandle);
impl_handle_based!(Bti);

static_assert_align!(
    #[doc="Ergonomic equivalent of [sys::zx_info_bti_t]. Must be ABI-compatible with it."]
    #[repr(C)]
    #[derive(Debug, Copy, Clone, Eq, PartialEq, FromBytes, Immutable)]
    <sys::zx_info_bti_t> pub struct BtiInfo {
        pub minimum_contiguity: u64,
        pub aspace_size: u64,
        pub pmo_count: u64,
        pub quarantine_count: u64,
    }
);

impl Default for BtiInfo {
    fn default() -> BtiInfo {
        Self::from(sys::zx_info_bti_t::default())
    }
}

impl From<sys::zx_info_bti_t> for BtiInfo {
    fn from(info: sys::zx_info_bti_t) -> BtiInfo {
        zerocopy::transmute!(info)
    }
}

struct BtiInfoQuery;
unsafe impl ObjectQuery for BtiInfoQuery {
    const TOPIC: Topic = Topic::BTI;
    type InfoTy = sys::zx_info_bti_t;
}

bitflags! {
    /// Options that may be used for [`zx::Bti::pin`].
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct BtiOptions: u32 {
        const PERM_READ = sys::ZX_BTI_PERM_READ;
        const PERM_WRITE = sys::ZX_BTI_PERM_WRITE;
        const PERM_EXECUTE = sys::ZX_BTI_PERM_EXECUTE;
        const COMPRESS = sys::ZX_BTI_COMPRESS;
        const CONTIGUOUS = sys::ZX_BTI_CONTIGUOUS;
    }
}

impl Bti {
    // Create a Bus Transaction Initiator object.
    // Wraps the
    // [`zx_bti_create`](https://fuchsia.dev/fuchsia-src/reference/syscalls/bti_create) system call to create a bti.
    pub fn create(iommu: &Iommu, id: u64) -> Result<Bti, Status> {
        let mut bti_handle = crate::sys::zx_handle_t::default();
        let status = unsafe {
            // SAFETY: regular system call with no unsafe parameters.
            crate::sys::zx_bti_create(iommu.raw_handle(), 0, id, &mut bti_handle)
        };
        ok(status)?;
        unsafe {
            // SAFETY: The syscall docs claim that upon success, bti_handle will be a valid
            // handle to bti object.
            Ok(Bti::from(NullableHandle::from_raw(bti_handle)))
        }
    }

    /// Wraps the [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_BTI topic.
    pub fn info(&self) -> Result<BtiInfo, Status> {
        Ok(BtiInfo::from(self.0.get_info_single::<BtiInfoQuery>()?))
    }

    // Pins pages in a VMO.
    // Wraps the [`zx_bti_pin`](https://fuchsia.dev/fuchsia-src/reference/syscalls/bti_pin) system
    // call.
    pub fn pin(
        &self,
        options: BtiOptions,
        vmo: &Vmo,
        offset: u64,
        size: u64,
        out_paddrs: &mut [zx_paddr_t],
    ) -> Result<Pmt, Status> {
        let mut pmt = crate::sys::zx_handle_t::default();
        let status = unsafe {
            // SAFETY: zx_bti_pin requires that out_paddrs is valid to write to for its whole
            // length, which is guaranteed by being a mutable slice reference.
            crate::sys::zx_bti_pin(
                self.raw_handle(),
                options.bits(),
                vmo.raw_handle(),
                offset,
                size,
                out_paddrs.as_mut_ptr(),
                out_paddrs.len(),
                &mut pmt,
            )
        };
        ok(status)?;
        unsafe {
            // SAFETY: The syscall docs claim that upon success, pmt will be a valid handle to a
            // zx_pmt_t.
            Ok(Pmt::from(NullableHandle::from_raw(pmt)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IommuDescStub, ObjectType, Resource, Vmo};
    use fidl_fuchsia_kernel as fkernel;
    use fuchsia_component::client::connect_channel_to_protocol;

    #[test]
    fn create_bti_wrong_handle() {
        let vmo = Vmo::create(0).unwrap();
        let wrong_handle = unsafe { Iommu::from(NullableHandle::from_raw(vmo.into_raw())) };

        let status = Bti::create(&wrong_handle, 0);
        assert_eq!(status, Err(Status::WRONG_TYPE));
    }

    fn create_iommu() -> Iommu {
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
        Iommu::create_stub(&resource, IommuDescStub::default()).unwrap()
    }

    #[test]
    fn create_from_valid_iommu() {
        let iommu = create_iommu();
        let bti = Bti::create(&iommu, 0).unwrap();

        let info = bti.as_handle_ref().basic_info().unwrap();
        assert_eq!(info.object_type, ObjectType::BTI);
        let bti_info = bti.info().unwrap();
        // Sanity check one of the info fields.
        assert!(bti_info.minimum_contiguity >= crate::system_get_page_size() as u64);
        assert!(bti_info.minimum_contiguity % crate::system_get_page_size() as u64 == 0);
    }

    #[test]
    fn create_and_pin_from_valid_iommu() {
        let iommu = create_iommu();
        let bti = Bti::create(&iommu, 0).unwrap();

        let vmo = Vmo::create_contiguous(&bti, 4096, 0).unwrap();
        let mut paddr = [0x1]; // Unaligned address, so we know it's invalid

        let pmt = bti.pin(BtiOptions::PERM_READ, &vmo, 0, 4096, &mut paddr[..]).unwrap();
        assert_ne!(paddr, [0x1]);

        let info = pmt.as_handle_ref().basic_info().unwrap();
        assert_eq!(info.object_type, ObjectType::PMT);

        unsafe {
            pmt.unpin().unwrap();
        }
    }
}
