// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/vm_address_region_dispatcher.h"

#include <align.h>
#include <assert.h>
#include <inttypes.h>
#include <lib/counters.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>

#define LOCAL_TRACE 0

KCOUNTER(dispatcher_vmar_create_count, "dispatcher.vmar.create")
KCOUNTER(dispatcher_vmar_destroy_count, "dispatcher.vmar.destroy")

namespace {

template <auto FromFlag, auto ToFlag>
auto ExtractFlag(auto* flags) {
  const auto flag_set = *flags & FromFlag;
  // Unconditionally clear |flags| so that the compiler can more easily see that multiple
  // ExtractFlag invocations can just use a single combined clear, greatly reducing code-gen.
  *flags &= ~FromFlag;
  if (flag_set) {
    return ToFlag;
  }
  return static_cast<decltype(ToFlag)>(0);
}

// Split out the syscall flags into vmar flags and mmu flags.  Note that this
// does not validate that the requested protections in *flags* are valid.  For
// that use is_valid_mapping_protection()
zx_status_t split_syscall_flags(uint32_t flags, uint32_t* vmar_flags,
                                arch_mmu_flags_t* arch_mmu_flags, uint8_t* align_pow2) {
  // Figure out arch_mmu_flags
  arch_mmu_flags_t mmu_flags = 0;
  mmu_flags |= ExtractFlag<ZX_VM_PERM_READ, ARCH_MMU_FLAG_PERM_READ>(&flags);
  mmu_flags |= ExtractFlag<ZX_VM_PERM_WRITE, ARCH_MMU_FLAG_PERM_WRITE>(&flags);
  mmu_flags |= ExtractFlag<ZX_VM_PERM_EXECUTE, ARCH_MMU_FLAG_PERM_EXECUTE>(&flags);

  // This flag is no longer needed and should have already been acted upon.
  ExtractFlag<ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED, 0>(&flags);

  // Figure out vmar flags
  uint32_t vmar = 0;
  vmar |= ExtractFlag<ZX_VM_COMPACT, VMAR_FLAG_COMPACT>(&flags);
  vmar |= ExtractFlag<ZX_VM_SPECIFIC, VMAR_FLAG_SPECIFIC>(&flags);
  vmar |= ExtractFlag<ZX_VM_SPECIFIC_OVERWRITE, VMAR_FLAG_SPECIFIC_OVERWRITE>(&flags);
  vmar |= ExtractFlag<ZX_VM_CAN_MAP_SPECIFIC, VMAR_FLAG_CAN_MAP_SPECIFIC>(&flags);
  vmar |= ExtractFlag<ZX_VM_CAN_MAP_READ, VMAR_FLAG_CAN_MAP_READ>(&flags);
  vmar |= ExtractFlag<ZX_VM_CAN_MAP_WRITE, VMAR_FLAG_CAN_MAP_WRITE>(&flags);
  vmar |= ExtractFlag<ZX_VM_CAN_MAP_EXECUTE, VMAR_FLAG_CAN_MAP_EXECUTE>(&flags);
  vmar |= ExtractFlag<ZX_VM_REQUIRE_NON_RESIZABLE, VMAR_FLAG_REQUIRE_NON_RESIZABLE>(&flags);
  vmar |= ExtractFlag<ZX_VM_ALLOW_FAULTS, VMAR_FLAG_ALLOW_FAULTS>(&flags);
  vmar |= ExtractFlag<ZX_VM_OFFSET_IS_UPPER_LIMIT, VMAR_FLAG_OFFSET_IS_UPPER_LIMIT>(&flags);
  vmar |= ExtractFlag<ZX_VM_FAULT_BEYOND_STREAM_SIZE, VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE>(&flags);

  if (flags & ((1u << ZX_VM_ALIGN_BASE) - 1u)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Figure out alignment.
  uint8_t alignment = static_cast<uint8_t>(flags >> ZX_VM_ALIGN_BASE);

  if (((alignment < 10) && (alignment != 0)) || (alignment > 32)) {
    return ZX_ERR_INVALID_ARGS;
  }

  *vmar_flags = vmar;
  *arch_mmu_flags |= mmu_flags;
  *align_pow2 = alignment;
  return ZX_OK;
}

}  // namespace

zx_status_t VmAddressRegionDispatcher::Create(fbl::RefPtr<VmAddressRegion> vmar,
                                              arch_mmu_flags_t base_arch_mmu_flags,
                                              KernelHandle<VmAddressRegionDispatcher>* handle,
                                              zx_rights_t* rights) {
  // The initial rights should match the VMAR's creation permissions
  zx_rights_t vmar_rights = default_rights();
  uint32_t vmar_flags = vmar->flags();
  if (vmar_flags & VMAR_FLAG_CAN_MAP_READ) {
    vmar_rights |= ZX_RIGHT_READ;
  }
  if (vmar_flags & VMAR_FLAG_CAN_MAP_WRITE) {
    vmar_rights |= ZX_RIGHT_WRITE;
  }
  if (vmar_flags & VMAR_FLAG_CAN_MAP_EXECUTE) {
    vmar_rights |= ZX_RIGHT_EXECUTE;
  }

  fbl::AllocChecker ac;
  KernelHandle new_handle(
      fbl::AdoptRef(new (&ac) VmAddressRegionDispatcher(ktl::move(vmar), base_arch_mmu_flags)));
  if (!ac.check())
    return ZX_ERR_NO_MEMORY;

  *rights = vmar_rights;
  *handle = ktl::move(new_handle);
  return ZX_OK;
}

VmAddressRegionDispatcher::VmAddressRegionDispatcher(fbl::RefPtr<VmAddressRegion> vmar,
                                                     arch_mmu_flags_t base_arch_mmu_flags)
    : vmar_(ktl::move(vmar)), base_arch_mmu_flags_(base_arch_mmu_flags) {
  kcounter_add(dispatcher_vmar_create_count, 1);
}

VmAddressRegionDispatcher::~VmAddressRegionDispatcher() {
  kcounter_add(dispatcher_vmar_destroy_count, 1);
}

zx_status_t VmAddressRegionDispatcher::Allocate(size_t offset, size_t size, uint32_t flags,
                                                KernelHandle<VmAddressRegionDispatcher>* handle,
                                                zx_rights_t* new_rights) {
  canary_.Assert();

  uint32_t vmar_flags = 0;
  arch_mmu_flags_t arch_mmu_flags = 0;
  uint8_t alignment = 0;
  zx_status_t status = split_syscall_flags(flags, &vmar_flags, &arch_mmu_flags, &alignment);
  if (status != ZX_OK)
    return status;

  // Check if any MMU-related flags were requested.
  if (arch_mmu_flags != 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  fbl::RefPtr<VmAddressRegion> new_vmar;
  status = vmar_->CreateSubVmar(offset, size, alignment, vmar_flags, "useralloc", &new_vmar);
  if (status != ZX_OK)
    return status;

  return VmAddressRegionDispatcher::Create(ktl::move(new_vmar), base_arch_mmu_flags_, handle,
                                           new_rights);
}

zx_status_t VmAddressRegionDispatcher::Destroy() {
  canary_.Assert();

  // Disallow destroying the root vmar of an aspace as this violates the aspace invariants.
  if (vmar()->aspace()->RootVmar().get() == vmar().get()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  return vmar_->Destroy();
}

zx::result<VmAddressRegionDispatcher::MapResult> VmAddressRegionDispatcher::Map(
    size_t vmar_offset, fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset, size_t len,
    uint32_t flags) {
  canary_.Assert();

  if (!is_valid_mapping_protection(flags)) {
    return zx::error{ZX_ERR_INVALID_ARGS};
  }

  // Split flags into vmar_flags and arch_mmu_flags
  uint32_t vmar_flags = 0;
  arch_mmu_flags_t arch_mmu_flags = base_arch_mmu_flags_;
  uint8_t alignment = 0;
  zx_status_t status = split_syscall_flags(flags, &vmar_flags, &arch_mmu_flags, &alignment);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  if (vmar_flags & VMAR_FLAG_REQUIRE_NON_RESIZABLE) {
    vmar_flags &= ~VMAR_FLAG_REQUIRE_NON_RESIZABLE;
    if (vmo->is_resizable()) {
      return zx::error{ZX_ERR_NOT_SUPPORTED};
    }
  }

  if (vmar_flags & VMAR_FLAG_ALLOW_FAULTS) {
    vmar_flags &= ~VMAR_FLAG_ALLOW_FAULTS;
  } else {
    // TODO(https://fxbug.dev/42109795): Add additional checks once all clients (resizable and
    // pager-backed VMOs) start using the VMAR_FLAG_ALLOW_FAULTS flag.
    if (vmo->is_discardable()) {
      return zx::error{ZX_ERR_NOT_SUPPORTED};
    }
  }

  return vmar_->CreateVmMapping(vmar_offset, len, alignment, vmar_flags, ktl::move(vmo), vmo_offset,
                                arch_mmu_flags, "useralloc");
}

zx_status_t VmAddressRegionDispatcher::Protect(vaddr_t base, size_t len, uint32_t flags,
                                               VmAddressRegionOpChildren op_children) {
  canary_.Assert();

  if (!IsPageRounded(base)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!is_valid_mapping_protection(flags))
    return ZX_ERR_INVALID_ARGS;

  uint32_t vmar_flags = 0;
  arch_mmu_flags_t arch_mmu_flags = base_arch_mmu_flags_;
  uint8_t alignment = 0;
  zx_status_t status = split_syscall_flags(flags, &vmar_flags, &arch_mmu_flags, &alignment);
  if (status != ZX_OK)
    return status;

  // This request does not allow any VMAR flags or alignment flags to be set.
  if (vmar_flags || (alignment != 0))
    return ZX_ERR_INVALID_ARGS;

  return vmar_->Protect(base, len, arch_mmu_flags, op_children);
}

// static
ktl::optional<VmAddressRegion::RangeOpType> VmAddressRegionDispatcher::range_op_type_from_code(
    uint32_t op) {
  switch (op) {
    case ZX_VMAR_OP_COMMIT:
      return VmAddressRegion::RangeOpType::Commit;
    case ZX_VMAR_OP_DECOMMIT:
      return VmAddressRegion::RangeOpType::Decommit;
    case ZX_VMAR_OP_MAP_RANGE:
      return VmAddressRegion::RangeOpType::MapRange;
    case ZX_VMAR_OP_ZERO:
      return VmAddressRegion::RangeOpType::Zero;
    case ZX_VMAR_OP_DONT_NEED:
      return VmAddressRegion::RangeOpType::DontNeed;
    case ZX_VMAR_OP_ALWAYS_NEED:
      return VmAddressRegion::RangeOpType::AlwaysNeed;
    case ZX_VMAR_OP_PREFETCH:
      return VmAddressRegion::RangeOpType::Prefetch;
    default:
      return ktl::nullopt;
  }
}

// static
bool VmAddressRegionDispatcher::is_operation_allowed_from_rights(VmAddressRegion::RangeOpType op,
                                                                 zx_rights_t rights) {
  switch (op) {
    case VmAddressRegion::RangeOpType::Commit:
    case VmAddressRegion::RangeOpType::Decommit:
    case VmAddressRegion::RangeOpType::Zero:
      return (rights & ZX_RIGHT_WRITE) != 0;
    case VmAddressRegion::RangeOpType::Prefetch:
    case VmAddressRegion::RangeOpType::MapRange:
      return (rights & ZX_RIGHT_READ) != 0;
    case VmAddressRegion::RangeOpType::DontNeed:
    case VmAddressRegion::RangeOpType::AlwaysNeed:
      return true;  // just hints
    default:
      panic("invalid range op type %d", static_cast<int>(op));
  }
}

zx_status_t VmAddressRegionDispatcher::RangeOp(uint32_t op, vaddr_t base, size_t len,
                                               zx_rights_t rights, user_inout_ptr<void> buffer,
                                               size_t buffer_size) {
  canary_.Assert();

  const ktl::optional<VmAddressRegion::RangeOpType> which_op = range_op_type_from_code(op);
  if (!which_op.has_value()) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!is_operation_allowed_from_rights(*which_op, rights)) {
    return ZX_ERR_ACCESS_DENIED;
  }

  const VmAddressRegionOpChildren op_children = op_children_from_rights(rights);
  return vmar_->RangeOp(*which_op, base, len, op_children, buffer, buffer_size);
}

zx_status_t VmAddressRegionDispatcher::Unmap(vaddr_t base, size_t len,
                                             VmAddressRegionOpChildren op_children) {
  canary_.Assert();

  if (!IsPageRounded(base)) {
    return ZX_ERR_INVALID_ARGS;
  }

  return vmar_->Unmap(base, len, op_children);
}

zx_status_t VmAddressRegionDispatcher::SetMemoryPriority(
    VmAddressRegion::MemoryPriority memory_priority) {
  canary_.Assert();

  return vmar_->SetMemoryPriority(memory_priority);
}

zx_info_vmar_t VmAddressRegionDispatcher::GetVmarInfo() const {
  zx_info_vmar_t info = {
      .base = vmar_->base(),
      .len = vmar_->size(),
  };
  return info;
}

bool VmAddressRegionDispatcher::is_valid_mapping_protection(uint32_t flags) {
  if (!(flags & ZX_VM_PERM_READ)) {
    // No way to express non-readable mappings that are also writeable or
    // executable.
    if (flags & (ZX_VM_PERM_WRITE | ZX_VM_PERM_EXECUTE)) {
      return false;
    }
  }
  return true;
}
