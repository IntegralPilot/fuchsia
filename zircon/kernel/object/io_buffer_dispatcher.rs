// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher::{
    Dispatcher, DispatcherOps, PeerHolder, PeerHolderMuClass, PeeredState,
    impl_peered_dispatcher_facade_with_state,
};
use super::handle::{HandleValue, KernelHandle};
use super::io_buffer_dispatcher_ffi::{
    cpp_io_buffer_dispatcher_as_child_observer, cpp_io_buffer_dispatcher_create,
};
use super::io_buffer_shared_region_dispatcher::IoBufferSharedRegionDispatcher;
use super::vm_object_dispatcher::VmObjectDispatcher;
use crate::counters::define_kcounter;
use crate::kernel::koid;
use crate::kernel::types::VAddr;
use crate::user_copy::UserInPtr;
use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE};
use crate::vm::pinned_vm_object::PinnedVmObject;
use crate::vm::pmm;
use crate::vm::vm_address_region::flag::DEBUG_DYNAMIC_KERNEL_MAPPING;
use crate::vm::vm_aspace::VmAspace;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object::{Resizability, VmObject, VmObjectChildObserver};
use crate::vm::vm_object_paged::VmObjectPaged;
use core::convert::Infallible;
use core::pin::Pin;
use core::{ptr, slice};
use fbl::{Array, Canary, Recyclable, RefPtr, pin_make_ref_counted, ref_counted};
use iob::{BlobIdAllocator, ZeroFill};
use kalloc::AllocError;
use ksync::{KMutex, LockToken, PhantomMutex, guarded};
use object_constants_rs::{
    kIoBufferDispatcherStateAlign, kIoBufferDispatcherStateOffset, kIoBufferDispatcherStateSize,
};
use page;
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_IOB_ACCESS_EP0_CAN_MAP_READ, ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE,
    ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ, ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
    ZX_IOB_ACCESS_EP1_CAN_MAP_READ, ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE,
    ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ, ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE,
    ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR, ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER,
    ZX_IOB_DISCIPLINE_TYPE_NONE, ZX_IOB_MAX_REGIONS, ZX_IOB_PEER_CLOSED,
    ZX_IOB_REGION_TYPE_PRIVATE, ZX_IOB_REGION_TYPE_SHARED, ZX_MAX_NAME_LEN, ZX_OBJ_TYPE_IOB,
    ZX_RIGHT_DUPLICATE, ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_INSPECT, ZX_RIGHT_MAP, ZX_RIGHT_NONE,
    ZX_RIGHT_READ, ZX_RIGHT_SET_PROPERTY, ZX_RIGHT_SIGNAL, ZX_RIGHT_SIGNAL_PEER, ZX_RIGHT_TRANSFER,
    ZX_RIGHT_WAIT, ZX_RIGHT_WRITE, ZX_USER_SIGNAL_ALL, zx_info_iob_t, zx_iob_region_info_t,
    zx_iob_region_t, zx_iovec_t, zx_koid_t, zx_rights_t,
};

const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_READ
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_GET_PROPERTY
    | ZX_RIGHT_SET_PROPERTY
    | ZX_RIGHT_MAP
    | ZX_RIGHT_SIGNAL
    | ZX_RIGHT_SIGNAL_PEER;

const ALLOWED_SIGNALS: u32 = ZX_USER_SIGNAL_ALL;

zr::static_assert_size_and_align!(
    IoBufferDispatcherState,
    kIoBufferDispatcherStateSize,
    kIoBufferDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_IOB_CREATE_COUNT, "dispatcher.iob.create", Sum);
define_kcounter!(DISPATCHER_IOB_DESTROY_COUNT, "dispatcher.iob.destroy", Sum);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum IobEndpointId {
    Ep0 = 0,
    Ep1 = 1,
}

impl IobEndpointId {
    fn other(self) -> Self {
        match self {
            Self::Ep0 => Self::Ep1,
            Self::Ep1 => Self::Ep0,
        }
    }
}

/// Represents an IOBuffer memory region and its endpoint-specific child VMOs and kernel mapping.
struct IobRegion {
    // A child VMO reference for each endpoint so that they can individually be notified of maps
    // and unmaps.
    ep_vmos: [RefPtr<VmObject>; 2],
    // A mapping of the region into kernel space, or None if the region does not permit mediated
    // access.
    mapping: Option<RefPtr<VmMapping>>,
    // The base address of the mapping.
    base: VAddr,
    region: zx_iob_region_t,
    vmo_user_id: zx_koid_t,
}

impl IobRegion {
    fn new(
        ep0_vmo: RefPtr<VmObject>,
        ep1_vmo: RefPtr<VmObject>,
        mapping: Option<RefPtr<VmMapping>>,
        base: VAddr,
        region: &zx_iob_region_t,
        vmo_user_id: zx_koid_t,
    ) -> Self {
        Self { ep_vmos: [ep0_vmo, ep1_vmo], mapping, base, region: *region, vmo_user_id }
    }

    /// Calculates the map rights for a given endpoint based on region access flags.
    fn get_map_rights(&self, id: IobEndpointId) -> zx_rights_t {
        let (read_flag, write_flag) = match id {
            IobEndpointId::Ep0 => (ZX_IOB_ACCESS_EP0_CAN_MAP_READ, ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE),
            IobEndpointId::Ep1 => (ZX_IOB_ACCESS_EP1_CAN_MAP_READ, ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE),
        };
        let mut rights = 0;
        if (self.region.access & write_flag) != 0 {
            rights |= ZX_RIGHT_WRITE | ZX_RIGHT_MAP;
        }
        if (self.region.access & read_flag) != 0 {
            rights |= ZX_RIGHT_READ | ZX_RIGHT_MAP;
        }
        rights
    }

    /// Calculates the mediated access rights for a given endpoint based on region access flags.
    fn get_mediated_rights(&self, id: IobEndpointId) -> zx_rights_t {
        let (read_flag, write_flag) = match id {
            IobEndpointId::Ep0 => {
                (ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ, ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE)
            }
            IobEndpointId::Ep1 => {
                (ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ, ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE)
            }
        };
        let mut rights = 0;
        if (self.region.access & write_flag) != 0 {
            rights |= ZX_RIGHT_WRITE;
        }
        if (self.region.access & read_flag) != 0 {
            rights |= ZX_RIGHT_READ;
        }
        rights
    }

    /// Returns the child VMO reference for the specified endpoint.
    fn get_vmo(&self, id: IobEndpointId) -> &RefPtr<VmObject> {
        &self.ep_vmos[id as usize]
    }

    /// Returns the `zx_iob_region_info_t` for this region, swapping endpoint access flags if
    /// querying from endpoint 1.
    fn get_region_info(&self, swap_endpoints: bool) -> zx_iob_region_info_t {
        let mut info = zx_iob_region_info_t { region: self.region, koid: self.vmo_user_id };
        if swap_endpoints {
            let ep0_access = info.region.access
                & (ZX_IOB_ACCESS_EP0_CAN_MAP_READ
                    | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE
                    | ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ
                    | ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE);
            let ep1_access = info.region.access
                & (ZX_IOB_ACCESS_EP1_CAN_MAP_READ
                    | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE
                    | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ
                    | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE);
            info.region.access = (ep0_access << 4) | (ep1_access >> 4);
        }
        info
    }

    /// Debug asserts that the discipline is of the expected type, intended as a guardrail for use
    /// in discipline-specific subclasses.
    #[inline(always)]
    fn assert_discipline(&self, discipline_type: u64) {
        debug_assert_eq!(self.region.discipline.r#type, discipline_type);
    }
}

impl Drop for IobRegion {
    fn drop(&mut self) {
        if let Some(mapping) = &self.mapping {
            let _ = mapping.destroy();
        }
    }
}

/// Represents `ZX_IOB_DISCIPLINE_TYPE_NONE`.
type IobRegionNone = IobRegion;

/// Represents `ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR`.
struct IobRegionIdAllocator {
    region: IobRegion,
    _pin: Option<PinnedVmObject>,
}

impl IobRegionIdAllocator {
    /// When mediated access is requested, this initializes the ID allocator container and pins and
    /// maps the whole region mapping for reliable future access.
    fn new(
        ep0_vmo: RefPtr<VmObject>,
        ep1_vmo: RefPtr<VmObject>,
        mapping: Option<RefPtr<VmMapping>>,
        base: VAddr,
        region: &zx_iob_region_t,
        vmo_user_id: zx_koid_t,
    ) -> Result<Self, Status> {
        let pin = if let Some(mapping) = &mapping {
            let vmo = mapping.vmo().expect("mapping must have a VMO");
            let pin = PinnedVmObject::create(vmo, 0, region.size, true)?;
            mapping.map_range(0, region.size as usize, true, false)?;
            // A fresh VMO, so already zero-filled.
            let slice =
                unsafe { slice::from_raw_parts_mut(base.0 as *mut u8, region.size as usize) };
            BlobIdAllocator::init_from_slice(slice, ZeroFill::No);
            Some(pin)
        } else {
            None
        };
        let this = Self {
            region: IobRegion::new(ep0_vmo, ep1_vmo, mapping, base, region, vmo_user_id),
            _pin: pin,
        };
        this.region.assert_discipline(ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR);
        Ok(this)
    }

    /// Returns a view into the whole kernel-mapped region.
    fn mediated_bytes(&self) -> &'static [u8] {
        if self.region.base.0 == 0 {
            return &[];
        }
        // SAFETY: The region is mapped and pinned in the kernel address space for the
        // lifetime of the allocator.
        unsafe {
            slice::from_raw_parts(self.region.base.0 as *const u8, self.region.region.size as usize)
        }
    }

    /// Allocates a blob ID within the ID allocator region.
    fn allocate_id(
        &self,
        id: IobEndpointId,
        blob: UserInPtr<u8>,
        blob_size: usize,
    ) -> Result<u32, Status> {
        self.region.assert_discipline(ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR);
        let mediated_rights = self.region.get_mediated_rights(id);
        if (mediated_rights & ZX_RIGHT_WRITE) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        let allocator = BlobIdAllocator::from_slice(self.mediated_bytes());
        allocator
            .allocate_with(blob_size, |dest| blob.copy_slice_from_user(dest).map(|_| ()))
            .map_err(Into::into)
    }
}

/// Represents `ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER`.
struct IobRegionMediatedWriteRingBuffer {
    region: IobRegion,
    shared_region: RefPtr<IoBufferSharedRegionDispatcher>,
    tag: u64,
}

impl IobRegionMediatedWriteRingBuffer {
    fn new(
        ep0_vmo: RefPtr<VmObject>,
        ep1_vmo: RefPtr<VmObject>,
        region: &zx_iob_region_t,
        vmo_user_id: zx_koid_t,
        shared_region: RefPtr<IoBufferSharedRegionDispatcher>,
        tag: u64,
    ) -> Self {
        let this = Self {
            region: IobRegion::new(ep0_vmo, ep1_vmo, None, VAddr(0), region, vmo_user_id),
            shared_region,
            tag,
        };
        this.region.assert_discipline(ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER);
        this
    }

    /// Performs a mediated write to the shared ring buffer region.
    fn write(
        &self,
        id: IobEndpointId,
        vector: UserInPtr<zx_iovec_t>,
        vector_count: usize,
    ) -> Result<(), Status> {
        self.region.assert_discipline(ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER);
        let mediated_rights = self.region.get_mediated_rights(id);
        if (mediated_rights & ZX_RIGHT_WRITE) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        self.shared_region.write(self.tag, vector, vector_count)
    }
}

/// Enum representing the supported IOBuffer region disciplines.
enum IobRegionVariant {
    None(IobRegionNone),
    IdAllocator(IobRegionIdAllocator),
    MediatedWriteRingBuffer(IobRegionMediatedWriteRingBuffer),
}

impl IobRegionVariant {
    fn base_region(&self) -> &IobRegion {
        match self {
            Self::None(r) => r,
            Self::IdAllocator(r) => &r.region,
            Self::MediatedWriteRingBuffer(r) => &r.region,
        }
    }

    fn get_map_rights(&self, id: IobEndpointId) -> zx_rights_t {
        self.base_region().get_map_rights(id)
    }

    fn get_mediated_rights(&self, id: IobEndpointId) -> zx_rights_t {
        self.base_region().get_mediated_rights(id)
    }

    fn get_vmo(&self, id: IobEndpointId) -> &RefPtr<VmObject> {
        self.base_region().get_vmo(id)
    }

    fn get_region_info(&self, swap_endpoints: bool) -> zx_iob_region_info_t {
        self.base_region().get_region_info(swap_endpoints)
    }

    fn region(&self) -> zx_iob_region_t {
        self.base_region().region
    }

    fn allocate_id(
        &self,
        id: IobEndpointId,
        blob: UserInPtr<u8>,
        blob_size: usize,
    ) -> Result<u32, Status> {
        match self {
            Self::IdAllocator(allocator) => allocator.allocate_id(id, blob, blob_size),
            _ => Err(Status::WRONG_TYPE),
        }
    }

    fn write(
        &self,
        id: IobEndpointId,
        vector: UserInPtr<zx_iovec_t>,
        vector_count: usize,
    ) -> Result<(), Status> {
        match self {
            Self::MediatedWriteRingBuffer(rb) => rb.write(id, vector, vector_count),
            _ => Err(Status::WRONG_TYPE),
        }
    }
}

/// Wrapper struct to allow both peers to hold a reference to the regions.
#[ref_counted]
#[pin_data]
#[derive(Recyclable)]
#[repr(C)]
struct SharedIobState {
    regions: Array<IobRegionVariant>,
}

impl SharedIobState {
    fn create(regions: Array<IobRegionVariant>) -> Result<RefPtr<Self>, AllocError> {
        pin_make_ref_counted!(Self { regions })
    }

    fn get_region(&self, index: usize) -> Result<&IobRegionVariant, Status> {
        self.regions.get(index).ok_or(Status::OUT_OF_RANGE)
    }
}

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct IoBufferDispatcherState {
    canary: Canary<{ fbl::magic(b"IOBD") }>,

    #[pin]
    pub peered: PeeredState<IoBufferDispatcher>,

    shared_state: RefPtr<SharedIobState>,
    endpoint_id: IobEndpointId,

    // Number of regions currently mapped by the peer endpoint.
    #[guarded_by(mu)]
    peer_mapped_regions: usize,
    // Whether the peer endpoint has closed all of its handles.
    #[guarded_by(mu)]
    peer_zero_handles: bool,

    #[mutex(PeerHolderMuClass<IoBufferDispatcher>)]
    pub mu: KMutex<PhantomMutex>,
}

impl IoBufferDispatcherState {
    pub fn init(
        holder: RefPtr<PeerHolder<IoBufferDispatcher>>,
        endpoint_id: usize,
        shared_state: *mut (),
    ) -> impl PinInit<Self, Infallible> {
        DISPATCHER_IOB_CREATE_COUNT.add(1);
        let endpoint_id = if endpoint_id == 0 { IobEndpointId::Ep0 } else { IobEndpointId::Ep1 };
        // SAFETY: `shared_state` is a valid `SharedIobState` pointer passed via `RefPtr::into_raw`.
        let shared_state = unsafe { RefPtr::from_raw(shared_state as *mut SharedIobState) };
        pin_init!(Self {
            canary: Canary::new(),
            peered <- PeeredState::init(holder),
            shared_state,
            endpoint_id,
            peer_mapped_regions: 0.into(),
            peer_zero_handles: false.into(),
            mu: KMutex::new(PhantomMutex),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for IoBufferDispatcherState {
    fn drop(self: Pin<&mut Self>) {
        DISPATCHER_IOB_DESTROY_COUNT.add(1);
        let this = self.project();
        let other_id = this.endpoint_id.other();

        // The other endpoint's VMOs are set up to notify this endpoint when they map regions via a
        // raw pointer (`VmObjectChildObserver*`). Since we're about to be destroyed, we need to
        // unregister.
        //
        // `VmObjectDispatcher` unregisters when the last handle is closed (`on_zero_handles`),
        // but we do it differently here in the destructor.
        //
        // If we were to unregister in `on_peer_zero_handles_locked`, updating the child observer
        // would establish the lock ordering `dispatcher.lock -> child_observer_lock`. However,
        // when `OnZeroChild` callback is invoked from the VMO subsystem, it first acquires
        // `child_observer_lock` and then acquires `dispatcher.lock`, creating an ABBA lock cycle
        // (`child_observer_lock -> dispatcher.lock`).
        //
        // Cleaning up in the destructor avoids this lock inversion, but means that a child
        // notification could potentially occur while we are being destroyed. This is safe because:
        // - Each VMO's child observer is protected by its internal `child_observer_lock`.
        // - While we are in this destructor, a concurrent notification may attempt to acquire
        //   the VMO's `child_observer_lock`.
        // - If we have already cleared the observer, the notifier will observe `nullptr` and
        //   will not access our state.
        // - If we have not yet cleared the observer, it will notify us while continuing to hold
        //   the `child_observer_lock`.
        // - Calling `set_child_observer` will block until any in-flight notification on that VMO
        //   completes and releases the lock.
        // - Thus, destruction cannot proceed past this loop until all in-progress notifications
        //   have finished, and no subsequent observer callback can access our state.
        for region in this.shared_state.regions.iter() {
            let vmo = region.get_vmo(other_id);
            // SAFETY: `vmo` is a valid `RefPtr<VmObject>` owned by `SharedIobState`. Passing
            // `null_mut()` unregisters the observer safely while synchronizing with the VMO's
            // internal `child_observer_lock`, ensuring no further callbacks dereference this
            // dispatcher.
            unsafe {
                vmo.set_child_observer(ptr::null_mut());
            }
        }
    }
}

// An IOBuffer is a memory container for interacting with drivers and other hardware-adjacent
// software.
//
// IOBuffer memory is divided into one or more regions, each of which represents a single VMO
// that is either private to the IOBuffer or shared with another.
//
// Like channels and sockets, IOBuffers are peered objects; each endpoint represents one side
// of the memory conversation.
impl_peered_dispatcher_facade_with_state!(
    pub struct IoBufferDispatcher,
    IoBufferDispatcherState,
    ZX_OBJ_TYPE_IOB,
    kIoBufferDispatcherStateOffset,
    allowed_signals: ALLOWED_SIGNALS,
);

impl IoBufferDispatcher {
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Performs the C++ multiple-inheritance upcast from `IoBufferDispatcher*` to
    /// `VmObjectChildObserver*`.
    pub fn as_child_observer(&self) -> *mut VmObjectChildObserver {
        unsafe { cpp_io_buffer_dispatcher_as_child_observer(self) }
    }

    /// Creates an endpoint pair of `IoBufferDispatcher` objects.
    pub fn create(
        options: u64,
        region_configs: &[zx_iob_region_t],
    ) -> Result<(KernelHandle<Self>, KernelHandle<Self>, zx_rights_t), Status> {
        if options != 0 || region_configs.is_empty() || region_configs.len() > ZX_IOB_MAX_REGIONS {
            return Err(Status::INVALID_ARGS);
        }

        let holder0 = PeerHolder::<Self>::create().map_err(|_| Status::NO_MEMORY)?;
        let holder1 = holder0.clone();

        let regions = Self::create_regions(region_configs)?;
        let shared_state = SharedIobState::create(regions).map_err(|_| Status::NO_MEMORY)?;

        let create_single = |holder: RefPtr<PeerHolder<Self>>,
                             endpoint_id: IobEndpointId,
                             shared_state: RefPtr<SharedIobState>|
         -> Result<KernelHandle<Self>, Status> {
            // SAFETY: `holder` and `shared_state` transfer acquired refcounts, and
            // `cpp_io_buffer_dispatcher_create` initializes `handle` on success.
            unsafe {
                KernelHandle::create(|out| {
                    cpp_io_buffer_dispatcher_create(
                        RefPtr::into_raw(holder) as *mut _,
                        endpoint_id as usize,
                        RefPtr::into_raw(shared_state) as *mut _,
                        out,
                    )
                })
            }
        };

        let handle0 = create_single(holder0, IobEndpointId::Ep0, shared_state.clone())?;
        let handle1 = create_single(holder1, IobEndpointId::Ep1, shared_state.clone())?;

        // Now each endpoint can observe the mappings created by the other.
        for region in shared_state.regions.iter() {
            // SAFETY: `handle0` and `handle1` are valid, initialized dispatchers whose
            // `as_child_observer` upcasts yield valid `VmObjectChildObserver` pointers that
            // remain valid until unregistered during destruction.
            unsafe {
                region
                    .get_vmo(IobEndpointId::Ep0)
                    .set_child_observer(handle1.dispatcher().as_child_observer());
                region
                    .get_vmo(IobEndpointId::Ep1)
                    .set_child_observer(handle0.dispatcher().as_child_observer());
            }
        }

        handle0.dispatcher().init_peer(handle1.dispatcher().clone());
        handle1.dispatcher().init_peer(handle0.dispatcher().clone());

        Ok((handle0, handle1, DEFAULT_RIGHTS))
    }

    fn create_regions(
        region_configs: &[zx_iob_region_t],
    ) -> Result<Array<IobRegionVariant>, Status> {
        let mut regions = Array::<IobRegionVariant>::try_new_uninit_slice(region_configs.len())
            .map_err(|_| Status::NO_MEMORY)?;

        for (i, config) in region_configs.iter().enumerate() {
            let mut region_config = *config;
            let (vmo, vmo_user_id, dispatcher) = match region_config.r#type {
                ZX_IOB_REGION_TYPE_PRIVATE => {
                    let options = unsafe { region_config.extension.private_region.options };
                    let stats = VmObjectDispatcher::parse_create_syscall_flags(
                        options,
                        region_config.size,
                    )?;
                    let created_vmo = VmObjectPaged::create(
                        pmm::ALLOC_FLAG_ANY | pmm::ALLOC_FLAG_CAN_WAIT,
                        stats.flags,
                        stats.size,
                    )?;
                    // parse_create_syscall_flags will round up the size to the nearest page, or set
                    // the size to the maximum possible VMO size if ZX_VMO_UNBOUNDED is used. We
                    // need to know the actual size of the VMO to later return if asked.
                    region_config.size = stats.size;
                    let vmo_user_id = koid::generate();
                    created_vmo.set_user_id(vmo_user_id);
                    let vmo = VmObjectPaged::into_vm_object(created_vmo);
                    (vmo, vmo_user_id, None)
                }
                ZX_IOB_REGION_TYPE_SHARED => {
                    // TODO(https://fxbug.dev/319500512): Remove the cast when we move it out of
                    // vdso next.
                    let shared_region = unsafe { region_config.extension.shared_region };
                    if shared_region.options != 0 || region_config.size != 0 {
                        return Err(Status::INVALID_ARGS);
                    }
                    let sr = Dispatcher::get_with_rights::<IoBufferSharedRegionDispatcher>(
                        HandleValue::new(shared_region.shared_region),
                        ZX_RIGHT_NONE,
                    )?;
                    let vmo = sr.vmo();
                    let paged_vmo =
                        VmObject::downcast_paged(vmo.clone()).ok_or(Status::INVALID_ARGS)?;
                    if paged_vmo.size() < (page::SIZE as u64) * 2 {
                        return Err(Status::INVALID_ARGS);
                    }
                    // For memory attribution to work correctly, we need to use the same KOID for
                    // all IOBuffers that use this shared region.
                    let vmo_user_id = sr.get_koid();
                    (vmo, vmo_user_id, Some(sr))
                }
                _ => return Err(Status::INVALID_ARGS),
            };

            // A note on resource management:
            //
            // Everything we allocate in this loop ultimately gets owned by the SharedIobState and
            // will be cleaned up when both PeeredDispatchers are destroyed and drop their reference
            // to the SharedIobState.
            //
            // However, there is a complication. The VmoChildObservers have a reference to the
            // dispatchers which creates a cycle: IoBufferDispatcher -> SharedIobState -> IobRegion
            // -> VmObject -> VmoChildObserver -> IoBufferDispatcher.
            //
            // Since the VmoChildObservers keep raw pointers to the dispatchers, we need to be sure
            // to reset the corresponding pointers when we destroy an IoBufferDispatcher. Otherwise
            // when an IoBufferDispatcher maps a region, it could try to update a destroyed peer.
            //
            // See: PinnedDrop for IoBufferDispatcherState.

            // We effectively duplicate the logic from sys_vmo_create here, but instead of creating
            // a kernel handle and dispatcher, we keep ownership of it and assign it to a region.

            // In order to track mappings and unmappings separately for each endpoint, we give each
            // endpoint a child reference instead of the created VMO.
            let resizability = if vmo.is_resizable() {
                Resizability::Resizable
            } else {
                Resizability::NonResizable
            };
            let (ep0_reference, _) = vmo.create_child_reference(resizability, 0, 0, true)?;
            let (ep1_reference, _) = vmo.create_child_reference(resizability, 0, 0, true)?;

            ep0_reference.set_user_id(vmo_user_id);
            ep1_reference.set_user_id(vmo_user_id);

            let variant = Self::create_iob_region_variant(
                ep0_reference,
                ep1_reference,
                vmo,
                &region_config,
                vmo_user_id,
                dispatcher,
            )?;
            regions[i].write(variant);
        }

        Ok(unsafe { regions.assume_init() })
    }

    fn create_iob_region_variant(
        ep0_vmo: RefPtr<VmObject>,
        ep1_vmo: RefPtr<VmObject>,
        kernel_vmo: RefPtr<VmObject>,
        region: &zx_iob_region_t,
        vmo_user_id: zx_koid_t,
        shared_region: Option<RefPtr<IoBufferSharedRegionDispatcher>>,
    ) -> Result<IobRegionVariant, Status> {
        let mediated0 = region.access
            & (ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ | ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE);
        let mediated1 = region.access
            & (ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ | ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE);
        let mediated = mediated0 != 0 || mediated1 != 0;
        let map_writable = (region.access & ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE) != 0
            || (region.access & ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE) != 0;

        // The region memory must be directly accessible by userspace or the kernel.
        if !map_writable && !mediated {
            return Err(Status::INVALID_ARGS);
        }

        match region.discipline.r#type {
            ZX_IOB_DISCIPLINE_TYPE_NONE => {
                // For now, disallow shared regions. This can be relaxed as and when need arises.
                if shared_region.is_some() {
                    return Err(Status::INVALID_ARGS);
                }
                if mediated {
                    // NONE type discipline does not support mediated access.
                    return Err(Status::INVALID_ARGS);
                }
                Ok(IobRegionVariant::None(IobRegion::new(
                    ep0_vmo,
                    ep1_vmo,
                    None,
                    VAddr(0),
                    region,
                    vmo_user_id,
                )))
            }
            ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR => {
                // For now, disallow shared regions. This can be relaxed as and when need arises.
                if shared_region.is_some() {
                    return Err(Status::INVALID_ARGS);
                }
                // If an ID allocator endpoint requests mediated access, that access must at
                // least admit mediated writes. Equivalently, no ID allocator endpoint can
                // be read-only in terms of mediated access.
                if mediated0 == ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ
                    || mediated1 == ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ
                {
                    return Err(Status::INVALID_ARGS);
                }
                // Create a mapping of this VMO in the kernel aspace. It is convenient to
                // create the mapping object first before any discipline-specific pinning
                // and mapping is actually performed, allowing that to be done in
                // subclasses, so we pass `DEBUG_DYNAMIC_KERNEL_MAPPING`.
                let kernel_vmar = VmAspace::kernel_aspace()
                    .root_vmar()
                    .expect("kernel aspace root VMAR must be initialized");
                let map_result = kernel_vmar.create_vm_mapping(
                    0,
                    region.size as usize,
                    0,
                    DEBUG_DYNAMIC_KERNEL_MAPPING,
                    kernel_vmo,
                    0,
                    ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
                    c"IOBuffer region (ID allocator)",
                )?;
                let allocator = IobRegionIdAllocator::new(
                    ep0_vmo,
                    ep1_vmo,
                    Some(map_result.mapping),
                    VAddr(map_result.base),
                    region,
                    vmo_user_id,
                )?;
                Ok(IobRegionVariant::IdAllocator(allocator))
            }
            ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER => {
                // For now, insist upon a shared region. This can be relaxed as and when need
                // arises.
                let shared_region = shared_region.ok_or(Status::INVALID_ARGS)?;
                // TODO(https://fxbug.dev/319500512): Remove the cast when we move it out of vdso
                // next.
                let tag = unsafe { region.discipline.extension.ring_buffer.tag };
                Ok(IobRegionVariant::MediatedWriteRingBuffer(
                    IobRegionMediatedWriteRingBuffer::new(
                        ep0_vmo,
                        ep1_vmo,
                        region,
                        vmo_user_id,
                        shared_region,
                        tag,
                    ),
                ))
            }
            _ => {
                // Unknown discipline.
                Err(Status::INVALID_ARGS)
            }
        }
    }

    /// Returns the number of regions configured in this IOBuffer.
    pub fn region_count(&self) -> usize {
        self.state().canary.assert();
        self.state().shared_state.regions.len()
    }

    /// Returns a `zx_info_iob_t` struct containing information about the IOBuffer.
    pub fn get_info(&self) -> zx_info_iob_t {
        self.state().canary.assert();
        zx_info_iob_t { options: 0, region_count: self.region_count() as u32, ..Default::default() }
    }

    /// Returns a `zx_iob_region_info_t` struct for the region at the given index.
    pub fn get_region_info(&self, index: usize) -> zx_iob_region_info_t {
        self.state().canary.assert();
        debug_assert!(index < self.state().shared_state.regions.len());
        self.state().shared_state.regions[index]
            .get_region_info(self.state().endpoint_id == IobEndpointId::Ep1)
    }

    /// Returns the region configuration at the given index.
    pub fn get_region(&self, region_index: usize) -> Result<zx_iob_region_t, Status> {
        self.state().canary.assert();
        self.state().shared_state.get_region(region_index).map(|r| r.region())
    }

    /// Returns the child VMO reference for the specified region and this endpoint.
    pub fn get_vmo(&self, region_index: usize) -> Result<RefPtr<VmObject>, Status> {
        self.state().canary.assert();
        self.state()
            .shared_state
            .get_region(region_index)
            .map(|r| r.get_vmo(self.state().endpoint_id).clone())
    }

    /// Returns the map rights available for the given region, masked by the caller's IOBuffer
    /// rights.
    pub fn get_map_rights(&self, iob_rights: zx_rights_t, region_index: usize) -> zx_rights_t {
        self.state().canary.assert();
        self.state()
            .shared_state
            .get_region(region_index)
            .map_or(0, |r| iob_rights & r.get_map_rights(self.state().endpoint_id))
    }

    /// Returns the mediated access rights for the given region and this endpoint.
    pub fn get_mediated_rights(&self, region_index: usize) -> zx_rights_t {
        self.state().canary.assert();
        self.state()
            .shared_state
            .get_region(region_index)
            .map_or(0, |r| r.get_mediated_rights(self.state().endpoint_id))
    }

    /// Creates a child reference to the region's VMO that can be mapped by userspace.
    pub fn create_mappable_vmo_for_region(
        &self,
        region_index: usize,
    ) -> Result<RefPtr<VmObject>, Status> {
        self.state().canary.assert();
        let vmo = self.get_vmo(region_index)?;
        let resizability =
            if vmo.is_resizable() { Resizability::Resizable } else { Resizability::NonResizable };
        let (child_reference, first_child) =
            vmo.create_child_reference(resizability, 0, 0, true)?;
        if first_child {
            // Need to record 0->1 transition. As we are holding a RefPtr to the child we know that
            // OnZeroChildren cannot get called to decrement before we can increment. In this case
            // the lock is not attempting to synchronize anything beyond the raw access of
            // peer_mapped_regions.
            ksync::lock!(let mut guard = self.state().peered.lock());
            if let Some(peer) = guard.peer().clone() {
                *peer.state().guard_mu_mut(guard.as_mut().token_mut()).peer_mapped_regions_mut() +=
                    1;
            }
        }
        child_reference.set_user_id(vmo.user_id());
        Ok(child_reference)
    }

    /// Attempts to allocate a sequential ID for a blob within an ID allocator region.
    pub fn allocate_id(
        &self,
        region_index: u32,
        blob: UserInPtr<u8>,
        blob_size: usize,
    ) -> Result<u32, Status> {
        self.state().canary.assert();
        self.state().shared_state.get_region(region_index as usize)?.allocate_id(
            self.state().endpoint_id,
            blob,
            blob_size,
        )
    }

    /// Performs a mediated write to the region.
    pub fn write(
        &self,
        region_index: u32,
        vector: UserInPtr<zx_iovec_t>,
        vector_count: usize,
    ) -> Result<(), Status> {
        self.state().canary.assert();
        self.state().shared_state.get_region(region_index as usize)?.write(
            self.state().endpoint_id,
            vector,
            vector_count,
        )
    }

    /// Called by `VmObjectChildObserver` when child VMO references drop to zero.
    pub fn on_zero_child(&self) {
        self.state().canary.assert();
        ksync::lock!(let mut guard = self.state().peered.lock());
        let (is_zero, peer_zero_handles) = {
            let mut state = self.state().guard_mu_mut(guard.as_mut().token_mut());
            let peer_mapped_regions = state.peer_mapped_regions_mut();
            // OnZeroChildren gets called every time the number of children is equal to zero. Due to
            // races, by the time this method is called we could already have children again. This
            // is fine, since all we need to do is ensure that every increment of
            // peer_mapped_regions_ is paired with a decrement.
            debug_assert!(*peer_mapped_regions > 0);
            *peer_mapped_regions -= 1;
            (*peer_mapped_regions == 0, *state.peer_zero_handles())
        };
        // If there are no more mapped regions, and the peer has closed its handles, then we
        // can set the peer closed signal.
        if is_zero && peer_zero_handles {
            self.update_state_locked(guard.token(), 0, ZX_IOB_PEER_CLOSED);
        }
    }

    /// Called when the peer endpoint closes its last handle.
    pub fn on_peer_zero_handles_locked(
        &self,
        token: &mut LockToken<'_, PeerHolderMuClass<IoBufferDispatcher>>,
    ) {
        self.state().canary.assert();
        let is_zero = {
            let mut state = self.state().guard_mu_mut(token);
            *state.peer_zero_handles_mut() = true;
            *state.peer_mapped_regions() == 0
        };
        // If there are no mapped regions we are already closed and need to update state, otherwise
        // we need to wait for on_zero_child.
        if is_zero {
            self.update_state_locked(token, 0, ZX_IOB_PEER_CLOSED);
        }
    }

    pub fn on_zero_handles_locked(
        &self,
        _token: &LockToken<'_, PeerHolderMuClass<IoBufferDispatcher>>,
    ) {
        self.state().canary.assert();
    }

    /// Gets the name of the underlying VMO in the first region.
    pub fn get_name(&self, out_name: &mut [u8; ZX_MAX_NAME_LEN]) -> Result<(), Status> {
        self.state().canary.assert();
        let region =
            self.state().shared_state.regions.first().expect("IoBuffer has at least one region");
        let vmo = region.get_vmo(IobEndpointId::Ep0);
        vmo.get_name(out_name);
        Ok(())
    }

    /// Sets the name on all private region VMOs.
    pub fn set_name(&self, name: &[u8]) -> Result<(), Status> {
        self.state().canary.assert();
        for region in self.state().shared_state.regions.iter() {
            match region {
                IobRegionVariant::None(r)
                | IobRegionVariant::IdAllocator(IobRegionIdAllocator { region: r, .. }) => {
                    if r.region.r#type == ZX_IOB_REGION_TYPE_PRIVATE {
                        r.get_vmo(IobEndpointId::Ep0).set_name(name)?;
                        r.get_vmo(IobEndpointId::Ep1).set_name(name)?;
                    }
                }
                _ => continue,
            }
        }
        Ok(())
    }
}

/// In-tree kernel unit tests for `IoBufferDispatcher`.
#[cfg(ktest)]
#[unittest::suite(name = "io_buffer_dispatcher_rust_tests")]
mod tests {
    use super::{IoBufferDispatcher, IoBufferSharedRegionDispatcher, IobEndpointId};
    use crate::user_copy::UserInPtr;
    use crate::user_memory::UserMemory;
    use crate::vm::pmm::ALLOC_FLAG_ANY;
    use crate::vm::vm_object_paged::VmObjectPaged;
    use core::{mem, ptr, slice};
    use iob::{BlobIdAllocator, Header, Index, ZeroFill};
    use kalloc::Box;
    use page;
    use unittest::{expect_eq, expect_nonnull, expect_ok, expect_true};
    use zx_status::Status;
    use zx_types::{
        ZX_IOB_ACCESS_EP0_CAN_MAP_READ, ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE,
        ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ, ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE,
        ZX_IOB_ACCESS_EP1_CAN_MAP_READ, ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE,
        ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR, ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER,
        ZX_IOB_DISCIPLINE_TYPE_NONE, ZX_IOB_PEER_CLOSED, ZX_IOB_REGION_TYPE_PRIVATE,
        ZX_IOB_REGION_TYPE_SHARED, ZX_MAX_NAME_LEN, ZX_RIGHT_MAP, ZX_RIGHT_READ, ZX_RIGHT_WRITE,
        zx_iob_region_t, zx_iovec_t,
    };

    fn make_user_in(data: &[u8]) -> Option<(UserMemory, UserInPtr<u8>)> {
        let alloc_size = if data.is_empty() { 1 } else { data.len() };
        let mem = UserMemory::create(alloc_size)?;
        mem.commit_and_map(0..alloc_size).ok()?;
        if !data.is_empty() {
            mem.vmo_write(data, 0).ok()?;
        }
        let ptr = UserInPtr::new(mem.base() as *const u8);
        Some((mem, ptr))
    }

    fn make_user_in_iovec(iovecs: &[zx_iovec_t]) -> Option<(UserMemory, UserInPtr<zx_iovec_t>)> {
        let size = mem::size_of_val(iovecs);
        let alloc_size = if size == 0 { 1 } else { size };
        let mem = UserMemory::create(alloc_size)?;
        mem.commit_and_map(0..alloc_size).ok()?;
        if !iovecs.is_empty() {
            let bytes = unsafe { slice::from_raw_parts(iovecs.as_ptr().cast(), size) };
            mem.vmo_write(bytes, 0).ok()?;
        }
        let ptr = UserInPtr::new(mem.base() as *const zx_iovec_t);
        Some((mem, ptr))
    }

    /// Allocate/destroy many iobuffers. Ad hoc resource leak check.
    #[test]
    fn test_create_destroy_many_io_buffers() {
        const MANY: usize = 10_000;

        for region_count in [1, 7, 64] {
            let mut regions = Box::<[zx_iob_region_t]>::try_new_uninit_slice(region_count)
                .expect("allocation failed");
            for uninit_region in regions.iter_mut() {
                let mut region = unsafe { mem::zeroed::<zx_iob_region_t>() };
                region.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
                region.access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE;
                region.size = page::SIZE as u64;
                region.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_NONE;
                uninit_region.write(region);
            }
            let regions = unsafe { regions.assume_init() };
            for _ in 0..MANY {
                expect_ok!(IoBufferDispatcher::create(0, &regions).map(|_| ()));
            }
        }
    }

    /// Tests Header and Index conversions and bounds checking.
    #[test]
    fn test_blob_id_allocator_header_and_index() {
        // Test Header conversion
        let hdr = Header { next_id: 42, blob_head: 4096 };
        let raw = hdr.to_u64();
        let decoded = Header::from_u64(raw);
        expect_true!(hdr == decoded);

        // Test index_end
        expect_true!(Header { next_id: 0, blob_head: 4096 }.index_end() == Some(8));
        expect_true!(Header { next_id: 1, blob_head: 4096 }.index_end() == Some(16));
        expect_true!(Header { next_id: 5, blob_head: 4096 }.index_end() == Some(48));

        const MAX_NEXT_ID: u32 = (u32::MAX - 8) / 8;
        expect_true!(
            Header { next_id: MAX_NEXT_ID, blob_head: 4096 }.index_end()
                == Some(8 + MAX_NEXT_ID * 8)
        );
        expect_true!(Header { next_id: MAX_NEXT_ID + 1, blob_head: 4096 }.index_end().is_none());

        // Test remaining_bytes
        expect_true!(
            Header { next_id: 0, blob_head: 1024 }.remaining_bytes(1024) == Some(1024 - 8)
        );
        expect_true!(
            Header { next_id: 1, blob_head: 1000 }.remaining_bytes(1024) == Some(1000 - 16)
        );
        // Boundary where blob_head == index_end (0 remaining bytes)
        expect_true!(Header { next_id: 0, blob_head: 8 }.remaining_bytes(8) == Some(0));
        // Overlapping index and blobs
        expect_true!(Header { next_id: 1, blob_head: 15 }.remaining_bytes(1024).is_none());
        // Blob head past length
        expect_true!(Header { next_id: 0, blob_head: 2000 }.remaining_bytes(1024).is_none());

        // Test Index conversion
        let idx = Index { size: 100, offset: 500 };
        let raw_idx = idx.to_u64();
        expect_eq!(raw_idx, 100 | (500u64 << 32));
    }

    /// Tests BlobIdAllocator allocation sequence and index table layout.
    #[test]
    fn test_blob_id_allocator_allocation_and_index_layout() {
        const BUF_SIZE: usize = 1024;
        let mut buf = Box::<[u64]>::try_new_zeroed_slice(BUF_SIZE / 8).expect("allocation failed");
        let base = buf.as_mut_ptr() as usize;

        let slice = unsafe { slice::from_raw_parts_mut(base as *mut u8, BUF_SIZE) };
        let alloc = BlobIdAllocator::init_from_slice(slice, ZeroFill::Yes);

        let blob0_data = [0xAAu8; 16];
        let (_mem0, ptr0) = make_user_in(&blob0_data).expect("user mem 0");
        let id0 = alloc
            .allocate_with(blob0_data.len(), |dest| ptr0.copy_slice_from_user(dest).map(|_| ()))
            .expect("allocate blob 0");
        expect_eq!(id0, 0);

        let blob1_data = [0xBBu8; 32];
        let (_mem1, ptr1) = make_user_in(&blob1_data).expect("user mem 1");
        let id1 = alloc
            .allocate_with(blob1_data.len(), |dest| ptr1.copy_slice_from_user(dest).map(|_| ()))
            .expect("allocate blob 1");
        expect_eq!(id1, 1);

        // Verify written data in buffer
        let offset0 = BUF_SIZE - 16;
        let offset1 = offset0 - 32;
        unsafe {
            let slice0 = slice::from_raw_parts((base + offset0) as *const u8, 16);
            expect_true!(slice0 == &blob0_data[..]);

            let slice1 = slice::from_raw_parts((base + offset1) as *const u8, 32);
            expect_true!(slice1 == &blob1_data[..]);

            // Verify index entries
            let idx0_raw = ptr::read_volatile((base + 8) as *const u64);
            expect_eq!(idx0_raw, 16u64 | ((offset0 as u64) << 32));

            let idx1_raw = ptr::read_volatile((base + 16) as *const u64);
            expect_eq!(idx1_raw, 32u64 | ((offset1 as u64) << 32));
        }
    }

    /// Tests BlobIdAllocator out of memory conditions.
    #[test]
    fn test_blob_id_allocator_out_of_memory() {
        const BUF_SIZE: usize = 64;
        let mut buf = Box::<[u64]>::try_new_zeroed_slice(BUF_SIZE / 8).expect("allocation failed");
        let base = buf.as_mut_ptr() as usize;

        let slice = unsafe { slice::from_raw_parts_mut(base as *mut u8, BUF_SIZE) };
        let alloc = BlobIdAllocator::init_from_slice(slice, ZeroFill::No);

        // Buffer is 64 bytes. Header is 8 bytes. Index 0 is 8 bytes.
        // That leaves 48 bytes for data.
        let blob_data = [1u8; 48];
        let (_mem, ptr) = make_user_in(&blob_data).expect("user mem");
        let id = alloc
            .allocate_with(48, |dest| ptr.copy_slice_from_user(dest).map(|_| ()))
            .expect("allocate 48 bytes");
        expect_eq!(id, 0);

        // Now buffer has 0 remaining bytes. Allocating 1 byte fails with NO_MEMORY.
        let blob1_data = [2u8; 1];
        let (_mem1, ptr1) = make_user_in(&blob1_data).expect("user mem 1");
        let res: Result<u32, Status> = alloc
            .allocate_with(1, |dest| ptr1.copy_slice_from_user(dest).map(|_| ()))
            .map_err(Into::into);
        expect_true!(res == Err(Status::NO_MEMORY));
    }

    /// Tests BlobIdAllocator data integrity and corruption handling.
    #[test]
    fn test_blob_id_allocator_corruption_detection() {
        const BUF_SIZE: usize = 128;
        let mut buf = Box::<[u64]>::try_new_zeroed_slice(BUF_SIZE / 8).expect("allocation failed");
        let base = buf.as_mut_ptr() as usize;

        let slice = unsafe { slice::from_raw_parts_mut(base as *mut u8, BUF_SIZE) };
        let alloc = BlobIdAllocator::init_from_slice(slice, ZeroFill::No);

        // Corrupt header by setting blob_head < index_end
        let corrupted_hdr = Header { next_id: 1, blob_head: 8 };
        unsafe {
            ptr::write_volatile(base as *mut u64, corrupted_hdr.to_u64());
        }

        let blob_data = [0u8; 4];
        let (_mem, ptr) = make_user_in(&blob_data).expect("user mem");
        let res: Result<u32, Status> = alloc
            .allocate_with(4, |dest| ptr.copy_slice_from_user(dest).map(|_| ()))
            .map_err(Into::into);
        expect_true!(res == Err(Status::IO_DATA_INTEGRITY));

        // Re-initialize and corrupt index slot to cause CAS failure
        let slice = unsafe { slice::from_raw_parts_mut(base as *mut u8, BUF_SIZE) };
        let alloc = BlobIdAllocator::init_from_slice(slice, ZeroFill::No);
        unsafe {
            // Slot for index 0 is at base + 8. Write non-zero value to it.
            ptr::write_volatile((base + 8) as *mut u64, 0x1234);
        }
        let res2: Result<u32, Status> = alloc
            .allocate_with(4, |dest| ptr.copy_slice_from_user(dest).map(|_| ()))
            .map_err(Into::into);
        expect_true!(res2 == Err(Status::IO_DATA_INTEGRITY));
    }

    /// Tests IobEndpointId enum methods.
    #[test]
    fn test_iob_endpoint_id() {
        expect_true!(IobEndpointId::Ep0.other() == IobEndpointId::Ep1);
        expect_true!(IobEndpointId::Ep1.other() == IobEndpointId::Ep0);
    }

    /// Tests IoBufferDispatcher::create argument validation.
    #[test]
    fn test_create_validation() {
        let mut region = unsafe { mem::zeroed::<zx_iob_region_t>() };
        region.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
        region.access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE;
        region.size = page::SIZE as u64;
        region.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_NONE;

        // Invalid options
        expect_true!(
            IoBufferDispatcher::create(1, &[region]).map(|_| ()) == Err(Status::INVALID_ARGS)
        );

        // Empty regions
        expect_true!(IoBufferDispatcher::create(0, &[]).map(|_| ()) == Err(Status::INVALID_ARGS));

        // No access permissions
        let mut no_access_region = region;
        no_access_region.access = 0;
        expect_true!(
            IoBufferDispatcher::create(0, &[no_access_region]).map(|_| ())
                == Err(Status::INVALID_ARGS)
        );

        // Invalid region type
        let mut invalid_type_region = region;
        invalid_type_region.r#type = 999;
        expect_true!(
            IoBufferDispatcher::create(0, &[invalid_type_region]).map(|_| ())
                == Err(Status::INVALID_ARGS)
        );

        // Invalid discipline: NONE with mediated access
        let mut invalid_disc = region;
        invalid_disc.access |= ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ;
        expect_true!(
            IoBufferDispatcher::create(0, &[invalid_disc]).map(|_| ()) == Err(Status::INVALID_ARGS)
        );

        // Invalid discipline: ID_ALLOCATOR with mediated read only
        let mut id_alloc_ro = region;
        id_alloc_ro.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR;
        id_alloc_ro.access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE;
        expect_true!(
            IoBufferDispatcher::create(0, &[id_alloc_ro]).map(|_| ()) == Err(Status::INVALID_ARGS)
        );

        // Unknown discipline type
        let mut unknown_disc = region;
        unknown_disc.discipline.r#type = 999;
        expect_true!(
            IoBufferDispatcher::create(0, &[unknown_disc]).map(|_| ()) == Err(Status::INVALID_ARGS)
        );
    }

    /// Tests dispatcher methods, region info, rights, and child observers.
    #[test]
    fn test_dispatcher_methods_and_region_info() {
        let mut r0 = unsafe { mem::zeroed::<zx_iob_region_t>() };
        r0.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
        r0.access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ
            | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE
            | ZX_IOB_ACCESS_EP1_CAN_MAP_READ;
        r0.size = page::SIZE as u64;
        r0.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_NONE;

        let mut r1 = unsafe { mem::zeroed::<zx_iob_region_t>() };
        r1.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
        r1.access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ
            | ZX_IOB_ACCESS_EP1_CAN_MAP_READ
            | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE;
        r1.size = page::SIZE as u64;
        r1.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_NONE;

        let (h0, h1, rights) =
            IoBufferDispatcher::create(0, &[r0, r1]).expect("failed to create iob");
        expect_eq!(rights, super::DEFAULT_RIGHTS);
        expect_eq!(IoBufferDispatcher::default_rights(), super::DEFAULT_RIGHTS);

        let d0 = h0.dispatcher();
        let d1 = h1.dispatcher();

        expect_eq!(d0.region_count(), 2);
        expect_eq!(d1.region_count(), 2);

        let info0 = d0.get_info();
        expect_eq!(info0.region_count, 2);
        expect_eq!(info0.options, 0);

        // Query regions
        expect_ok!(d0.get_region(0).map(|_| ()));
        expect_ok!(d0.get_region(1).map(|_| ()));
        expect_true!(d0.get_region(2).map(|_| ()) == Err(Status::OUT_OF_RANGE));

        // Query VMOs
        let vmo0 = d0.get_vmo(0).expect("get_vmo 0");
        let vmo1 = d0.get_vmo(1).expect("get_vmo 1");
        expect_true!(vmo0.user_id() != 0);
        expect_true!(vmo1.user_id() != 0);
        expect_true!(d0.get_vmo(2).map(|_| ()) == Err(Status::OUT_OF_RANGE));

        // Map rights
        expect_eq!(
            d0.get_map_rights(super::DEFAULT_RIGHTS, 0),
            ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP
        );
        expect_eq!(d1.get_map_rights(super::DEFAULT_RIGHTS, 0), ZX_RIGHT_READ | ZX_RIGHT_MAP);
        expect_eq!(d0.get_map_rights(super::DEFAULT_RIGHTS, 1), ZX_RIGHT_READ | ZX_RIGHT_MAP);
        expect_eq!(
            d1.get_map_rights(super::DEFAULT_RIGHTS, 1),
            ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP
        );
        // Masking with limited rights
        expect_eq!(d0.get_map_rights(ZX_RIGHT_READ, 0), ZX_RIGHT_READ);
        expect_eq!(d0.get_map_rights(super::DEFAULT_RIGHTS, 2), 0);

        // Region info swapping
        let rinfo0_ep0 = d0.get_region_info(0);
        let rinfo0_ep1 = d1.get_region_info(0);
        expect_eq!(rinfo0_ep0.region.access, r0.access);
        expect_eq!(
            rinfo0_ep1.region.access,
            ZX_IOB_ACCESS_EP1_CAN_MAP_READ
                | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE
                | ZX_IOB_ACCESS_EP0_CAN_MAP_READ
        );

        // as_child_observer pointer is non-null
        expect_nonnull!(d0.as_child_observer());
        expect_nonnull!(d1.as_child_observer());

        // create_mappable_vmo_for_region
        let mappable = d0.create_mappable_vmo_for_region(0).expect("create mappable vmo");
        expect_eq!(mappable.user_id(), vmo0.user_id());
        expect_true!(d0.create_mappable_vmo_for_region(2).map(|_| ()) == Err(Status::OUT_OF_RANGE));

        // set_name and get_name
        expect_ok!(d0.set_name(b"test_iob_name"));
        let mut name_buf = [0u8; ZX_MAX_NAME_LEN];
        expect_ok!(d0.get_name(&mut name_buf));
        let name_len = name_buf.iter().position(|&b| b == 0).unwrap_or(ZX_MAX_NAME_LEN);
        expect_true!(&name_buf[..name_len] == b"test_iob_name");
    }

    /// Tests ID allocator discipline operations and type safety.
    #[test]
    fn test_id_allocator_and_discipline_type_safety() {
        let mut region = unsafe { mem::zeroed::<zx_iob_region_t>() };
        region.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
        region.access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | ZX_IOB_ACCESS_EP1_CAN_MAP_READ;
        region.size = page::SIZE as u64;
        region.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR;

        let (h0, h1, _) =
            IoBufferDispatcher::create(0, &[region]).expect("create id allocator iob");
        let d0 = h0.dispatcher();
        let d1 = h1.dispatcher();

        let blob_data = [42u8; 16];
        let (_mem, ptr) = make_user_in(&blob_data).expect("user mem");

        // EP0 has mediated write rights -> allocate_id succeeds
        let id = d0.allocate_id(0, ptr, blob_data.len()).expect("allocate_id ep0");
        expect_eq!(id, 0);

        // EP1 lacks mediated write rights -> ACCESS_DENIED
        let res_denied = d1.allocate_id(0, ptr, blob_data.len());
        expect_true!(res_denied == Err(Status::ACCESS_DENIED));

        // Out of range region index -> OUT_OF_RANGE
        let res_oor = d0.allocate_id(1, ptr, blob_data.len());
        expect_true!(res_oor == Err(Status::OUT_OF_RANGE));

        // Calling write on ID allocator discipline -> WRONG_TYPE
        let dummy_iovec = UserInPtr::<zx_iovec_t>::new(core::ptr::null());
        let res_wt = d0.write(0, dummy_iovec, 0);
        expect_true!(res_wt == Err(Status::WRONG_TYPE));
    }

    /// Tests ZX_IOB_PEER_CLOSED signal propagation with mapped child VMO lifecycle.
    #[test]
    fn test_peer_closed_signal_with_child_vmo() {
        let mut region = unsafe { mem::zeroed::<zx_iob_region_t>() };
        region.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
        region.access = ZX_IOB_ACCESS_EP0_CAN_MAP_READ
            | ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE
            | ZX_IOB_ACCESS_EP1_CAN_MAP_READ
            | ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE;
        region.size = page::SIZE as u64;
        region.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_NONE;

        let (h0, h1, _) = IoBufferDispatcher::create(0, &[region]).expect("create iob");
        let d0 = h0.dispatcher();
        let d1 = h1.dispatcher();

        // Create a child VMO on endpoint 1
        let child_vmo = d1.create_mappable_vmo_for_region(0).expect("create child vmo");

        // Drop endpoint 1 handle
        drop(h1);

        // d0 should NOT have PEER_CLOSED yet because child VMO is still alive
        {
            ksync::lock!(let guard = d0.state().peered.lock());
            let sigs = d0.signals_state_locked(guard.token());
            expect_eq!(sigs & ZX_IOB_PEER_CLOSED, 0);
        }

        // Dropping child VMO triggers on_zero_child
        drop(child_vmo);

        // Now d0 should observe PEER_CLOSED
        {
            ksync::lock!(let guard = d0.state().peered.lock());
            let sigs = d0.signals_state_locked(guard.token());
            expect_true!((sigs & ZX_IOB_PEER_CLOSED) != 0);
        }
    }

    /// Tests ID allocator discipline variant and allocator extraction.
    #[test]
    fn test_id_allocator_variant() {
        let page_size = page::SIZE as u64;
        let vmo0 = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, page_size).expect("create vmo0");
        let vmo1 = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, page_size).expect("create vmo1");
        let kernel_vmo =
            VmObjectPaged::create(ALLOC_FLAG_ANY, 0, page_size).expect("create kernel_vmo");
        let vmo0 = VmObjectPaged::into_vm_object(vmo0);
        let vmo1 = VmObjectPaged::into_vm_object(vmo1);
        let kernel_vmo = VmObjectPaged::into_vm_object(kernel_vmo);

        let mut region = unsafe { mem::zeroed::<zx_iob_region_t>() };
        region.r#type = ZX_IOB_REGION_TYPE_PRIVATE;
        region.access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | ZX_IOB_ACCESS_EP1_CAN_MAP_READ;
        region.size = page_size;
        region.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_ID_ALLOCATOR;

        let variant = IoBufferDispatcher::create_iob_region_variant(
            vmo0.clone(),
            vmo1.clone(),
            kernel_vmo,
            &region,
            vmo0.user_id(),
            None,
        )
        .expect("create id allocator variant");

        // EP0 has mediated write rights -> allocate_id succeeds
        let blob_data = [42u8; 16];
        let (_mem, ptr) = make_user_in(&blob_data).expect("user mem");
        let id =
            variant.allocate_id(IobEndpointId::Ep0, ptr, blob_data.len()).expect("allocate_id ep0");
        expect_eq!(id, 0);

        let id = variant
            .allocate_id(IobEndpointId::Ep0, ptr, blob_data.len())
            .expect("allocate_id ep0 second");
        expect_eq!(id, 1);

        // EP1 lacks mediated write rights -> ACCESS_DENIED
        expect_true!(matches!(
            variant.allocate_id(IobEndpointId::Ep1, ptr, blob_data.len()),
            Err(Status::ACCESS_DENIED)
        ));

        // Calling write on ID allocator discipline -> WRONG_TYPE
        let iovec = zx_iovec_t { buffer: ptr.as_ptr().cast_mut(), capacity: blob_data.len() };
        let (_vec_mem, vec_ptr) = make_user_in_iovec(&[iovec]).expect("user vec mem");
        expect_true!(matches!(
            variant.write(IobEndpointId::Ep0, vec_ptr, 1),
            Err(Status::WRONG_TYPE)
        ));
    }

    /// Tests mediated write ring buffer discipline without lock violation.
    #[test]
    fn test_mediated_write_ring_buffer_variant() {
        let page_size = page::SIZE as u64;
        let (sr_handle, _) =
            IoBufferSharedRegionDispatcher::create(2 * page_size).expect("create shared region");
        let sr_disp = sr_handle.dispatcher().clone();

        let vmo0 = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, page_size).expect("create vmo0");
        let vmo1 = VmObjectPaged::create(ALLOC_FLAG_ANY, 0, page_size).expect("create vmo1");
        let vmo0 = VmObjectPaged::into_vm_object(vmo0);
        let vmo1 = VmObjectPaged::into_vm_object(vmo1);

        let mut region = unsafe { mem::zeroed::<zx_iob_region_t>() };
        region.r#type = ZX_IOB_REGION_TYPE_SHARED;
        region.access = ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE | ZX_IOB_ACCESS_EP1_CAN_MAP_READ;
        region.size = 0;
        region.discipline.r#type = ZX_IOB_DISCIPLINE_TYPE_MEDIATED_WRITE_RING_BUFFER;
        region.discipline.extension.ring_buffer.tag = 0x1234_5678;

        let variant = IoBufferDispatcher::create_iob_region_variant(
            vmo0.clone(),
            vmo1.clone(),
            vmo0.clone(),
            &region,
            sr_disp.get_koid(),
            Some(sr_disp.clone()),
        )
        .expect("create mediated write ring buffer variant");

        // Perform write through variant (assert_no_locks_held runs inside shared_region.write)
        let msg_data = [42u8; 8];
        let (_msg_mem, msg_ptr) = make_user_in(&msg_data).expect("user msg mem");
        let iovec = zx_iovec_t { buffer: msg_ptr.as_ptr().cast_mut(), capacity: msg_data.len() };
        let (_vec_mem, vec_ptr) = make_user_in_iovec(&[iovec]).expect("user vec mem");
        expect_ok!(variant.write(IobEndpointId::Ep0, vec_ptr, 1));

        // EP1 lacks mediated write rights -> ACCESS_DENIED
        expect_true!(matches!(
            variant.write(IobEndpointId::Ep1, vec_ptr, 1),
            Err(Status::ACCESS_DENIED)
        ));

        // Calling allocate_id on mediated write ring buffer discipline -> WRONG_TYPE
        expect_true!(matches!(
            variant.allocate_id(IobEndpointId::Ep0, msg_ptr, msg_data.len()),
            Err(Status::WRONG_TYPE)
        ));
    }
}
