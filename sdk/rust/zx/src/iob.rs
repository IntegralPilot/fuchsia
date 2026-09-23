// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon iobuffer objects.

use crate::{NullableHandle, Status, ok, sys};
use bitflags::bitflags;

mod io_slice;
pub use self::io_slice::*;

// TODO(https://fxbug.dev/389788832): Point this at the right place when the documentation is fixed.
/// An object representing a Zircon
/// [IOBuffer](https://fuchsia.dev/reference/syscalls/iob_create).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Iob(NullableHandle);
impl_handle_based!(Iob);

#[derive(Default)]
pub struct IobOptions;

#[derive(Clone, Copy)]
pub enum IobRegionType<'a> {
    Private { size: u64, options: IobRegionPrivateOptions },
    Shared { options: vdso_next::IobRegionSharedOptions, region: &'a vdso_next::IobSharedRegion },
}

impl IobRegionType<'_> {
    fn to_raw(&self) -> (sys::zx_iob_region_type_t, sys::zx_iob_region_extension_t) {
        match self {
            IobRegionType::Private { .. } => (
                sys::ZX_IOB_REGION_TYPE_PRIVATE,
                sys::zx_iob_region_extension_t { private_region: Default::default() },
            ),
            IobRegionType::Shared { region, .. } => (
                sys::ZX_IOB_REGION_TYPE_SHARED,
                sys::zx_iob_region_extension_t {
                    shared_region: sys::zx_iob_region_shared_t {
                        options: 0,
                        shared_region: region.raw_handle(),
                        padding: Default::default(),
                    },
                },
            ),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct IobRegionPrivateOptions;

pub struct IobRegion<'a> {
    pub region_type: IobRegionType<'a>,
    pub access: IobAccess,
    pub discipline: IobDiscipline,
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct IobAccess: u32 {
        const EP0_CAN_MAP_READ = sys::ZX_IOB_ACCESS_EP0_CAN_MAP_READ;
        const EP0_CAN_MAP_WRITE = sys::ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE;
        const EP0_CAN_MEDIATED_READ = sys::ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ;
        const EP0_CAN_MEDIATED_WRITE = sys::ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE;
        const EP1_CAN_MAP_READ = sys::ZX_IOB_ACCESS_EP1_CAN_MAP_READ;
        const EP1_CAN_MAP_WRITE = sys::ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE;
        const EP1_CAN_MEDIATED_READ = sys::ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ;
        const EP1_CAN_MEDIATED_WRITE = sys::ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE;
    }
}

#[derive(Clone, Copy)]
pub enum IobDiscipline {
    None,
    MediatedWriteRingBuffer { tag: u64 },
}

impl IobDiscipline {
    fn to_raw(&self) -> sys::zx_iob_discipline_t {
        match self {
            IobDiscipline::None => sys::zx_iob_discipline_t {
                r#type: sys::ZX_IOB_DISCIPLINE_TYPE_NONE,
                extension: sys::zx_iob_discipline_extension_t {
                    reserved: [sys::PadByte::default(); 64],
                },
            },
            IobDiscipline::MediatedWriteRingBuffer { tag } => sys::zx_iob_discipline_t {
                r#type: sys::ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER,
                extension: sys::zx_iob_discipline_extension_t {
                    ring_buffer: sys::zx_iob_discipline_mediated_write_ring_buffer_t {
                        tag: *tag,
                        padding: [sys::PadByte::default(); 56],
                    },
                },
            },
        }
    }
}

#[derive(Default)]
pub struct IobWriteOptions;

impl Iob {
    /// Creates an IOBuffer.
    ///
    /// Wraps the [zx_iob_create](https://fuchsia.dev/reference/syscalls/iob_create) syscall.
    pub fn create(_options: IobOptions, regions: &[IobRegion<'_>]) -> Result<(Iob, Iob), Status> {
        let raw_regions: Vec<_> = regions
            .iter()
            .map(|r| {
                let (r#type, extension) = r.region_type.to_raw();
                sys::zx_iob_region_t {
                    r#type,
                    access: r.access.bits(),
                    size: match &r.region_type {
                        IobRegionType::Private { size, .. } => *size,
                        IobRegionType::Shared { .. } => 0,
                    },
                    discipline: r.discipline.to_raw(),
                    extension,
                }
            })
            .collect();
        let mut handle1 = 0;
        let mut handle2 = 0;
        let status = unsafe {
            sys::zx_iob_create(
                0,
                raw_regions.as_ptr() as *const u8,
                raw_regions.len(),
                &mut handle1,
                &mut handle2,
            )
        };
        ok(status)?;
        unsafe {
            Ok((
                Iob::from(NullableHandle::from_raw(handle1)),
                Iob::from(NullableHandle::from_raw(handle2)),
            ))
        }
    }

    /// Performs a mediated write to an IOBuffer region.
    ///
    /// Wraps the [zx_iob_writev](https://fuchsia.dev/reference/syscalls/iob_writev) syscall.
    pub fn write(
        &self,
        options: IobWriteOptions,
        region_index: u32,
        data: &[u8],
    ) -> Result<(), Status> {
        self.writev(options, region_index, &[IobIoSlice::new(data)])
    }

    /// Performs a mediated write (with vectors) to an IOBuffer region.
    ///
    /// Wraps the
    /// [`zx_stream_writev`](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_writev)
    /// syscall.
    pub fn writev(
        &self,
        _options: IobWriteOptions,
        region_index: u32,
        iovecs: &[IobIoSlice<'_>],
    ) -> Result<(), Status> {
        let status = unsafe {
            sys::zx_iob_writev(
                self.raw_handle(),
                0,
                region_index,
                iovecs.as_ptr().cast::<sys::zx_iovec_t>(),
                iovecs.len(),
            )
        };
        ok(status)?;
        Ok(())
    }
}

pub(crate) mod vdso_next {
    use super::*;

    use std::sync::OnceLock;

    #[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
    #[repr(transparent)]
    pub struct IobSharedRegion(NullableHandle);
    impl_handle_based!(IobSharedRegion);

    /// Options for `IobSharedRegion::create`.
    #[derive(Default)]
    pub struct IobSharedRegionOptions;

    /// Options for `IobRegionType` which is used by `Iob::create`.
    #[derive(Clone, Copy, Default)]
    pub struct IobRegionSharedOptions;

    impl IobSharedRegion {
        /// Creates a shared region that can be used with multiple IOBuffer objects.
        ///
        /// Wraps the `zx_iob_create_shared_region` syscall.
        pub fn create(_options: IobSharedRegionOptions, size: u64) -> Result<Self, Status> {
            // We have to go through this dance because we now have shared libraries (e.g. the VFS
            // library) which means we encounter a relocation failure if used with the stable vdso.
            // Weak link attributes are experimental, so we search for the symbol dynamically.
            static ZX_IOB_CREATE_SHARED_REGION_FN: OnceLock<
                unsafe extern "C" fn(u64, u64, *mut sys::zx_handle_t) -> sys::zx_status_t,
            > = OnceLock::new();

            let zx_iob_create_shared_region = ZX_IOB_CREATE_SHARED_REGION_FN.get_or_init(|| {
                // SAFETY: These arguments should be safe to pass to dlsym.
                let symbol = unsafe {
                    libc::dlsym(libc::RTLD_DEFAULT, c"zx_iob_create_shared_region".as_ptr())
                };
                assert!(!symbol.is_null(), "zx_iob_create_shared_region requires vdso next");
                // SAFETY: The above signature should be correct for the symbol we found.
                unsafe { std::mem::transmute(symbol) }
            });

            let mut handle = 0;
            let status = unsafe { zx_iob_create_shared_region(0, size, &mut handle) };
            ok(status)?;
            Ok(Self::from(unsafe { NullableHandle::from_raw(handle) }))
        }
    }

    #[cfg(all(test, vdso_next))]
    mod tests {
        use crate::handle::AsHandleRef;
        use crate::{
            Iob, IobDiscipline, IobRegion, IobRegionType, IobSharedRegion, Unowned, Vmar,
            VmarFlags, sys, system_get_page_size,
        };
        use std::sync::atomic::{AtomicU64, Ordering};

        #[test]
        fn test() {
            let region_size = 2 * system_get_page_size() as usize;

            let shared_region =
                IobSharedRegion::create(region_size as u64, Default::default()).unwrap();

            let (ep0, ep1) = Iob::create(
                Default::default(),
                &[IobRegion {
                    region_type: IobRegionType::Shared(Default::default(), &shared_region),
                    access: sys::ZX_IOB_ACCESS_EP0_CAN_MAP_READ
                        | sys::ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE
                        | sys::ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE,
                    discipline: IobDiscipline::MediatedWriteRingBuffer,
                }],
            )
            .unwrap();

            ep1.write(Default::default(), 0, b"hello").unwrap();

            let vmar_handle = unsafe { fuchsia_runtime::zx_vmar_root_self() };
            let vmar = unsafe { Unowned::<Vmar>::from_raw_handle(vmar_handle) };
            let addr = vmar
                .map_iob(VmarFlags::PERM_READ | VmarFlags::PERM_WRITE, 0, &ep0, 0, 0, region_size)
                .unwrap();

            #[repr(C)]
            struct Header {
                head: AtomicU64,
                tail: AtomicU64,
            }

            let header = unsafe { &*(addr as *const Header) };

            let head = header.head.load(Ordering::Acquire);
            assert_eq!(head, 24);
            let tail = header.tail.load(Ordering::Relaxed);
            assert_eq!(tail, 0);

            struct Message {
                tag: u64,
                length: u64,
                data: [u8; 8],
            }

            let message =
                unsafe { &(*((addr + system_get_page_size() as usize) as *const Message)) };

            assert_eq!(message.tag, ep1.koid().unwrap().raw_koid());
            assert_eq!(message.length, 5);
            assert_eq!(&message.data[..5], b"hello");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Iob, IobAccess, IobDiscipline, IobRegion, IobRegionType};
    use crate::{Unowned, Vmar, VmarFlags};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn test_create_iob() {
        let region_size = zx::system_get_page_size() as usize * 8;
        let (ep0, ep1) = Iob::create(
            Default::default(),
            &[IobRegion {
                region_type: IobRegionType::Private {
                    size: region_size as u64,
                    options: Default::default(),
                },
                access: IobAccess::EP0_CAN_MAP_READ
                    | IobAccess::EP0_CAN_MAP_WRITE
                    | IobAccess::EP1_CAN_MAP_READ,
                discipline: IobDiscipline::None,
            }],
        )
        .expect("create failed");

        // We can't use fuchsia_runtime other than to get the handle, because its Vmar type is
        // considered distinct from crate::Vmar.
        let root_vmar =
            unsafe { Unowned::<Vmar>::from_raw_handle(fuchsia_runtime::zx_vmar_root_self()) };

        let write_addr = root_vmar
            .map_iob(VmarFlags::PERM_READ | VmarFlags::PERM_WRITE, 0, &ep0, 0, 0, region_size)
            .expect("map_iob failed");
        let read_addr = root_vmar
            .map_iob(VmarFlags::PERM_READ, 0, &ep1, 0, 0, region_size)
            .expect("map_iob failed");

        const VALUE: u64 = 0x123456789abcdef;

        unsafe { &*(write_addr as *const AtomicU64) }.store(VALUE, Ordering::Relaxed);

        assert_eq!(unsafe { &*(read_addr as *const AtomicU64) }.load(Ordering::Relaxed), VALUE);

        unsafe {
            root_vmar.unmap(write_addr, region_size).expect("unmap failed");
            root_vmar.unmap(read_addr, region_size).expect("unmap failed");
        }
    }
}
