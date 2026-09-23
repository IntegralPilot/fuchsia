// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use core::ffi::CStr;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr;
use fbl::{
    Canary, DoublyLinkedList, DoublyLinkedListContainable, DoublyLinkedListNode, RefPtr, UniquePtr,
};
use init::LkInitLevel;
use kprint::kprintln;
use ksync::{
    KCell, KMutex, LockToken, PhantomMutex, RawCriticalMutex, declare_singleton_critical_mutex,
    guarded, kcell_init, lock,
};
use object_constants_rs::{
    kResourceDispatcherStateAlign, kResourceDispatcherStateOffset, kResourceDispatcherStateSize,
};
use pin_init::{
    InPlaceWrite as _, PinInit, pin_data, pin_init, pin_init_array_from_fn, pinned_drop,
};
use range_check::{RangeExt, from_offset_len};
use region_alloc::{AllowOverlap, Region, RegionAllocator, RegionPool, RegionSpan};
use zx_status::Status;
use zx_types::{
    ZX_MAX_NAME_LEN, ZX_OBJ_TYPE_RESOURCE, ZX_RIGHT_DUPLICATE, ZX_RIGHT_GET_PROPERTY,
    ZX_RIGHT_INSPECT, ZX_RIGHT_TRANSFER, ZX_RIGHT_WRITE, ZX_RSRC_FLAG_EXCLUSIVE,
    ZX_RSRC_KIND_IOPORT, ZX_RSRC_KIND_IRQ, ZX_RSRC_KIND_MMIO, ZX_RSRC_KIND_SMC,
    ZX_RSRC_KIND_SYSTEM, zx_info_resource_t, zx_rights_t, zx_rsrc_kind_t,
};

use super::KernelHandle;
use super::resource_dispatcher_ffi::cpp_resource_dispatcher_create;

/// Total number of valid resource kinds.
const ZX_RSRC_KIND_COUNT: u32 = ZX_RSRC_KIND_SYSTEM + 1;

/// Mask of valid resource flags.
pub const ZX_RSRC_FLAGS_MASK: u32 = ZX_RSRC_FLAG_EXCLUSIVE;

/// Default rights for `ResourceDispatcher` handles.
const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_GET_PROPERTY;

/// Maximum size for the global region pool (64 KiB).
const MAX_REGION_POOL_SIZE: usize = 64 << 10;

define_kcounter!(MMIO_RESOURCE_CREATED, "resource.mmio.created", Sum);
define_kcounter!(IRQ_RESOURCE_CREATED, "resource.irq.created", Sum);
define_kcounter!(IOPORT_RESOURCE_CREATED, "resource.ioport.created", Sum);
define_kcounter!(SMC_RESOURCE_CREATED, "resource.smc.created", Sum);
define_kcounter!(SYSTEM_RESOURCE_CREATED, "resource.system.created", Sum);
define_kcounter!(DISPATCHER_RESOURCE_CREATE_COUNT, "dispatcher.resource.create", Sum);
define_kcounter!(DISPATCHER_RESOURCE_DESTROY_COUNT, "dispatcher.resource.destroy", Sum);

/// Returns true if `kind` is a valid resource kind.
///
/// Note: ZX_RSRC_KIND_ROOT (3) was previously used to represent the root resource, but was
/// deprecated and removed from the kernel. The numeric value 3 remains reserved to avoid shifting
/// the values of ZX_RSRC_KIND_SMC (4) and ZX_RSRC_KIND_SYSTEM (5).
pub const fn is_valid_kind(kind: zx_rsrc_kind_t) -> bool {
    kind < ZX_RSRC_KIND_COUNT && kind != /*ZX_RSRC_KIND_ROOT*/ 3
}

const KIND_LABELS: [&str; ZX_RSRC_KIND_COUNT as usize] =
    ["mmio", "irq", "ioport", "deprecated", "smc", "system"];

/// Returns the string label for a resource kind.
fn kind_to_string(kind: zx_rsrc_kind_t) -> &'static str {
    assert!(kind < ZX_RSRC_KIND_COUNT);
    KIND_LABELS[kind as usize]
}

zr::static_assert_size_and_align!(
    ResourceDispatcherState,
    kResourceDispatcherStateSize,
    kResourceDispatcherStateAlign,
);

/// Bookkeeping and allocator storage for resources.
#[guarded]
pub struct ResourceStorage {
    #[mutex(ResourcesLock)]
    mu: KMutex<PhantomMutex>,

    #[pin]
    #[guarded_by(mu)]
    resource_list: DoublyLinkedList<*mut ResourceDispatcher>,

    #[pin]
    #[guarded_by(mu)]
    rallocs: [RegionAllocator; ZX_RSRC_KIND_COUNT as usize],
}

// SAFETY: ResourceStorage contains DoublyLinkedList which holds raw pointers
// (*mut ResourceDispatcher) and is therefore !Send and !Sync by default. All internal access is
// synchronized by ResourcesLock via KMutex<PhantomMutex> and KCell.
unsafe impl Send for ResourceStorage {}
unsafe impl Sync for ResourceStorage {}

impl ResourceStorage {
    /// In-place initializer for `ResourceStorage`.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            mu: KMutex::new(PhantomMutex),
            resource_list <- kcell_init(DoublyLinkedList::new()),
            rallocs <- kcell_init(pin_init_array_from_fn(|_| RegionAllocator::init())),
        })
    }

    /// Returns a reference to the global static storage.
    pub fn get() -> &'static Self {
        // SAFETY: `STATIC_STORAGE` is initialized during early boot via `lk_init_hook` at
        // `LK_INIT_LEVEL_PLATFORM_EARLY`, before any concurrent access or dispatcher creation.
        unsafe { &*ptr::addr_of!(STATIC_STORAGE).cast::<ResourceStorage>() }
    }

    /// Returns the count of tracked active resources in this storage.
    #[cfg(ktest)]
    pub fn resource_count(&self) -> usize {
        lock!(let mut guard = ResourcesLock::lock());
        let storage_guard = self.guard_mu(guard.as_mut().token_mut());
        storage_guard.resource_list().iter().count()
    }
}

// Static tracking data structures for physical address space allocations. Exclusive allocations
// are pulled out of the RegionAllocators, and all allocations are added to the static resource
// list. Shared allocations will check that no exclusive reservation exists, but then release the
// region back to the allocator. Likewise, exclusive allocations will check to ensure that the
// region has not already been allocated as a shared region by checking the static resource list.
declare_singleton_critical_mutex!(ResourcesLock);

// A single global list is used for all resources so that root and hypervisor resources can still
// be tracked, and filtering can be done via client tools/commands when displaying the list is
// concerned.
static mut STATIC_STORAGE: MaybeUninit<ResourceStorage> = MaybeUninit::uninit();

// Global region pool shared across all region allocators, allocated on first use.
static REGION_POOL: KCell<Option<RefPtr<RegionPool>>, ResourcesLock> = KCell::new(None);

/// Initializes `STATIC_STORAGE` during early boot before secondary CPUs or threads are started.
fn init_resource_storage(_level: LkInitLevel) {
    // SAFETY: Called once during single-threaded early boot before any other CPUs or threads exist.
    unsafe {
        let storage = &mut *ptr::addr_of_mut!(STATIC_STORAGE);
        let _ = storage.write_pin_init(ResourceStorage::init());
    }
}

init::lk_init_hook!(
    init_resource_storage,
    init_resource_storage,
    init::LK_INIT_LEVEL_PLATFORM_EARLY
);

/// Returns the global region pool, creating it on first use.
fn get_region_pool(token: &mut LockToken<'_, ResourcesLock>) -> Result<RefPtr<RegionPool>, Status> {
    // SAFETY: The caller proves that `ResourcesLock` is held via `token`.
    let pool_slot = unsafe { REGION_POOL.get_mut(token) };
    if let Some(pool) = pool_slot {
        return Ok(RefPtr::clone(pool));
    }
    let pool = RegionPool::create(MAX_REGION_POOL_SIZE).map_err(|_| Status::NO_MEMORY)?;
    *pool_slot = Some(RefPtr::clone(&pool));
    Ok(pool)
}

/// Internal state storage for `ResourceDispatcher`.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct ResourceDispatcherState {
    canary: Canary<{ fbl::magic(b"RSRC") }>,

    node: DoublyLinkedListNode<ResourceDispatcher>,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,

    kind: zx_rsrc_kind_t,
    flags: u32,
    base: u64,
    size: usize,
    storage: *const ResourceStorage,
    exclusive_region: Option<UniquePtr<Region>>,

    name: [u8; ZX_MAX_NAME_LEN],
}

impl ResourceDispatcherState {
    /// Initializes a `ResourceDispatcherState`.
    ///
    /// # Safety
    ///
    /// - `storage` must point to a valid `ResourceStorage` instance.
    /// - If `region` is non-null, it must be an exclusively owned raw pointer from
    ///   `UniquePtr::into_raw`.
    /// - If `name` is non-null and `name_size > 0`, `name` must point to `name_size` bytes of
    ///   readable memory.
    #[allow(clippy::too_many_arguments)]
    pub fn init(
        _dispatcher: *const ResourceDispatcher,
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
        flags: u32,
        name: *const core::ffi::c_char,
        name_size: usize,
        storage: *mut ResourceStorage,
        region: *mut Region,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_RESOURCE_CREATE_COUNT.add(1);
        match kind {
            ZX_RSRC_KIND_MMIO => MMIO_RESOURCE_CREATED.add(1),
            ZX_RSRC_KIND_IRQ => IRQ_RESOURCE_CREATED.add(1),
            ZX_RSRC_KIND_IOPORT => IOPORT_RESOURCE_CREATED.add(1),
            ZX_RSRC_KIND_SMC => SMC_RESOURCE_CREATED.add(1),
            ZX_RSRC_KIND_SYSTEM => SYSTEM_RESOURCE_CREATED.add(1),
            _ => {}
        }

        let exclusive_region = if region.is_null() {
            None
        } else {
            // SAFETY: `region` was created via `UniquePtr::into_raw` and passed in exclusively.
            Some(unsafe { UniquePtr::from_raw(region) })
        };

        let mut name_buf = [0u8; ZX_MAX_NAME_LEN];
        if !name.is_null() && name_size > 0 {
            // SAFETY: Caller guarantees `name` is valid for `name_size` bytes if non-null.
            let name_slice = unsafe { core::slice::from_raw_parts(name.cast::<u8>(), name_size) };
            let len = name_slice.len().min(ZX_MAX_NAME_LEN - 1);
            name_buf[..len].copy_from_slice(&name_slice[..len]);
        }

        pin_init!(Self {
            canary: Canary::new(),
            node: DoublyLinkedListNode::new(),
            lock <- KMutex::init(),
            kind,
            flags,
            base,
            size,
            storage,
            exclusive_region,
            name: name_buf,
        })
    }

    /// Returns a reference to the enclosing `ResourceDispatcher`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `self` is embedded in a valid `ResourceDispatcher` at
    /// `kResourceDispatcherStateOffset`.
    unsafe fn dispatcher(&self) -> &ResourceDispatcher {
        unsafe {
            &*ptr::from_ref(self)
                .cast::<u8>()
                .sub(kResourceDispatcherStateOffset)
                .cast::<ResourceDispatcher>()
        }
    }

    /// Copies the resource name into `out_name`.
    pub fn get_name(&self, out_name: &mut [u8; ZX_MAX_NAME_LEN]) {
        *out_name = self.name;
    }
}

#[pinned_drop]
impl PinnedDrop for ResourceDispatcherState {
    fn drop(self: Pin<&mut Self>) {
        DISPATCHER_RESOURCE_DESTROY_COUNT.add(1);
        lock!(let mut guard = ResourcesLock::lock());
        let token = guard.as_mut().token_mut();
        // SAFETY: `self.storage` was set at creation and points to `STATIC_STORAGE` or a stack
        // storage that outlives the dispatcher. `ResourcesLock` ensures exclusive list mutation.
        unsafe {
            if let Some(storage) = self.storage.as_ref() {
                let disp = self.dispatcher();
                let _ =
                    storage.guard_mu_mut(token).resource_list_mut().get_unchecked_mut().erase(disp);
            }
        }
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    #[repr(align(8))]
    pub struct ResourceDispatcher,
    ResourceDispatcherState,
    ZX_OBJ_TYPE_RESOURCE,
    kResourceDispatcherStateOffset
);

impl DoublyLinkedListContainable<ResourceDispatcher> for ResourceDispatcher {
    fn get_node(&self) -> &DoublyLinkedListNode<ResourceDispatcher> {
        &self.state().node
    }
}

struct ResourceParams<'a> {
    kind: zx_rsrc_kind_t,
    base: u64,
    size: usize,
    flags: u32,
    name: &'a [u8],
}

impl ResourceDispatcher {
    /// Default rights for resources.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a ResourceDispatcher object representing access rights to a given region of address
    /// space from a particular address space allocator.
    pub fn create(
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
        flags: u32,
        name: &[u8],
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        lock!(let mut guard = ResourcesLock::lock());
        let storage = ResourceStorage::get();
        Self::create_locked(kind, base, size, flags, name, storage, guard.as_mut().token_mut())
    }

    /// Creates a ResourceDispatcher object in a specific `ResourceStorage` instance.
    pub fn create_with_storage(
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
        flags: u32,
        name: &[u8],
        storage: &ResourceStorage,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        lock!(let mut guard = ResourcesLock::lock());
        Self::create_locked(kind, base, size, flags, name, storage, guard.as_mut().token_mut())
    }

    fn create_locked(
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
        flags: u32,
        name: &[u8],
        storage: &ResourceStorage,
        token: &mut LockToken<'_, ResourcesLock>,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        if !is_valid_kind(kind) || (flags & !ZX_RSRC_FLAGS_MASK) != 0 {
            return Err(Status::INVALID_ARGS);
        }

        let req_range = from_offset_len(base, size as u64).ok_or(Status::INVALID_ARGS)?;

        let exclusive_region = {
            let storage_guard = storage.guard_mu(token);
            let allocator = &storage_guard.rallocs()[kind as usize];
            if !allocator.has_region_pool() {
                return Err(Status::BAD_STATE);
            }

            let region = allocator.get_region_specific(RegionSpan { base, size: size as u64 })?;
            if (flags & ZX_RSRC_FLAG_EXCLUSIVE) != 0 {
                if storage_guard.resource_list().iter().any(|rsrc| {
                    rsrc.get_kind() == kind
                        && from_offset_len(rsrc.get_base(), rsrc.get_size() as u64)
                            .is_some_and(|r| req_range.intersects(&r))
                }) {
                    return Err(Status::NOT_FOUND);
                }
                Some(region)
            } else {
                None
            }
        };

        let params = ResourceParams { kind, base, size, flags, name };
        Self::create_handle(params, storage, token, exclusive_region)
    }

    /// Creates a ResourceDispatcher object representing access rights to all regions of address
    /// space for a ranged resource.
    pub fn create_ranged_root(
        kind: zx_rsrc_kind_t,
        name: &[u8],
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        lock!(let mut guard = ResourcesLock::lock());
        let storage = ResourceStorage::get();
        Self::create_ranged_root_locked(kind, name, storage, guard.as_mut().token_mut())
    }

    /// Creates a ranged root ResourceDispatcher object in a specific `ResourceStorage` instance.
    pub fn create_ranged_root_with_storage(
        kind: zx_rsrc_kind_t,
        name: &[u8],
        storage: &ResourceStorage,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        lock!(let mut guard = ResourcesLock::lock());
        Self::create_ranged_root_locked(kind, name, storage, guard.as_mut().token_mut())
    }

    fn create_ranged_root_locked(
        kind: zx_rsrc_kind_t,
        name: &[u8],
        storage: &ResourceStorage,
        token: &mut LockToken<'_, ResourcesLock>,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        if !is_valid_kind(kind) {
            return Err(Status::INVALID_ARGS);
        }

        if !storage.guard_mu(token).rallocs()[kind as usize].has_region_pool() {
            return Err(Status::BAD_STATE);
        }

        let params = ResourceParams { kind, base: 0, size: 0, flags: 0, name };
        Self::create_handle(params, storage, token, None)
    }

    fn create_handle(
        params: ResourceParams<'_>,
        storage: &ResourceStorage,
        token: &mut LockToken<'_, ResourcesLock>,
        exclusive_region: Option<UniquePtr<Region>>,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let region_raw = exclusive_region.map_or(ptr::null_mut(), UniquePtr::into_raw);

        // SAFETY: `cpp_resource_dispatcher_create` allocates and constructs a `ResourceDispatcher`
        // in C++ using the provided parameters. If creation succeeds, `handle` is tracked in
        // `storage.resource_list`; if creation fails, `region_raw` is reclaimed.
        let handle = unsafe {
            let handle = KernelHandle::create(|out| {
                cpp_resource_dispatcher_create(
                    params.kind,
                    params.base,
                    params.size,
                    params.flags,
                    params.name.as_ptr().cast(),
                    params.name.len(),
                    ptr::from_ref(storage).cast_mut(),
                    region_raw,
                    out,
                )
            })
            .inspect_err(|_| {
                if !region_raw.is_null() {
                    let _ = UniquePtr::from_raw(region_raw);
                }
            })?;

            storage
                .guard_mu_mut(token)
                .resource_list_mut()
                .get_unchecked_mut()
                .push_back_raw(RefPtr::as_ptr(handle.dispatcher()).cast_mut());

            handle
        };

        Ok((handle, DEFAULT_RIGHTS))
    }

    /// Initializes the bookkeeping allocator for a specified resource kind.
    pub fn initialize_allocator(
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
    ) -> Result<(), Status> {
        lock!(let mut guard = ResourcesLock::lock());
        let token = guard.as_mut().token_mut();
        let pool = get_region_pool(token)?;
        let storage = ResourceStorage::get();
        Self::initialize_allocator_locked(kind, base, size, storage, pool, token)
    }

    /// Initializes the allocator for a specified resource kind in a specific `ResourceStorage`.
    pub fn initialize_allocator_with_storage(
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
        storage: &ResourceStorage,
    ) -> Result<(), Status> {
        lock!(let mut guard = ResourcesLock::lock());
        let token = guard.as_mut().token_mut();
        let pool = get_region_pool(token)?;
        Self::initialize_allocator_locked(kind, base, size, storage, pool, token)
    }

    fn initialize_allocator_locked(
        kind: zx_rsrc_kind_t,
        base: u64,
        size: usize,
        storage: &ResourceStorage,
        pool: RefPtr<RegionPool>,
        token: &mut LockToken<'_, ResourcesLock>,
    ) -> Result<(), Status> {
        if !is_valid_kind(kind) || size == 0 {
            return Err(Status::INVALID_ARGS);
        }

        let storage_guard = storage.guard_mu(token);
        let allocator = &storage_guard.rallocs()[kind as usize];

        allocator.set_region_pool(pool)?;
        allocator.add_region(RegionSpan { base, size: size as u64 }, AllowOverlap::No)?;

        Ok(())
    }

    /// Returns whether this dispatcher represents a ranged root resource for `kind`.
    pub fn is_ranged_root(&self, kind: zx_rsrc_kind_t) -> bool {
        self.get_kind() == kind && self.get_base() == 0 && self.get_size() == 0
    }

    /// Returns the base address of this resource.
    pub fn get_base(&self) -> u64 {
        self.state().base
    }

    /// Returns the size of this resource.
    pub fn get_size(&self) -> usize {
        self.state().size
    }

    /// Returns the resource kind.
    pub fn get_kind(&self) -> zx_rsrc_kind_t {
        self.state().kind
    }

    /// Returns the resource flags.
    pub fn get_flags(&self) -> u32 {
        self.state().flags
    }

    /// Copies the resource name into `out_name`.
    pub fn get_name(&self, out_name: &mut [u8; ZX_MAX_NAME_LEN]) {
        self.state().get_name(out_name);
    }

    /// Returns the `zx_info_resource_t` diagnostics record.
    pub fn get_info(&self) -> zx_info_resource_t {
        let mut info = zx_info_resource_t {
            kind: self.get_kind(),
            flags: self.get_flags(),
            base: self.get_base(),
            size: self.get_size(),
            name: [0; ZX_MAX_NAME_LEN],
        };
        let mut name_buf = [0u8; ZX_MAX_NAME_LEN];
        self.get_name(&mut name_buf);
        info.name = name_buf;
        info
    }
}

/// Invokes `callback` for each active resource in storage.
fn for_each_resource_locked<F>(
    storage: &ResourceStorage,
    token: &LockToken<'_, ResourcesLock>,
    mut callback: F,
) -> Result<(), Status>
where
    F: FnMut(&ResourceDispatcher) -> Result<(), Status>,
{
    let storage_guard = storage.guard_mu(token);
    for disp in storage_guard.resource_list().iter() {
        callback(disp)?;
    }
    Ok(())
}

/// Prints the active resources list to the kernel console.
pub fn dump_resources() {
    lock!(let mut guard = ResourcesLock::lock());
    let token = guard.as_mut().token_mut();
    let storage = ResourceStorage::get();

    kprintln!("Resources in use:");
    kprintln!(
        "{:>32s}  \t{:>10s}  {:>8s}  \t{:<10s}  \t{:>8s}  {:<32s}",
        "name",
        "type",
        "flags",
        "koid",
        "size",
        "region"
    );
    kprintln!(
        "        -------------------------------------------------------------------------------------------"
    );

    for kind in 0..ZX_RSRC_KIND_COUNT {
        let _ = for_each_resource_locked(storage, token, |r| {
            if r.get_kind() != kind {
                return Ok(());
            }

            let mut name_buf = [0u8; ZX_MAX_NAME_LEN];
            r.get_name(&mut name_buf);
            let name_str = CStr::from_bytes_until_nul(&name_buf)
                .map(|c| c.to_str().unwrap_or(""))
                .unwrap_or("");

            let flag_str = if (r.get_flags() & ZX_RSRC_FLAG_EXCLUSIVE) != 0 { "x" } else { "" };

            if r.get_size() != 0 && r.get_kind() != ZX_RSRC_KIND_SYSTEM {
                if r.get_kind() == ZX_RSRC_KIND_MMIO {
                    let mut size_buf = [0u8; pretty::MAX_FORMAT_SIZE_LEN];
                    let size_str = pretty::format_size_rs(&mut size_buf, r.get_size());
                    kprintln!(
                        "{:>32s}  \t{:>10s}  {:>8s}  \t{:<#10x}  \t{:>8s}  [{:#x}, {:#x})",
                        name_str,
                        kind_to_string(r.get_kind()),
                        flag_str,
                        r.get_koid(),
                        size_str,
                        r.get_base(),
                        r.get_base() + r.get_size() as u64
                    );
                } else {
                    kprintln!(
                        "{:>32s}  \t{:>10s}  {:>8s}  \t{:<#10x}  \t{:>#8x}  [{:#x}, {:#x})",
                        name_str,
                        kind_to_string(r.get_kind()),
                        flag_str,
                        r.get_koid(),
                        r.get_size(),
                        r.get_base(),
                        r.get_base() + r.get_size() as u64
                    );
                }
            } else {
                kprintln!(
                    "{:>32s}  \t{:>10s}  {:>8s}  \t{:<#10x}  \t{:>8s}  {:<32s}",
                    name_str,
                    kind_to_string(r.get_kind()),
                    flag_str,
                    r.get_koid(),
                    "",
                    ""
                );
            }
            Ok(())
        });
    }
}

/// Prints available allocator regions to the kernel console.
pub fn dump_allocators() {
    lock!(let mut guard = ResourcesLock::lock());
    let token = guard.as_mut().token_mut();
    let storage = ResourceStorage::get();

    kprintln!("Available regions:");
    kprintln!("{:>32s}  \t{:>8s}  region", "type", "size");
    kprintln!(
        "        -------------------------------------------------------------------------------------------"
    );

    let storage_guard = storage.guard_mu(token);
    for &kind in &[ZX_RSRC_KIND_MMIO, ZX_RSRC_KIND_IRQ, ZX_RSRC_KIND_IOPORT] {
        storage_guard.rallocs()[kind as usize].walk_available_regions(|region| {
            if kind == ZX_RSRC_KIND_MMIO {
                let mut size_buf = [0u8; pretty::MAX_FORMAT_SIZE_LEN];
                let size_str = pretty::format_size_rs(&mut size_buf, region.size() as usize);
                kprintln!(
                    "{:>32s}  \t{:>8s}  [{:#x}, {:#x})",
                    kind_to_string(kind),
                    size_str,
                    region.base(),
                    region.base() + region.size()
                );
            } else {
                kprintln!(
                    "{:>32s}  \t{:>#8x}  [{:#x}, {:#x})",
                    kind_to_string(kind),
                    region.size(),
                    region.base(),
                    region.base() + region.size()
                );
            }
            true
        });
    }
}

/// Kernel unit tests for ResourceDispatcher.
#[cfg(ktest)]
#[unittest::suite(name = "resource_dispatcher_tests")]
mod tests {
    use super::{ResourceDispatcher, ResourceStorage};
    use pin_init::stack_pin_init;
    use zx_status::Status;
    use zx_types::{ZX_RSRC_FLAG_EXCLUSIVE, ZX_RSRC_KIND_IRQ, ZX_RSRC_KIND_MMIO};

    /// Tests creating resources on unconfigured allocators returns BAD_STATE.
    #[test]
    fn test_unconfigured_allocators() {
        stack_pin_init!(let storage = ResourceStorage::init());

        let res_mmio = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            page::SIZE,
            0,
            b"",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res_mmio.map(|_| ())),
            Status::BAD_STATE.into_raw()
        );

        let res_irq = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_IRQ,
            0,
            page::SIZE,
            0,
            b"",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res_irq.map(|_| ())),
            Status::BAD_STATE.into_raw()
        );

        unittest::expect_eq!(storage.resource_count(), 0);
    }

    /// Tests allocator initialization and double-initialization.
    #[test]
    fn test_allocators_configured() {
        stack_pin_init!(let storage = ResourceStorage::init());

        unittest::expect_ok!(ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            usize::MAX - 1,
            &storage,
        ));

        // Ensure double initialization returns BAD_STATE
        let double_init = ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            u32::MAX as usize - 1,
            &storage,
        );
        unittest::expect_eq!(Status::result_into_raw(double_init), Status::BAD_STATE.into_raw());

        // IRQ allocator initializes successfully
        unittest::expect_ok!(ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_IRQ,
            0,
            256,
            &storage,
        ));
    }

    /// Tests exclusive allocation preventing shared allocation on the same range.
    #[test]
    fn test_exclusive_then_shared() {
        stack_pin_init!(let storage = ResourceStorage::init());

        unittest::expect_ok!(ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            u32::MAX as usize - 1,
            &storage,
        ));

        let base = 0;
        let size = page::SIZE;

        // Creating exclusive resource succeeds
        let (_handle1, _) = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            base,
            size,
            ZX_RSRC_FLAG_EXCLUSIVE,
            b"ets-disp1",
            &storage,
        )
        .expect("creating exclusive resource failed");
        unittest::expect_eq!(storage.resource_count(), 1);

        // Creating shared resource on the same range fails
        let res2 = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            base,
            size,
            0,
            b"ets-disp2",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res2.map(|_| ())),
            Status::NOT_FOUND.into_raw()
        );
        unittest::expect_eq!(storage.resource_count(), 1);
    }

    /// Tests shared allocation preventing exclusive allocation on the same range.
    #[test]
    fn test_shared_then_exclusive() {
        stack_pin_init!(let storage = ResourceStorage::init());

        unittest::expect_ok!(ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            u32::MAX as usize - 1,
            &storage,
        ));

        let base = 0;
        let size = page::SIZE;

        // Creating shared resource succeeds
        let (_handle1, _) = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            base,
            size,
            0,
            b"ste-disp1",
            &storage,
        )
        .expect("creating shared resource failed");
        unittest::expect_eq!(storage.resource_count(), 1);

        // Creating exclusive resource on the same range fails
        let res2 = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            base,
            size,
            ZX_RSRC_FLAG_EXCLUSIVE,
            b"ste-disp2",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res2.map(|_| ())),
            Status::NOT_FOUND.into_raw()
        );
        unittest::expect_eq!(storage.resource_count(), 1);
    }

    /// Tests allocating outside or overlapping the end of allocator bounds.
    #[test]
    fn test_out_of_allocator_range() {
        stack_pin_init!(let storage = ResourceStorage::init());

        let size = 0xFFFF;
        unittest::expect_ok!(ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            size,
            &storage,
        ));

        // Overlap near the end
        let res1 = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            size as u64 - 0xFF,
            0xFFF,
            0,
            b"ooar-disp1",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res1.map(|_| ())),
            Status::NOT_FOUND.into_raw()
        );

        // Outside range entirely
        let res2 = ResourceDispatcher::create_with_storage(
            ZX_RSRC_KIND_MMIO,
            (size + size) as u64,
            size,
            0,
            b"ooar-disp2",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res2.map(|_| ())),
            Status::NOT_FOUND.into_raw()
        );
    }

    /// Tests ranged root creation rules.
    #[test]
    fn test_create_root_ranged() {
        stack_pin_init!(let storage = ResourceStorage::init());

        unittest::expect_ok!(ResourceDispatcher::initialize_allocator_with_storage(
            ZX_RSRC_KIND_MMIO,
            0,
            u32::MAX as usize - 1,
            &storage,
        ));

        // Creating an invalid kind (out of bounds) should fail.
        let res_invalid = ResourceDispatcher::create_ranged_root_with_storage(
            super::ZX_RSRC_KIND_COUNT,
            b"crr-disp1",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res_invalid.map(|_| ())),
            Status::INVALID_ARGS.into_raw()
        );

        // Creating a deprecated kind (3) should fail.
        let res_deprecated = ResourceDispatcher::create_ranged_root_with_storage(
            /*ZX_RSRC_KIND_ROOT*/ 3,
            b"crr-disp-dep",
            &storage,
        );
        unittest::expect_eq!(
            Status::result_into_raw(res_deprecated.map(|_| ())),
            Status::INVALID_ARGS.into_raw()
        );

        // Creating an MMIO ranged root succeeds
        let (_handle_mmio, _) = ResourceDispatcher::create_ranged_root_with_storage(
            ZX_RSRC_KIND_MMIO,
            b"crr-disp2",
            &storage,
        )
        .expect("creating ranged root failed");
        unittest::expect_eq!(storage.resource_count(), 1);
    }
}
