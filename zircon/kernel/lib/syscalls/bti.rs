// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::mem::MaybeUninit;

use crate::object::{
    BusTransactionInitiatorDispatcher, Dispatcher, HandleValue, IOMMU_FLAG_PERM_EXECUTE,
    IOMMU_FLAG_PERM_READ, IOMMU_FLAG_PERM_WRITE, IommuDispatcher, VmObjectDispatcher, dev_vaddr_t,
};
use crate::user_copy::UserOutPtr;
use debug::ltracef;
use syscalls_macro::syscall;
use zerocopy::{Immutable, IntoBytes};
use zx_status::Status;
use zx_types::{
    ZX_BTI_COMPRESS, ZX_BTI_CONTIGUOUS, ZX_BTI_PERM_EXECUTE, ZX_BTI_PERM_READ, ZX_BTI_PERM_WRITE,
    ZX_RIGHT_MAP, ZX_RIGHT_NONE, ZX_RIGHT_READ, ZX_RIGHT_WRITE, zx_paddr_t,
};

const LOCAL_TRACE: u32 = 0;

/// Helper for optimizing writing many small elements of user ptr array by allowing for a variable
/// amount of buffering.
struct BufferedUserOutPtr<T: IntoBytes + Immutable + Copy, const N: usize> {
    out_ptr: UserOutPtr<T>,
    buffer: [MaybeUninit<T>; N],
    index: usize,
}

impl<T: IntoBytes + Immutable + Copy, const N: usize> BufferedUserOutPtr<T, N> {
    fn new(out_ptr: UserOutPtr<T>) -> Self {
        // Expectation is this is going to be stack allocated, so ensure it's not too big.
        const { assert!(core::mem::size_of::<[MaybeUninit<T>; N]>() < page::SIZE) };
        Self { out_ptr, buffer: [const { MaybeUninit::uninit() }; N], index: 0 }
    }

    /// Add a single element, either appending to the buffer and/or flushing the buffer if full.
    fn write(&mut self, val: T) -> Result<(), Status> {
        self.buffer[self.index].write(val);
        self.index += 1;
        if self.index == N {
            self.flush()?;
        }
        Ok(())
    }

    /// Flush any remaining buffered items. Must be called prior to destruction.
    fn flush(&mut self) -> Result<(), Status> {
        if self.index == 0 {
            return Ok(());
        }
        let count = self.index;
        self.index = 0;
        // SAFETY: `self.buffer[..count]` has been initialized via `write`.
        let slice = unsafe { core::slice::from_raw_parts(self.buffer.as_ptr().cast::<T>(), count) };
        self.out_ptr.copy_slice_to_user(slice)?;
        self.out_ptr = self.out_ptr.element_offset(count);
        Ok(())
    }
}

impl<T: IntoBytes + Immutable + Copy, const N: usize> Drop for BufferedUserOutPtr<T, N> {
    fn drop(&mut self) {
        // Ensure Flush was called and everything got written out.
        assert!(self.index == 0);
    }
}

#[syscall]
pub fn sys_bti_create(
    iommu: HandleValue,
    options: u32,
    bti_id: u64,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("options {options:#x}, bti_id {bti_id}\n");

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // TODO(teisenbe): This should probably have a right on it.
    let iommu_dispatcher = Dispatcher::get_with_rights::<IommuDispatcher>(iommu, ZX_RIGHT_NONE)?;

    let (handle, rights) =
        BusTransactionInitiatorDispatcher::create(iommu_dispatcher.iommu(), bti_id)?;
    *out = handle.make_and_add_handle(rights)?;
    Ok(())
}

#[syscall]
pub fn sys_bti_release_quarantine(handle: HandleValue) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    let bti_dispatcher =
        Dispatcher::get_with_rights::<BusTransactionInitiatorDispatcher>(handle, ZX_RIGHT_WRITE)?;

    bti_dispatcher.release_quarantine();
    Ok(())
}

#[syscall]
pub fn sys_bti_pin(
    handle: HandleValue,
    options: u32,
    vmo: HandleValue,
    offset: u64,
    size: u64,
    addrs: UserOutPtr<zx_paddr_t>,
    addrs_count: usize,
    pmt: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!(
        "handle {:#x}, options {options:#x}, vmo {:#x}, offset {offset:#x}, size {size:#x}, addrs_count {addrs_count}\n",
        handle.raw_value(),
        vmo.raw_value(),
    );

    let bti_dispatcher =
        Dispatcher::get_with_rights::<BusTransactionInitiatorDispatcher>(handle, ZX_RIGHT_MAP)?;

    // Address count is currently limited to the amount of addresses that can fit on 64 pages. This
    // is large enough for all current usage of bti_pin, but protects against the case of an
    // arbitrarily large array being allocated on the heap.
    const MAX_ADDRS: usize = (page::SIZE * 64) / core::mem::size_of::<dev_vaddr_t>();
    if !page::is_aligned(offset as usize)
        || !page::is_aligned(size as usize)
        || addrs_count > MAX_ADDRS
    {
        return Err(Status::INVALID_ARGS);
    }

    let (vmo_dispatcher, vmo_rights) =
        Dispatcher::get_with_rights_and_actual::<VmObjectDispatcher>(vmo, ZX_RIGHT_MAP)?;

    // Convert requested permissions and check against VMO rights
    let mut iommu_perms = 0u32;
    let mut compress_results = false;
    let mut contiguous = false;
    let mut remaining_options = options;

    if (remaining_options & ZX_BTI_PERM_READ) != 0 {
        if (vmo_rights & ZX_RIGHT_READ) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        iommu_perms |= IOMMU_FLAG_PERM_READ;
        remaining_options &= !ZX_BTI_PERM_READ;
    }
    if (remaining_options & ZX_BTI_PERM_WRITE) != 0 {
        if (vmo_rights & ZX_RIGHT_WRITE) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        iommu_perms |= IOMMU_FLAG_PERM_WRITE;
        remaining_options &= !ZX_BTI_PERM_WRITE;
    }
    if (remaining_options & ZX_BTI_PERM_EXECUTE) != 0 {
        // Note: We check ZX_RIGHT_READ instead of ZX_RIGHT_EXECUTE
        // here because the latter applies to execute permission of
        // the host CPU, whereas ZX_BTI_PERM_EXECUTE applies to
        // transactions initiated by the bus device.
        if (vmo_rights & ZX_RIGHT_READ) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        iommu_perms |= IOMMU_FLAG_PERM_EXECUTE;
        remaining_options &= !ZX_BTI_PERM_EXECUTE;
    }
    if !((remaining_options & ZX_BTI_COMPRESS != 0) && (remaining_options & ZX_BTI_CONTIGUOUS != 0))
    {
        if (remaining_options & ZX_BTI_COMPRESS) != 0 {
            compress_results = true;
            remaining_options &= !ZX_BTI_COMPRESS;
        }
        if (remaining_options & ZX_BTI_CONTIGUOUS) != 0 && vmo_dispatcher.vmo().is_contiguous() {
            contiguous = true;
            remaining_options &= !ZX_BTI_CONTIGUOUS;
        }
    }
    if remaining_options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let (new_pmt_handle, new_pmt_rights) =
        bti_dispatcher.pin(vmo_dispatcher.vmo().clone(), offset, size, iommu_perms)?;

    // If anything goes wrong from here on out, we _must_ remember to unpin the
    // PMT we are holding. Failure to do this means that the PMT will hit
    // on-zero-handles while it still has pages pinned and end up in the BTI's
    // quarantine list. This is definitely not correct as the user never got
    // access to the PMT handle in order to unpin the data.
    //
    // Notice that we're holding a RefPtr to the dispatcher rather than a
    // reference to the `new_pmt_handle`. Just before we return, `new_pmt_handle`
    // will be moved in order to make a `HandleValue`. `new_pmt_handle` will
    // not be valid after the move so we keep a RefPtr to the dispatcher instead.
    let pmt_disp = new_pmt_handle.dispatcher().clone();
    let mut cleanup = zr::defer(move || {
        pmt_disp.unpin();
    });

    // Based on the passed in options, determine what size chunks we are going to report to the
    // user, and how many of those there will be.
    let (target_contig, expected_addrs) = if compress_results {
        let min_contig = bti_dispatcher.minimum_contiguity() as usize;
        let expected = (size as usize).div_ceil(min_contig);
        (min_contig, expected)
    } else if contiguous {
        (size as usize, 1)
    } else {
        (page::SIZE, size as usize / page::SIZE)
    };

    if addrs_count != expected_addrs {
        return Err(Status::INVALID_ARGS);
    }

    // Define a helper closure with some state that can fetch potentially large ranges from the
    // PMT, but return them gradually. This just serves as an optimization around repeatedly
    // querying the PMT for a range that it knows is contiguous, but where we need to fill out
    // multiple addresses for the user.
    struct ConsumeState {
        offset: u64,
        addr: dev_vaddr_t,
        remaining: usize,
    }
    let mut consume_state = ConsumeState { offset: 0, addr: 0, remaining: 0 };

    let mut consume_addr = |expected_contig: usize| -> Result<dev_vaddr_t, Status> {
        // If the amount of contiguous memory we are tracking in our consume_state
        // is not enough to satisfy the request, we need to perform another query.
        // Otherwise, we can just consume from the existing consume state.
        if expected_contig > consume_state.remaining {
            let remain = (size - consume_state.offset) as usize;
            let range = new_pmt_handle.dispatcher().query_address(consume_state.offset, remain)?;
            if expected_contig > range.size {
                // This happening suggests an error with contiguity calculations and/or the
                // underlying IOMMU implementation not reporting its contiguity correctly.
                //
                // TODO(teisenbe): consider making this a louder error.
                return Err(Status::INVALID_ARGS);
            }
            consume_state.addr = range.device_vaddr;
            consume_state.remaining = range.size;
        }

        let ret = consume_state.addr;
        consume_state.offset += expected_contig as u64;
        consume_state.addr += expected_contig as dev_vaddr_t;
        consume_state.remaining -= expected_contig;
        Ok(ret)
    };

    const {
        assert!(
            core::mem::size_of::<dev_vaddr_t>() == core::mem::size_of::<zx_paddr_t>(),
            "mismatched types"
        );
    };

    // Calculate / lookup the addresses.
    let mut buffered_addrs =
        BufferedUserOutPtr::<dev_vaddr_t, 32>::new(addrs.reinterpret::<dev_vaddr_t>());
    for i in 0..addrs_count {
        let expected_size = core::cmp::min(size as usize - (target_contig * i), target_contig);
        let addr = consume_addr(expected_size)?;
        buffered_addrs.write(addr)?;
    }

    buffered_addrs.flush()?;
    debug_assert_eq!(buffered_addrs.index, 0);

    let handle = new_pmt_handle.make_and_add_handle(new_pmt_rights)?;
    cleanup.cancel();
    *pmt = handle;
    Ok(())
}
