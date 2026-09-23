// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/fit/defer.h>
#include <lib/syscalls/forward.h>
#include <lib/user_copy/user_ptr.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/features.h>
#include <zircon/types.h>

#include <arch/ops.h>
#include <fbl/ref_ptr.h>
#include <object/clock_dispatcher.h>
#include <object/handle.h>
#include <object/process_dispatcher.h>
#include <object/vm_address_region_dispatcher.h>
#include <object/vm_object_dispatcher.h>
#include <vm/vm_address_region.h>
#include <vm/vm_object.h>
#include <vm/vm_object_paged.h>

#define LOCAL_TRACE 0

// zx_status_t zx_vmar_allocate
zx_status_t sys_vmar_allocate(zx_handle_t parent_vmar_handle, zx_vm_option_t options,
                              uint64_t offset, uint64_t size, zx_handle_t* child_vmar,
                              user_out_ptr<zx_vaddr_t> child_addr) {
  auto* up = ProcessDispatcher::GetCurrent();

  // Compute needed rights from requested mapping protections.
  zx_rights_t vmar_rights = 0u;
  if (options & ZX_VM_CAN_MAP_READ) {
    vmar_rights |= ZX_RIGHT_READ;
  }
  if (options & ZX_VM_CAN_MAP_WRITE) {
    vmar_rights |= ZX_RIGHT_WRITE;
  }
  if (options & ZX_VM_CAN_MAP_EXECUTE) {
    vmar_rights |= ZX_RIGHT_EXECUTE;
  }

  // lookup the dispatcher from handle
  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_status_t status =
      up->handle_table().GetDispatcherWithRights(*up, parent_vmar_handle, vmar_rights, &vmar);
  if (status != ZX_OK) {
    return status;
  }

  // Create the new VMAR
  KernelHandle<VmAddressRegionDispatcher> handle;
  zx_rights_t new_rights;
  status = vmar->Allocate(offset, size, options, &handle, &new_rights);
  if (status != ZX_OK) {
    return status;
  }

  // Setup a handler to destroy the new VMAR if the syscall is unsuccessful.
  fbl::RefPtr<VmAddressRegionDispatcher> vmar_dispatcher = handle.dispatcher();
  auto cleanup_handler = fit::defer([&vmar_dispatcher]() { vmar_dispatcher->Destroy(); });

  // Create a handle and attach the dispatcher to it
  status = up->MakeAndAddHandle(ktl::move(handle), new_rights, child_vmar);

  if (status == ZX_OK) {
    status = child_addr.copy_to_user(vmar_dispatcher->vmar()->base());
  }

  if (status == ZX_OK) {
    cleanup_handler.cancel();
  }
  return status;
}

// zx_status_t zx_vmar_destroy
zx_status_t sys_vmar_destroy(zx_handle_t handle) {
  auto* up = ProcessDispatcher::GetCurrent();

  // lookup the dispatcher from handle
  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_status_t status =
      up->handle_table().GetDispatcherWithRights(*up, handle, ZX_RIGHT_OP_CHILDREN, &vmar);
  if (status != ZX_OK) {
    return status;
  }

  return vmar->Destroy();
}

namespace {
zx_status_t vmar_map_common(zx_vm_option_t options, fbl::RefPtr<VmAddressRegionDispatcher> vmar,
                            uint64_t vmar_offset, zx_rights_t vmar_rights,
                            fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset, zx_rights_t vmo_rights,
                            uint64_t len, user_out_ptr<zx_vaddr_t> mapped_addr) {
  // Test to see if we should even be able to map this.
  if (!(vmo_rights & ZX_RIGHT_MAP)) {
    return ZX_ERR_ACCESS_DENIED;
  }

  if ((options & ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED)) {
    if (!(arch_vm_features() & ZX_VM_FEATURE_CAN_MAP_XOM)) {
      options |= ZX_VM_PERM_READ;
    }
  }

  if (!VmAddressRegionDispatcher::is_valid_mapping_protection(options)) {
    return ZX_ERR_INVALID_ARGS;
  }

  bool do_map_range = false;
  if (options & ZX_VM_MAP_RANGE) {
    do_map_range = true;
    options &= ~ZX_VM_MAP_RANGE;
  }

  if (do_map_range && (options & ZX_VM_SPECIFIC_OVERWRITE)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Usermode is not allowed to specify these flags on mappings, though we may
  // set them below.
  if (options & (ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Permissions allowed by both the VMO and the VMAR.
  const bool can_read = (vmo_rights & ZX_RIGHT_READ) && (vmar_rights & ZX_RIGHT_READ);
  const bool can_write = (vmo_rights & ZX_RIGHT_WRITE) && (vmar_rights & ZX_RIGHT_WRITE);
  const bool can_exec = (vmo_rights & ZX_RIGHT_EXECUTE) && (vmar_rights & ZX_RIGHT_EXECUTE);

  // Test to see if the requested mapping protections are allowed.
  if ((options & ZX_VM_PERM_READ) && !can_read) {
    return ZX_ERR_ACCESS_DENIED;
  }
  if ((options & ZX_VM_PERM_WRITE) && !can_write) {
    return ZX_ERR_ACCESS_DENIED;
  }
  if ((options & ZX_VM_PERM_EXECUTE) && !can_exec) {
    return ZX_ERR_ACCESS_DENIED;
  }

  // If a permission is allowed by both the VMO and the VMAR, add it to the
  // flags for the new mapping, so that the VMO's rights as of now can be used
  // to constrain future permission changes via Protect().
  if (can_read) {
    options |= ZX_VM_CAN_MAP_READ;
  }
  if (can_write) {
    options |= ZX_VM_CAN_MAP_WRITE;
  }
  if (can_exec) {
    options |= ZX_VM_CAN_MAP_EXECUTE;
  }

  // Allow faults flag must be used if creating a mapping that can fault.
  if ((options & ZX_VM_FAULT_BEYOND_STREAM_SIZE) && !(options & ZX_VM_ALLOW_FAULTS)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // If fault beyond stream size has been requested, verify that the underlying VMO does, in fact,
  // have a user stream size. This might be false if the mapping is being created from a non
  // VmObjectDispatcher source.
  if (options & ZX_VM_FAULT_BEYOND_STREAM_SIZE) {
    if (VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(vmo.get()); likely(paged)) {
      Guard<CriticalMutex> guard{paged->lock()};
      if (!paged->user_stream_size_locked().has_value()) {
        return ZX_ERR_INVALID_ARGS;
      }
    } else {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  zx::result<VmAddressRegionDispatcher::MapResult> map_result =
      vmar->Map(vmar_offset, ktl::move(vmo), vmo_offset, len, options);
  if (map_result.is_error()) {
    return map_result.status_value();
  }

  // Setup a handler to destroy the new mapping if the syscall is unsuccessful.
  auto cleanup_handler = fit::defer([&map_result]() { map_result->mapping->Destroy(); });

  if (do_map_range) {
    // Mappings may have already been created due to memory priority, so need to ignore existing.
    // Ignoring existing mappings is safe here as we are always free to populate and destroy page
    // table mappings for user addresses.
    zx_status_t status =
        map_result->mapping->MapRange(0, len, /*commit=*/false, /*ignore_existing=*/true);
    if (status != ZX_OK) {
      return status;
    }
  }

  zx_status_t status = mapped_addr.copy_to_user(map_result->base);

  if (status != ZX_OK) {
    return status;
  }

  cleanup_handler.cancel();

  // This mapping will now always be used via the aspace so it is free to be merged into different
  // actual mapping objects.
  VmMapping::MarkMergeable(ktl::move(map_result->mapping));

  return ZX_OK;
}

}  // namespace

// zx_status_t zx_vmar_map
zx_status_t sys_vmar_map(zx_handle_t handle, zx_vm_option_t options, uint64_t vmar_offset,
                         zx_handle_t vmo_handle, uint64_t vmo_offset, uint64_t len,
                         user_out_ptr<zx_vaddr_t> mapped_addr) {
  auto* up = ProcessDispatcher::GetCurrent();

  // lookup the VMAR dispatcher from handle
  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_rights_t vmar_rights;
  zx_status_t status = up->handle_table().GetDispatcherAndRights(*up, handle, &vmar, &vmar_rights);
  if (status != ZX_OK) {
    return status;
  }

  // lookup the VMO dispatcher from handle
  fbl::RefPtr<VmObjectDispatcher> vmo;
  zx_rights_t vmo_rights;
  status = up->handle_table().GetDispatcherAndRights(*up, vmo_handle, &vmo, &vmo_rights);
  if (status != ZX_OK) {
    return status;
  }

  // Allocate SSM if creating a fault-beyond-stream-size mapping.
  if (options & ZX_VM_FAULT_BEYOND_STREAM_SIZE) {
    status = vmo->stream_size_manager().status_value();
    if (status != ZX_OK) {
      return status;
    }
  }

  return vmar_map_common(options, ktl::move(vmar), vmar_offset, vmar_rights, vmo->vmo(), vmo_offset,
                         vmo_rights, len, mapped_addr);
}

// zx_status_t zx_vmar_unmap
zx_status_t sys_vmar_unmap(zx_handle_t handle, zx_vaddr_t addr, uint64_t len) {
  auto* up = ProcessDispatcher::GetCurrent();

  // lookup the dispatcher from handle
  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_rights_t vmar_rights;
  zx_status_t status = up->handle_table().GetDispatcherAndRights(*up, handle, &vmar, &vmar_rights);
  if (status != ZX_OK) {
    return status;
  }

  return vmar->Unmap(addr, len, VmAddressRegionDispatcher::op_children_from_rights(vmar_rights));
}

// zx_status_t zx_vmar_protect
zx_status_t sys_vmar_protect(zx_handle_t handle, zx_vm_option_t options, zx_vaddr_t addr,
                             uint64_t len) {
  auto* up = ProcessDispatcher::GetCurrent();

  if ((options & ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED)) {
    if (!(arch_vm_features() & ZX_VM_FEATURE_CAN_MAP_XOM)) {
      options |= ZX_VM_PERM_READ;
    }
  }

  zx_rights_t vmar_rights = 0u;
  if (options & ZX_VM_PERM_READ) {
    vmar_rights |= ZX_RIGHT_READ;
  }
  if (options & ZX_VM_PERM_WRITE) {
    vmar_rights |= ZX_RIGHT_WRITE;
  }
  if (options & ZX_VM_PERM_EXECUTE) {
    vmar_rights |= ZX_RIGHT_EXECUTE;
  }

  // lookup the dispatcher from handle
  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_status_t status =
      up->handle_table().GetDispatcherWithRights(*up, handle, vmar_rights, &vmar, &vmar_rights);
  if (status != ZX_OK) {
    return status;
  }

  if (!VmAddressRegionDispatcher::is_valid_mapping_protection(options)) {
    return ZX_ERR_INVALID_ARGS;
  }

  return vmar->Protect(addr, len, options,
                       VmAddressRegionDispatcher::op_children_from_rights(vmar_rights));
}

// zx_status_t zx_vmar_op_range
zx_status_t sys_vmar_op_range(zx_handle_t handle, uint32_t op, zx_vaddr_t addr, uint64_t len,
                              user_inout_ptr<void> _buffer, size_t buffer_size) {
  auto* up = ProcessDispatcher::GetCurrent();

  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_rights_t vmar_rights;
  zx_status_t status = up->handle_table().GetDispatcherAndRights(*up, handle, &vmar, &vmar_rights);
  if (status != ZX_OK) {
    return status;
  }

  return vmar->RangeOp(op, addr, len, vmar_rights, _buffer, buffer_size);
}

extern "C" zx_status_t cpp_vmar_map_common(zx_vm_option_t options, VmAddressRegionDispatcher* vmar,
                                           uint64_t vmar_offset, zx_rights_t vmar_rights,
                                           VmObject* vmo, uint64_t vmo_offset,
                                           zx_rights_t vmo_rights, uint64_t len,
                                           zx_vaddr_t* mapped_addr) {
  return vmar_map_common(options, fbl::ImportFromRawPtr(vmar), vmar_offset, vmar_rights,
                         fbl::ImportFromRawPtr(vmo), vmo_offset, vmo_rights, len,
                         make_user_out_ptr(mapped_addr));
}

zx_status_t sys_vmar_map_clock(zx_handle_t handle, zx_vm_option_t options, uint64_t vmar_offset,
                               zx_handle_t clock_handle, uint64_t len,
                               user_out_ptr<zx_vaddr_t> mapped_addr) {
  // Pretty much all of the options are allowed when attempting to map a clock's
  // VMO, but not all of them.  Check out the options requested by the user and
  // reject the call if any of the explicitly disallowed options are present in
  // the request.  Leave the rest of the option validation logic to the common
  // map routine.
  constexpr zx_vm_option_t kDisallowedOptions =
      ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE | ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED;
  if ((options & kDisallowedOptions) != 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  // The length of the requested mapping must be what we expect it to be, in
  // this case, the value reported by the ZX_INFO_CLOCK_MAPPED_SIZE topic.
  // Anything else is an error.
  if (len != ClockDispatcher::kMappedSize) {
    return ZX_ERR_INVALID_ARGS;
  }

  // lookup the Clock dispatcher from handle
  auto* up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<ClockDispatcher> clock;
  zx_rights_t clock_rights;
  if (zx_status_t status =
          up->handle_table().GetDispatcherAndRights(*up, clock_handle, &clock, &clock_rights);
      status != ZX_OK) {
    return status;
  }

  // If this is not a mappable clock, then there is no point in proceeding.
  if (!clock->is_mappable()) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Grab a reference to the internal VMO which we can pass to the common map
  // routine.  It should be impossible to have successfully created a clock
  // whose options indicate that it is mappable, but which does not have a valid
  // underlying VMO.
  fbl::RefPtr<VmObject> clock_vmo = clock->vmo();
  DEBUG_ASSERT(clock_vmo.get() != nullptr);

  // lookup the VMAR dispatcher from handle
  fbl::RefPtr<VmAddressRegionDispatcher> vmar;
  zx_rights_t vmar_rights;
  if (zx_status_t status =
          up->handle_table().GetDispatcherAndRights(*up, handle, &vmar, &vmar_rights);
      status != ZX_OK) {
    return status;
  }

  // In order to map a clock, users must have both the READ and MAP permissions.
  // Mask out all of the other permissions to act as the "effective" permissions
  // for the underlying VMO that this clock owns.  We will pass these effective
  // rights as the VMO rights to the common mapping function.
  constexpr zx_rights_t kRequiredClockRights = ZX_RIGHT_READ | ZX_RIGHT_MAP;
  zx_rights_t effective_vmo_rights = clock_rights & kRequiredClockRights;

  // Finally hand off the map operation to the common map routine.
  return vmar_map_common(options, ktl::move(vmar), vmar_offset, vmar_rights, ktl::move(clock_vmo),
                         0, effective_vmo_rights, len, mapped_addr);
}
