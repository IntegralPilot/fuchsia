// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "vm/vm_address_region.h"

#include <align.h>
#include <assert.h>
#include <inttypes.h>
#include <lib/counters.h>
#include <lib/crypto/prng.h>
#include <lib/page/size.h>
#include <lib/userabi/vdso.h>
#include <pow2.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <kernel/mp.h>
#include <ktl/algorithm.h>
#include <ktl/limits.h>
#include <vm/fault.h>
#include <vm/vm.h>
#include <vm/vm_address_region_enumerator.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>

#include "vm_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE VM_GLOBAL_TRACE(0)

// Number of attempted address range mapping, regardless arguments.
KCOUNTER(vm_region_map_all_attempt, "vm.region.map.all.attempt")
// Number of successful address range mapping.
KCOUNTER(vm_region_map_all_success, "vm.region.map.all.success")
// Number of attempted address range mapping with a requested upper limit.
KCOUNTER(vm_region_map_specific_attempt, "vm.region.map.specific.attempt")
// Number of successful address range mapping with a requested upper limit.
KCOUNTER(vm_region_map_specific_success, "vm.region.map.specific.success")
// Number of attempted mapping at a specified address.
KCOUNTER(vm_region_map_upper_bound_attempt, "vm.region.map.upper_bound.attempt")
// Number of successful mapping at a specified address.
KCOUNTER(vm_region_map_upper_bound_success, "vm.region.map.upper_bound.success")

VmAddressRegion::VmAddressRegion(VmAspace& aspace, vaddr_t base, size_t size, uint32_t vmar_flags)
    : VmAddressRegionOrMapping(base, size, vmar_flags | VMAR_CAN_RWX_FLAGS, &aspace, nullptr,
                               false) {
  // We add in CAN_RWX_FLAGS above, since an address space can't usefully
  // contain a process without all of these.

  strlcpy(const_cast<char*>(name_), "root", sizeof(name_));
  LTRACEF("%p '%s'\n", this, name_);
}

VmAddressRegion::VmAddressRegion(VmAddressRegion& parent, vaddr_t base, size_t size,
                                 uint32_t vmar_flags, const char* name)
    : VmAddressRegionOrMapping(base, size, vmar_flags, parent.aspace_.get(), &parent, false) {
  strlcpy(const_cast<char*>(name_), name, sizeof(name_));
  LTRACEF("%p '%s'\n", this, name_);
}

VmAddressRegion::VmAddressRegion(VmAspace& kernel_aspace)
    : VmAddressRegion(kernel_aspace, kernel_aspace.base(), kernel_aspace.size(),
                      VMAR_FLAG_CAN_MAP_SPECIFIC) {
  // Activate the kernel root aspace immediately
  state_ = LifeCycleState::ALIVE;
}

zx_status_t VmAddressRegion::CreateRootLocked(VmAspace& aspace, uint32_t vmar_flags,
                                              fbl::RefPtr<VmAddressRegion>* out) {
  DEBUG_ASSERT(out);

  fbl::AllocChecker ac;
  auto vmar = new (&ac) VmAddressRegion(aspace, aspace.base(), aspace.size(), vmar_flags);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  AssertHeld(vmar->lock_ref());
  AssertHeld(vmar->region_lock_ref());

  vmar->state_ = LifeCycleState::ALIVE;
  *out = fbl::AdoptRef(vmar);
  return ZX_OK;
}

zx_status_t VmAddressRegion::CreateSubVmarInternal(size_t offset, size_t size, uint8_t align_pow2,
                                                   uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo,
                                                   uint64_t vmo_offset,
                                                   arch_mmu_flags_t arch_mmu_flags,
                                                   const char* name, vaddr_t* base_out,
                                                   fbl::RefPtr<VmAddressRegionOrMapping>* out) {
  zx_status_t status = CreateSubVmarInner(offset, size, align_pow2, vmar_flags, vmo, vmo_offset,
                                          arch_mmu_flags, name, base_out, out);

  const bool is_specific_overwrite = vmar_flags & VMAR_FLAG_SPECIFIC_OVERWRITE;
  const bool is_specific = (vmar_flags & VMAR_FLAG_SPECIFIC) || is_specific_overwrite;
  const bool is_upper_bound = vmar_flags & VMAR_FLAG_OFFSET_IS_UPPER_LIMIT;

  kcounter_add(vm_region_map_all_attempt, 1);
  if (is_specific) {
    kcounter_add(vm_region_map_specific_attempt, 1);
  } else if (is_upper_bound) {
    kcounter_add(vm_region_map_upper_bound_attempt, 1);
  }

  if (status == ZX_OK) {
    kcounter_add(vm_region_map_all_success, 1);
    if (is_specific) {
      kcounter_add(vm_region_map_specific_success, 1);
    } else if (is_upper_bound) {
      kcounter_add(vm_region_map_upper_bound_success, 1);
    }
  }
  return status;
}

zx_status_t VmAddressRegion::CreateSubVmarInner(size_t offset, size_t size, uint8_t align_pow2,
                                                uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo,
                                                uint64_t vmo_offset,
                                                arch_mmu_flags_t arch_mmu_flags, const char* name,
                                                vaddr_t* base_out,
                                                fbl::RefPtr<VmAddressRegionOrMapping>* out) {
  DEBUG_ASSERT(out);
  MemoryPriority memory_priority;
  fbl::RefPtr<VmAddressRegionOrMapping> vmar;

  {
    Guard<CriticalMutex> region_guard{region_lock()};
    if (state_locked_region() != LifeCycleState::ALIVE) {
      return ZX_ERR_BAD_STATE;
    }

    if (size == 0) {
      return ZX_ERR_INVALID_ARGS;
    }

    // Check if there are any RWX privileges that the child would have that the
    // parent does not.
    if (vmar_flags & ~flags_ & VMAR_CAN_RWX_FLAGS) {
      return ZX_ERR_ACCESS_DENIED;
    }

    // Extract the creation only flags out of vmar_flags, leaving the remaining to be stored
    // permanently in the object.
    const bool is_specific_overwrite = vmar_flags & VMAR_FLAG_SPECIFIC_OVERWRITE;
    const bool is_specific = (vmar_flags & VMAR_FLAG_SPECIFIC) || is_specific_overwrite;
    const bool is_upper_bound = vmar_flags & VMAR_FLAG_OFFSET_IS_UPPER_LIMIT;
    vmar_flags &=
        ~(VMAR_FLAG_SPECIFIC_OVERWRITE | VMAR_FLAG_SPECIFIC | VMAR_FLAG_OFFSET_IS_UPPER_LIMIT);
    if (is_specific && is_upper_bound) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (!is_specific && !is_upper_bound && offset != 0) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (!IsPageRounded(offset)) {
      return ZX_ERR_INVALID_ARGS;
    }

    // Check that we have the required privileges if we want a SPECIFIC or
    // UPPER_LIMIT mapping.
    if ((is_specific || is_upper_bound) && !(flags_ & VMAR_FLAG_CAN_MAP_SPECIFIC)) {
      return ZX_ERR_ACCESS_DENIED;
    }

    if (!is_upper_bound && (offset >= size_ || size > size_ - offset)) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (is_upper_bound && (offset > size_ || size > size_ || size > offset)) {
      return ZX_ERR_INVALID_ARGS;
    }

    vaddr_t new_base = ktl::numeric_limits<vaddr_t>::max();
    if (is_specific) {
      // This would not overflow because offset <= size_ - 1, base_ + offset <= base_ + size_ - 1.
      new_base = base_ + offset;
      if (align_pow2 > 0 && (new_base & ((1ULL << align_pow2) - 1))) {
        return ZX_ERR_INVALID_ARGS;
      }
      if (!subregions_locked_region().IsRangeAvailable(new_base, size)) {
        if (is_specific_overwrite) {
          *base_out = new_base;
          Guard<CriticalMutex> guard{lock()};
          return OverwriteVmMappingLocked(new_base, size, vmar_flags, vmo, vmo_offset,
                                          arch_mmu_flags, out);
        }
        return ZX_ERR_ALREADY_EXISTS;
      }
    } else {
      // If we're not mapping to a specific place, search for an opening.
      const vaddr_t upper_bound =
          is_upper_bound ? base_ + offset : ktl::numeric_limits<vaddr_t>::max();
      zx_status_t status =
          AllocSpotLockedRegion(size, align_pow2, arch_mmu_flags, &new_base, upper_bound);
      if (status != ZX_OK) {
        return status;
      }
    }

    // Notice if this is an executable mapping from the vDSO VMO
    // before we lose the VMO reference via ktl::move(vmo).
    const bool is_vdso_code =
        (vmo && (arch_mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) && VDso::vmo_is_vdso(vmo));

    fbl::AllocChecker ac;
    if (vmo) {
      // Check that VMOs that back kernel mappings start of with their pages pinned, unless the
      // dynamic flag has been set to opt out of this specific check.
      DEBUG_ASSERT(aspace_->is_user() || aspace_->is_guest_physical() ||
                   vmar_flags & VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING ||
                   vmo->DebugIsRangePinned(vmo_offset, size));
      vmar = fbl::AdoptRef(new (&ac) VmMapping(*this, false, new_base, size, vmar_flags,
                                               ktl::move(vmo), is_upper_bound ? 0 : vmo_offset,
                                               arch_mmu_flags, VmMapping::Mergeable::NO));
    } else {
      vmar = fbl::AdoptRef(new (&ac) VmAddressRegion(*this, new_base, size, vmar_flags, name));
    }

    Guard<CriticalMutex> guard{lock()};
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }

    if (is_vdso_code) {
      // For an executable mapping of the vDSO, allow only one per process
      // and only for the valid range of the image.
      if (aspace_->vdso_code_mapping_ || !VDso::valid_code_mapping(vmo_offset, size)) {
        return ZX_ERR_ACCESS_DENIED;
      }
      aspace_->vdso_code_mapping_ = fbl::RefPtr<VmMapping>::Downcast(vmar);
    }

    // These locked actions on the vmar are done inside a lambda as otherwise the AssertHeld, which
    // does not end at the block scope, will continue and cause an error in calling
    // CommitHighMemoryPriority, which requires that the lock not be held.
    zx_status_t status = [this, &vmar]() TA_REQ(lock()) {
      AssertHeld(vmar->lock_ref());
      AssertHeld(vmar->region_lock_ref());
      zx_status_t status = vmar->Activate();
      if (status != ZX_OK) {
        return status;
      }
      // Propagate any memory priority settings. This should only fail if not alive, but we hold the
      // lock and just made it alive, so that cannot happen.
      if (VmMapping* downcast_mapping = vmar->as_vm_mapping_ptr(); downcast_mapping) {
        DEBUG_ASSERT(downcast_mapping);
        AssertHeld(downcast_mapping->lock_ref());
        downcast_mapping->SetMemoryPriorityLocked(memory_priority_);
      } else {
        VmAddressRegion* downcast_vmar = vmar->as_vm_address_region_ptr();
        DEBUG_ASSERT(downcast_vmar);
        AssertHeld(downcast_vmar->lock_ref());
        status = downcast_vmar->SetMemoryPriorityLocked(memory_priority_);
        DEBUG_ASSERT_MSG(status == ZX_OK, "status: %d", status);
      }
      return ZX_OK;
    }();
    if (status != ZX_OK) {
      return status;
    }

    memory_priority = memory_priority_;
    *base_out = new_base;
  }

  if (memory_priority == MemoryPriority::HIGH) {
    vmar->CommitHighMemoryPriority();
  }
  *out = ktl::move(vmar);
  return ZX_OK;
}

zx_status_t VmAddressRegion::CreateSubVmar(size_t offset, size_t size, uint8_t align_pow2,
                                           uint32_t vmar_flags, const char* name,
                                           fbl::RefPtr<VmAddressRegion>* out) {
  DEBUG_ASSERT(out);

  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Check that only allowed flags have been set
  if (vmar_flags & ~(VMAR_FLAG_SPECIFIC | VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_COMPACT |
                     VMAR_CAN_RWX_FLAGS | VMAR_FLAG_OFFSET_IS_UPPER_LIMIT)) {
    return ZX_ERR_INVALID_ARGS;
  }

  fbl::RefPtr<VmAddressRegionOrMapping> res;
  vaddr_t base;
  zx_status_t status = CreateSubVmarInternal(offset, size, align_pow2, vmar_flags, nullptr, 0,
                                             ARCH_MMU_FLAG_INVALID, name, &base, &res);
  if (status != ZX_OK) {
    return status;
  }
  *out = VmAddressRegionOrMapping::downcast_as_vm_address_region(&res);
  return ZX_OK;
}

zx::result<VmAddressRegion::MapResult> VmAddressRegion::CreateVmMapping(
    size_t mapping_offset, size_t size, uint8_t align_pow2, uint32_t vmar_flags,
    fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset, arch_mmu_flags_t arch_mmu_flags,
    const char* name) {
  LTRACEF("%p %#zx %#zx %x\n", this, mapping_offset, size, vmar_flags);

  // Check that only allowed flags have been set
  if (vmar_flags & ~(VMAR_FLAG_SPECIFIC | VMAR_FLAG_SPECIFIC_OVERWRITE | VMAR_CAN_RWX_FLAGS |
                     VMAR_FLAG_OFFSET_IS_UPPER_LIMIT | VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING |
                     VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE)) {
    return zx::error{ZX_ERR_INVALID_ARGS};
  }

  // Validate that arch_mmu_flags does not contain any prohibited flags
  if (!is_valid_mapping_flags(arch_mmu_flags)) {
    return zx::error{ZX_ERR_ACCESS_DENIED};
  }

  if (!IsPageRounded(vmo_offset)) {
    return zx::error{ZX_ERR_INVALID_ARGS};
  }

  size_t mapping_size = RoundUpPageSize(size);
  // Make sure that rounding up the page size did not overflow.
  if (mapping_size < size) {
    return zx::error{ZX_ERR_OUT_OF_RANGE};
  }
  // Make sure that a mapping of this size wouldn't overflow the vmo offset.
  if (vmo_offset + mapping_size < vmo_offset) {
    return zx::error{ZX_ERR_OUT_OF_RANGE};
  }

  // For physical VMOs, the mapping must be within the VMO.
  if (vmo && !vmo->is_paged() && vmo_offset + mapping_size > vmo->size()) {
    return zx::error{ZX_ERR_OUT_OF_RANGE};
  }

  // Can't create fault-beyond-stream-size mapping of physical or contiguous VMOs. There is
  // currently no use case for this as the stream size of these VMOs is always zero, so the mapping
  // would always fault. In this case, sys_vmar_map should have returned  ZX_ERR_NOT_SUPPORTED.
  DEBUG_ASSERT(!(vmar_flags & VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE) ||
               (vmo->is_paged() && !vmo->is_contiguous()));

  // If we're mapping it with a specific permission, we should allow
  // future Protect() calls on the mapping to keep that permission.
  if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_READ) {
    vmar_flags |= VMAR_FLAG_CAN_MAP_READ;
  }
  if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) {
    vmar_flags |= VMAR_FLAG_CAN_MAP_WRITE;
  }
  if (arch_mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE) {
    vmar_flags |= VMAR_FLAG_CAN_MAP_EXECUTE;
  }

  fbl::RefPtr<VmAddressRegionOrMapping> res;
  vaddr_t base;
  zx_status_t status = CreateSubVmarInternal(mapping_offset, mapping_size, align_pow2, vmar_flags,
                                             vmo, vmo_offset, arch_mmu_flags, name, &base, &res);
  if (status != ZX_OK) {
    return zx::error{status};
  }
  fbl::RefPtr<VmMapping> map = res->downcast_as_vm_mapping(&res);
  return zx::ok(MapResult{ktl::move(map), base});
}

zx_status_t VmAddressRegion::OverwriteVmMappingLocked(
    vaddr_t base, size_t size, uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset,
    arch_mmu_flags_t arch_mmu_flags, fbl::RefPtr<VmAddressRegionOrMapping>* out) {
  canary_.Assert();
  DEBUG_ASSERT(vmo);

  fbl::AllocChecker ac;
  fbl::RefPtr<VmMapping> vmar;
  vmar = fbl::AdoptRef(new (&ac) VmMapping(*this, false, base, size, vmar_flags, ktl::move(vmo),
                                           vmo_offset, arch_mmu_flags, VmMapping::Mergeable::NO));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  zx_status_t status = UnmapInternalLocked(base, size, false /* can_destroy_regions */);
  if (status != ZX_OK) {
    return status;
  }

  AssertHeld(vmar->lock_ref());
  AssertHeld(vmar->region_lock_ref());
  status = vmar->Activate();
  if (status != ZX_OK) {
    // Activation can fail if an allocation needed to happen. Nothing can be done to rollback here
    // so the only option is to propagate the ZX_ERR_NO_MEMORY up. If this is happening in response
    // to a user request then they will most likely panic, as users have no ability to resolve
    // ZX_ERR_NO_MEMORY.
    ASSERT(status == ZX_ERR_NO_MEMORY);
    return status;
  }

  // Propagate any memory priority settings. This should only fail if not alive, but we hold the
  // lock and just made it alive, so that cannot happen.
  vmar->SetMemoryPriorityLocked(memory_priority_);

  *out = ktl::move(vmar);
  return ZX_OK;
}

zx_status_t VmAddressRegion::DestroyLocked() {
  canary_.Assert();
  LTRACEF("%p '%s'\n", this, name_);

  // Remove any applied memory priority.
  zx_status_t status = SetMemoryPriorityLocked(MemoryPriority::DEFAULT);
  DEBUG_ASSERT(status == ZX_OK);

  // The cur reference prevents regions from being destructed after dropping
  // the last reference to them when removing from their parent.
  fbl::RefPtr<VmAddressRegion> cur(this);
  AssertHeld(cur->lock_ref());
  AssertHeld(cur->region_lock_ref());
  while (cur) {
    // Iterate through children destroying mappings. If we find a
    // subregion, stop so we can traverse down.
    fbl::RefPtr<VmAddressRegion> child_region = nullptr;
    while (!cur->subregions_.IsEmpty() && !child_region) {
      VmAddressRegionOrMapping* child = &cur->subregions_.front();
      if (child->is_mapping()) {
        AssertHeld(child->lock_ref());
        AssertHeld(child->region_lock_ref());
        // DestroyLocked should remove this child from our list on success.
        status = child->DestroyLocked();
        if (status != ZX_OK) {
          // TODO(teisenbe): Do we want to handle this case differently?
          return status;
        }
      } else {
        child_region = child->as_vm_address_region();
      }
    }

    if (child_region) {
      // If we found a child region, traverse down the tree.
      cur = child_region;
    } else {
      // All children are destroyed, so now destroy the current node.
      if (cur->parent_) {
        AssertHeld(cur->parent_->lock_ref());
        AssertHeld(cur->parent_->region_lock_ref());
        cur->parent_->subregions_.RemoveRegion(cur.get());
      }
      cur->state_ = LifeCycleState::DEAD;
      VmAddressRegion* cur_parent = cur->parent_;
      cur->parent_ = nullptr;

      // If we destroyed the original node, stop. Otherwise traverse
      // up the tree and keep destroying.
      cur.reset((cur.get() == this) ? nullptr : cur_parent);
    }
  }
  return ZX_OK;
}

fbl::RefPtr<VmAddressRegionOrMapping> VmAddressRegion::FindRegion(vaddr_t addr) {
  Guard<CriticalMutex> guard{lock()};
  return FindRegionLocked(addr);
}

fbl::RefPtr<VmAddressRegionOrMapping> VmAddressRegion::FindRegionLocked(vaddr_t addr) {
  if (state_locked() != LifeCycleState::ALIVE) {
    return nullptr;
  }
  // The const_cast here is safe as this method itself is non-const, however subregions_locked()
  // returns a const references to the subregions. The subregions are const as with only holding
  // lock_ we are not permitted to modify the subregions_ tree, but we are allowed to manipulate the
  // objects in the tree, hence can remove const.
  return fbl::RefPtr(const_cast<VmAddressRegionOrMapping*>(subregions_locked().FindRegion(addr)));
}

VmObject::AttributionCounts VmAddressRegion::GetAttributedMemory() const {
  canary_.Assert();

  Guard<CriticalMutex> guard{aspace_->lock()};
  if (!IsAliveLocked()) {
    return AttributionCounts{};
  }

  AttributionCounts page_counts;

  // Enumerate all of the subregions below us & count allocated pages.
  VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::MappingsOnly, const VmAddressRegion>
      enumerator(*this, 0, UINT64_MAX);
  AssertHeld(enumerator.lock_ref());
  while (auto next = enumerator.next()) {
    if (const VmMapping* map = next->region_or_mapping->as_vm_mapping_ptr(); map) {
      AssertHeld(map->lock_ref());
      enumerator.pause();
      page_counts += map->GetAttributedMemoryLocked(guard);
      enumerator.resume();
      if (!IsAliveLocked()) {
        break;
      }
    }
  }

  return page_counts;
}

VmMapping* VmAddressRegion::FindMappingLocked(vaddr_t va) {
  canary_.Assert();

  const VmAddressRegion* vmar = this;
  AssertHeld(vmar->lock_ref());
  while (const VmAddressRegionOrMapping* next = vmar->subregions_locked().FindRegion(va)) {
    if (auto mapping = next->as_vm_mapping_ptr()) {
      // The const_cast here is safe as this method itself is non-const, however subregions_locked()
      // returns a const references to the subregions. The subregions are const as with only holding
      // lock_ we are not permitted to modify the subregions_ tree, but we are allowed to manipulate
      // the objects in the tree, hence can remove const.
      return const_cast<VmMapping*>(mapping);
    }
    vmar = next->as_vm_address_region_ptr();
  }

  return nullptr;
}

ktl::optional<vaddr_t> VmAddressRegion::CheckGapLockedRegion(const VmAddressRegionOrMapping* prev,
                                                             const VmAddressRegionOrMapping* next,
                                                             vaddr_t search_base, vaddr_t align,
                                                             size_t region_size, size_t min_gap,
                                                             arch_mmu_flags_t arch_mmu_flags) {
  vaddr_t gap_beg;  // first byte of a gap
  vaddr_t gap_end;  // last byte of a gap

  // compute the starting address of the gap
  if (prev != nullptr) {
    if (add_overflow(prev->base(), prev->size(), &gap_beg) ||
        add_overflow(gap_beg, min_gap, &gap_beg)) {
      return ktl::nullopt;
    }
  } else {
    gap_beg = base_;
  }

  // compute the ending address of the gap
  if (next != nullptr) {
    if (gap_beg == next->base()) {
      return ktl::nullopt;  // no gap between regions
    }
    if (sub_overflow(next->base(), 1, &gap_end) || sub_overflow(gap_end, min_gap, &gap_end)) {
      return ktl::nullopt;
    }
  } else {
    if (gap_beg - base_ == size_) {
      return ktl::nullopt;  // no gap at the end of address space.
    }
    if (add_overflow(base_, size_ - 1, &gap_end)) {
      return ktl::nullopt;
    }
  }

  DEBUG_ASSERT(gap_end > gap_beg);

  // trim it to the search range
  if (gap_end <= search_base) {
    return ktl::nullopt;
  }
  if (gap_beg < search_base) {
    gap_beg = search_base;
  }

  DEBUG_ASSERT(gap_end > gap_beg);

  LTRACEF_LEVEL(2, "search base %#" PRIxPTR " gap_beg %#" PRIxPTR " end %#" PRIxPTR "\n",
                search_base, gap_beg, gap_end);

  vaddr_t va =
      aspace_->arch_aspace().PickSpot(gap_beg, gap_end, align, region_size, arch_mmu_flags);

  if (va < gap_beg) {
    return ktl::nullopt;  // address wrapped around
  }

  if (va >= gap_end || ((gap_end - va + 1) < region_size)) {
    return ktl::nullopt;  // not enough room
  }

  return va;
}

zx_status_t VmAddressRegion::EnumerateChildren(VmEnumerator* ve) {
  canary_.Assert();
  DEBUG_ASSERT(ve != nullptr);
  Guard<CriticalMutex> guard{lock()};
  if (state_locked() != LifeCycleState::ALIVE) {
    return ZX_ERR_BAD_STATE;
  }
  zx_status_t status = ve->OnVmAddressRegion(this, 0, guard);
  if (status != ZX_ERR_NEXT) {
    if (status == ZX_ERR_STOP) {
      return ZX_OK;
    }
    return status;
  }
  VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::VmarsAndMappings, VmAddressRegion>
      enumerator(*this, 0, UINT64_MAX);
  AssertHeld(enumerator.lock_ref());
  while (auto result = enumerator.next()) {
    enumerator.pause();
    if (VmMapping* mapping = result->region_or_mapping->as_vm_mapping_ptr(); mapping) {
      DEBUG_ASSERT(mapping != nullptr);
      AssertHeld(mapping->lock_ref());
      status = ve->OnVmMapping(mapping, this, result->depth, guard);
    } else {
      VmAddressRegion* vmar = result->region_or_mapping->as_vm_address_region_ptr();
      DEBUG_ASSERT(vmar != nullptr);
      AssertHeld(vmar->lock_ref());
      status = ve->OnVmAddressRegion(vmar, result->depth, guard);
    }
    if (status != ZX_ERR_NEXT) {
      if (status == ZX_ERR_STOP) {
        return ZX_OK;
      }
      return status;
    }
    enumerator.resume();
  }
  return ZX_OK;
}

bool VmAddressRegion::has_parent() const {
  Guard<CriticalMutex> guard{lock()};
  return parent_ != nullptr;
}

void VmAddressRegion::DumpLocked(uint depth, bool verbose) const {
  canary_.Assert();
  for (uint i = 0; i < depth; ++i) {
    printf("  ");
  }
  printf("vmar %p [%#" PRIxPTR " %#" PRIxPTR "] sz %#zx ref %d state %d '%s' subregions %zu\n",
         this, base_, base_ + (size_ - 1), size_, ref_count_debug(), (int)state_, name_,
         subregions_.size_slow());
  for (const auto& child : subregions_) {
    AssertHeld(child.second->lock_ref());
    child.second->DumpLocked(depth + 1, verbose);
  }
}

zx_status_t VmAddressRegion::Activate() {
  DEBUG_ASSERT(state_ == LifeCycleState::NOT_READY);

  AssertHeld(parent_->lock_ref());
  AssertHeld(parent_->region_lock_ref());

  // Validate we are a correct child of our parent.
  DEBUG_ASSERT(parent_->is_in_range(base_, size_));

  // Look for a region in the parent starting from our desired base. If any region is found, make
  // sure we do not intersect with it.
  auto candidate = parent_->subregions_.IncludeOrHigher(base_);
  if (candidate != parent_->subregions_.end()) {
    AssertHeld((*candidate).second->lock_ref());
    ASSERT((*candidate).second->base() >= base_ + size_);
  }

  zx_status_t status =
      parent_->subregions_.InsertRegion(fbl::RefPtr<VmAddressRegionOrMapping>(this));
  if (status != ZX_OK) {
    return status;
  }
  state_ = LifeCycleState::ALIVE;
  return ZX_OK;
}

zx_status_t VmAddressRegion::RangeOp(RangeOpType op, vaddr_t base, size_t len,
                                     VmAddressRegionOpChildren op_children,
                                     user_inout_ptr<void> buffer, size_t buffer_size) {
  canary_.Assert();
  if (buffer || buffer_size) {
    return ZX_ERR_INVALID_ARGS;
  }
  len = RoundUpPageSize(len);
  if (len == 0 || !IsPageRounded(base)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (!is_in_range(base, len)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  const vaddr_t last_addr = base + len;

  Guard<CriticalMutex> guard{lock()};
  // Capture the validation that we need to do whenever the lock is acquired.
  auto validate = [this, base, len]() TA_REQ(lock()) -> zx_status_t {
    if (state_locked() != LifeCycleState::ALIVE) {
      return ZX_ERR_BAD_STATE;
    }

    // Don't allow any operations on the vDSO code mapping.
    if (aspace_->IntersectsVdsoCodeLocked(base, len)) {
      return ZX_ERR_ACCESS_DENIED;
    }
    return ZX_OK;
  };
  if (zx_status_t s = validate(); s != ZX_OK) {
    return s;
  }

  VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::MappingsOnly, VmAddressRegion>
      enumerator(*this, base, last_addr);
  AssertHeld(enumerator.lock_ref());
  vaddr_t expected = base;
  while (auto map = enumerator.next()) {
    // Presently we hold the lock, so we know that region_or_mapping is valid, but we want to use
    // this outside of the lock later on, and so we must upgrade it to a RefPtr.
    fbl::RefPtr<VmMapping> mapping = map->region_or_mapping->as_vm_mapping();
    DEBUG_ASSERT(mapping);
    AssertHeld(mapping->lock_ref());

    // It's possible base is less than expected if the first mapping is not precisely aligned
    // to the start of our range. After that base should always be expected, and if it's
    // greater then there is a gap and this is considered an error.
    if (mapping->base() > expected) {
      return ZX_ERR_BAD_STATE;
    }
    // We should only have been called if we were at least partially in range.
    DEBUG_ASSERT(mapping->is_in_range(expected, 1));
    const size_t mapping_offset = expected - mapping->base();
    const size_t vmo_offset = mapping->object_offset() + mapping_offset;

    // Should only have been called for a non-zero range.
    DEBUG_ASSERT(last_addr > expected);

    const size_t total_remain = last_addr - expected;
    DEBUG_ASSERT(mapping->size() > mapping_offset);
    const size_t max_in_mapping = mapping->size() - mapping_offset;

    const size_t size = ktl::min(total_remain, max_in_mapping);

    fbl::RefPtr<VmObject> vmo = mapping->vmo_locked();

    zx_status_t result = ZX_OK;
    enumerator.pause();
    // The commit, decommit and prefetch ops check the maximal permissions of the mapping and can be
    // thought of as acting as if they perform a protect to add read or write permissions. Since
    // protect to add permissions through a parent VMAR is not valid we similarly forbid this
    // notional protect by not allowing these operations if acting through a sub-vmar, regardless of
    // whether op_children is otherwise allowed.
    if ((op == RangeOpType::Commit || op == RangeOpType::Decommit || op == RangeOpType::Prefetch ||
         op_children == VmAddressRegionOpChildren::No) &&
        mapping->parent_ != this) {
      return ZX_ERR_INVALID_ARGS;
    }

    guard.CallUnlocked([&result, &vmo, &mapping, op, mapping_offset, vmo_offset, size] {
      // For fault-beyond-stream-size mappings, ensure there are no gaps due to the stream size
      // being less than the end of the mapping. User synchronisation is required for the observable
      // result to be defined, as the stream size is a user managed property & not guaranteed atomic
      // to the VMO.
      if (mapping->flags_ & VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE) {
        VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(vmo.get());
        DEBUG_ASSERT(paged);
        {
          Guard<CriticalMutex> vmo_guard{paged->lock()};

          auto stream_size = paged->saturating_stream_size_locked();
          DEBUG_ASSERT(stream_size);
          if (vmo_offset > *stream_size) {
            result = ZX_ERR_OUT_OF_RANGE;
            return;
          }
          if (size > *stream_size - vmo_offset) {
            result = ZX_ERR_OUT_OF_RANGE;
            return;
          }
        }
      }
      switch (op) {
        case RangeOpType::Commit:
          if (!mapping->is_valid_mapping_flags(ARCH_MMU_FLAG_PERM_WRITE)) {
            result = ZX_ERR_ACCESS_DENIED;
          } else {
            result = vmo->CommitRange(vmo_offset, size);
            if (result == ZX_OK) {
              result = mapping->MapRange(mapping_offset, size, /*commit=*/false,
                                         /*ignore_existing=*/true);
            }
          }
          break;
        case RangeOpType::Decommit:
          // Decommit zeroes pages of the VMO, equivalent to writing to it.
          // the mapping is currently writable, or could be made writable.
          if (!mapping->is_valid_mapping_flags(ARCH_MMU_FLAG_PERM_WRITE)) {
            result = ZX_ERR_ACCESS_DENIED;
          } else {
            result = vmo->DecommitRange(vmo_offset, size);
          }
          break;
        case RangeOpType::MapRange:
          result = mapping->MapRange(mapping_offset, size, /*commit=*/false,
                                     /*ignore_existing=*/true);
          break;
        case RangeOpType::Zero:
          if (!mapping->is_valid_mapping_flags(ARCH_MMU_FLAG_PERM_WRITE)) {
            result = ZX_ERR_ACCESS_DENIED;
          } else {
            result = vmo->ZeroRange(vmo_offset, size);
          }
          break;
        case RangeOpType::AlwaysNeed:
          result = vmo->HintRange(vmo_offset, size, VmObject::EvictionHint::AlwaysNeed);
          if (result == ZX_OK) {
            result = mapping->MapRange(mapping_offset, size, /*commit=*/false,
                                       /*ignore_existing=*/true);
          }
          break;
        case RangeOpType::DontNeed:
          result = vmo->HintRange(vmo_offset, size, VmObject::EvictionHint::DontNeed);
          break;
        case RangeOpType::Prefetch:
          if (!mapping->is_valid_mapping_flags(ARCH_MMU_FLAG_PERM_READ)) {
            result = ZX_ERR_ACCESS_DENIED;
          } else {
            result = vmo->PrefetchRange(vmo_offset, size);
            if (result == ZX_OK) {
              result = mapping->MapRange(mapping_offset, size, /*commit=*/false,
                                         /*ignore_existing=*/true);
            }
          }
          break;
        default:
          result = ZX_ERR_NOT_SUPPORTED;
          break;
      }
    });
    // Since the lock was dropped we must re-validate before doing anything else.
    if (zx_status_t s = validate(); s != ZX_OK) {
      return s;
    }
    enumerator.resume();

    if (result != ZX_OK) {
      return result;
    }
    expected += size;
  }

  // Check if there was a gap right at the end of the range.
  if (expected < last_addr) {
    return ZX_ERR_BAD_STATE;
  }
  return ZX_OK;
}

zx_status_t VmAddressRegion::Unmap(vaddr_t base, size_t size,
                                   VmAddressRegionOpChildren op_children) {
  canary_.Assert();

  size = RoundUpPageSize(size);
  if (size == 0 || !IsPageRounded(base)) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<CriticalMutex> region_guard{region_lock()};
  Guard<CriticalMutex> guard{lock()};
  if (state_ != LifeCycleState::ALIVE) {
    return ZX_ERR_BAD_STATE;
  }

  return UnmapInternalLocked(
      base, size, op_children == VmAddressRegionOpChildren::Yes /* can_destroy_regions */);
}

zx_status_t VmAddressRegion::UnmapInternalLocked(vaddr_t base, size_t size,
                                                 bool can_destroy_regions) {
  if (!is_in_range(base, size)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (subregions_.IsEmpty()) {
    return ZX_OK;
  }

  // Any unmap spanning the vDSO code mapping is verboten.
  if (aspace_->IntersectsVdsoCodeLocked(base, size)) {
    return ZX_ERR_ACCESS_DENIED;
  }

  vaddr_t end_addr_byte = 0;
  DEBUG_ASSERT(size > 0);
  bool overflowed = add_overflow(base, size - 1, &end_addr_byte);
  ASSERT(!overflowed);

  // Check if we're partially spanning a subregion, or aren't allowed to
  // destroy regions and are spanning a region, and bail if we are.
  for (auto itr = subregions_.IncludeOrHigher(base);
       itr.IsValid() && (*itr).second->base() <= end_addr_byte; ++itr) {
    vaddr_t itr_end_byte = 0;
    AssertHeld((*itr).second->lock_ref());
    DEBUG_ASSERT((*itr).second->size() > 0);
    overflowed = add_overflow((*itr).second->base(), (*itr).second->size() - 1, &itr_end_byte);
    ASSERT(!overflowed);
    if (!(*itr).second->is_mapping() &&
        (!can_destroy_regions || (*itr).second->base() < base || itr_end_byte > end_addr_byte)) {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  while (true) {
    // TODO(https://fxbug.dev/503042881): Every deletion invalidates our iterator, hence we have to
    // lookup a new one every time.
    auto itr = subregions_.IncludeOrHigher(base);
    if (!itr.IsValid() || (*itr).second->base() > end_addr_byte) {
      break;
    }

    auto curr_region = (*itr).second;
    AssertHeld(curr_region->lock_ref());
    AssertHeld(curr_region->region_lock_ref());

    vaddr_t unmap_base = 0;
    size_t unmap_size = 0;
    [[maybe_unused]] bool intersects = GetIntersect(base, size, curr_region->base(),
                                                    curr_region->size(), &unmap_base, &unmap_size);
    DEBUG_ASSERT(intersects);
    if (unmap_base == curr_region->base() && unmap_size == curr_region->size()) {
      zx_status_t status = curr_region->DestroyLocked();
      if (status != ZX_OK) {
        DEBUG_ASSERT(status == ZX_ERR_NO_MEMORY);
        return status;
      }
    } else {
      VmMapping* mapping = curr_region->as_vm_mapping_ptr();
      // Already validated in the loop above that the only kind of region we can partially span is
      // a mapping.
      ASSERT(mapping);
      AssertHeld(mapping->lock_ref());
      AssertHeld(mapping->region_lock_ref());
      // Unmapping can fail if an allocation needed to happen. Nothing can be done to rollback
      // here so the only option is to propagate the ZX_ERR_NO_MEMORY up. If this is happening
      // in response to a user request then they will most likely panic, as users have no
      // ability to resolve ZX_ERR_NO_MEMORY.
      zx_status_t status = mapping->UnmapLocked(unmap_base, unmap_size);
      if (status != ZX_OK) {
        ASSERT(status == ZX_ERR_NO_MEMORY);
        return status;
      }
    }
  }

  return ZX_OK;
}

zx_status_t VmAddressRegion::Protect(vaddr_t base, size_t size, arch_mmu_flags_t new_arch_mmu_flags,
                                     VmAddressRegionOpChildren op_children) {
  canary_.Assert();

  size = RoundUpPageSize(size);
  if (size == 0 || !IsPageRounded(base)) {
    return ZX_ERR_INVALID_ARGS;
  }

  Guard<CriticalMutex> guard{lock()};
  if (state_locked() != LifeCycleState::ALIVE) {
    return ZX_ERR_BAD_STATE;
  }

  if (!is_in_range(base, size)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Do not allow changing caching.
  if (new_arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK) {
    return ZX_ERR_INVALID_ARGS;
  }

  // The last byte of the range.
  vaddr_t end_addr_byte = 0;
  bool overflowed = add_overflow(base, size - 1, &end_addr_byte);
  ASSERT(!overflowed);

  // Check part of the range is not mapped, or the new permissions are invalid for some mapping in
  // the range.
  {
    VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::MappingsOnly, VmAddressRegion>
        enumerator(*this, base, end_addr_byte);
    AssertHeld(enumerator.lock_ref());
    vaddr_t expected = base;
    while (auto entry = enumerator.next()) {
      VmMapping* mapping = entry->region_or_mapping->as_vm_mapping_ptr();
      DEBUG_ASSERT(mapping);
      AssertHeld(mapping->lock_ref());
      if (mapping->base() > expected) {
        return ZX_ERR_NOT_FOUND;
      }
      vaddr_t end;
      overflowed = add_overflow(mapping->base(), mapping->size(), &end);
      ASSERT(!overflowed);
      if (!mapping->is_valid_mapping_flags(new_arch_mmu_flags)) {
        return ZX_ERR_ACCESS_DENIED;
      }
      if (mapping == aspace_->vdso_code_mapping_.get()) {
        return ZX_ERR_ACCESS_DENIED;
      }
      AssertHeld(mapping->lock_ref());
      if (mapping->parent_ != this) {
        if (op_children == VmAddressRegionOpChildren::No) {
          return ZX_ERR_INVALID_ARGS;
        }
        // As this is a sub-region we cannot increase its mapping flags, even if they might
        // otherwise be permissible. A mapping might have multiple different protect regions so need
        // to check all of them within the protection range.
        // Already know that expected is within the mapping, calculate a length that is within the
        // range of mapping.
        const size_t len = ktl::min(end_addr_byte, end - 1) - expected + 1;
        zx_status_t status = mapping->EnumerateProtectionRangesLocked(
            expected, len, [&new_arch_mmu_flags](vaddr_t, size_t, arch_mmu_flags_t flags) {
              if ((flags & new_arch_mmu_flags) != new_arch_mmu_flags) {
                return ZX_ERR_ACCESS_DENIED;
              }
              return ZX_ERR_NEXT;
            });
        if (status != ZX_OK) {
          return status;
        }
      }
      expected = end;
    }
    if (expected < end_addr_byte) {
      return ZX_ERR_NOT_FOUND;
    }
  }

  VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::MappingsOnly, VmAddressRegion>
      enumerator(*this, base, end_addr_byte);
  AssertHeld(enumerator.lock_ref());
  while (auto entry = enumerator.next()) {
    VmMapping* mapping = entry->region_or_mapping->as_vm_mapping_ptr();
    DEBUG_ASSERT(mapping);

    // The last byte of the current region.
    vaddr_t curr_end_byte = 0;
    AssertHeld(mapping->lock_ref());
    overflowed = add_overflow(mapping->base(), mapping->size() - 1, &curr_end_byte);
    ASSERT(!overflowed);
    const vaddr_t protect_base = ktl::max(mapping->base(), base);
    const vaddr_t protect_end_byte = ktl::min(curr_end_byte, end_addr_byte);
    size_t protect_size;
    overflowed = add_overflow(protect_end_byte - protect_base, 1, &protect_size);
    ASSERT(!overflowed);
    AssertHeld(mapping->lock_ref());

    // ProtectLocked might delete the mapping, and so we must pause the enumerator to safely perform
    // mutations. Note that even though we are pausing the enumerator here, it is *NOT* okay to drop
    // the lock between the pause and resume. We need to mutate permissions on all the mappings in
    // the requested range atomically (except for failure due to ZX_ERR_NO_MEMORY) and so the lock
    // must be held throughout.
    enumerator.pause();
    zx_status_t status = mapping->ProtectLocked(protect_base, protect_size, new_arch_mmu_flags);
    if (status != ZX_OK) {
      // We can error out only due to failed allocations. Other error conditions should have already
      // been checked above.
      ASSERT(status == ZX_ERR_NO_MEMORY);
      // TODO(teisenbe): Try to work out a way to guarantee success, or
      // provide a full unwind?
      return status;
    }
    enumerator.resume();
  }

  return ZX_OK;
}

// Perform allocations for VMARs. This allocator works by choosing uniformly at random from a set of
// positions that could satisfy the allocation. The set of positions are the 'left' most positions
// of the address space and are capped by the address entropy limit. The entropy limit is retrieved
// from the address space, and can vary based on whether the user has requested compact allocations
// or not.
zx_status_t VmAddressRegion::AllocSpotLockedRegion(size_t size, uint8_t align_pow2,
                                                   arch_mmu_flags_t arch_mmu_flags, vaddr_t* spot,
                                                   vaddr_t upper_limit) {
  LTRACEF("size=%zu align_pow2=%u arch_mmu_flags=%x upper_limit=%zx\n", size, align_pow2,
          arch_mmu_flags, upper_limit);
  canary_.Assert();
  DEBUG_ASSERT(size > 0 && IsPageRounded(size));
  DEBUG_ASSERT(spot);

  LTRACEF_LEVEL(2, "aspace %p size 0x%zx align %hhu upper_limit 0x%lx\n", this, size, align_pow2,
                upper_limit);

  align_pow2 = ktl::max(align_pow2, static_cast<uint8_t>(kPageShift));
  const vaddr_t align = 1UL << align_pow2;
  // Ensure our candidate calculation shift will not overflow.
  const uint8_t entropy = aspace_->AslrEntropyBits(flags_ & VMAR_FLAG_COMPACT);
  vaddr_t alloc_spot = 0;
  crypto::Prng* prng = nullptr;
  if (aspace_->is_aslr_enabled()) {
    prng = &aspace_->AslrPrng();
  }

  zx_status_t status = subregions_locked_region().GetAllocSpot(
      &alloc_spot, align_pow2, entropy, size, base_, size_, prng, upper_limit);

  if (status != ZX_OK) {
    return status;
  }

  // Sanity check that the allocation fits.
  vaddr_t alloc_last_byte;
  bool overflowed = add_overflow(alloc_spot, size - 1, &alloc_last_byte);
  ASSERT(!overflowed);
  auto after_iter = subregions_locked_region().UpperBound(alloc_last_byte);
  auto before_iter = after_iter;

  if (after_iter == subregions_locked_region().begin() || subregions_locked_region().IsEmpty()) {
    before_iter = subregions_locked_region().end();
  } else {
    --before_iter;
  }

  ASSERT(before_iter == subregions_locked_region().end() || before_iter.IsValid());
  const VmAddressRegionOrMapping* before = nullptr;
  if (before_iter.IsValid()) {
    before = (*before_iter).second;
  }
  const VmAddressRegionOrMapping* after = nullptr;
  if (after_iter.IsValid()) {
    after = (*after_iter).second;
  }
  if (auto va = CheckGapLockedRegion(before, after, alloc_spot, align, size, 0, arch_mmu_flags)) {
    *spot = *va;
    return ZX_OK;
  }
  panic("Unexpected allocation failure\n");
}

using ArchUnmapOptions = ArchVmAspaceInterface::ArchUnmapOptions;

zx_status_t VmAddressRegion::ReserveSpace(const char* name, vaddr_t base, size_t size,
                                          arch_mmu_flags_t arch_mmu_flags) {
  canary_.Assert();
  if (!is_in_range(base, size)) {
    return ZX_ERR_INVALID_ARGS;
  }
  size_t offset = base - this->base();
  // We need a zero-length VMO to pass into CreateVmMapping so that a VmMapping would be created.
  // The VmMapping is already mapped to physical pages in start.S.
  // We would never call MapRange on the VmMapping, thus the VMO would never actually allocate any
  // physical pages and we would never modify the PTE except for the permission change bellow
  // caused by Protect.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, 0, &vmo);
  if (status != ZX_OK) {
    return status;
  }
  vmo->set_name(name, strlen(name));

  // Set the cache policy on the VMO to match arch_mmu_flags to squelch a warning elsewhere when
  // the mapping is created.
  switch (arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK) {
    case ARCH_MMU_FLAG_UNCACHED:
      vmo->SetMappingCachePolicy(ZX_CACHE_POLICY_UNCACHED);
      break;
    case ARCH_MMU_FLAG_UNCACHED_DEVICE:
      vmo->SetMappingCachePolicy(ZX_CACHE_POLICY_UNCACHED_DEVICE);
      break;
    case ARCH_MMU_FLAG_WRITE_COMBINING:
      vmo->SetMappingCachePolicy(ZX_CACHE_POLICY_WRITE_COMBINING);
      break;
    case ARCH_MMU_FLAG_CACHED:
      break;  // nop
    default:
      panic("unhandled arch_mmu_flags %#x\n", arch_mmu_flags);
  }

  // allocate a region and put it in the aspace list.
  // Need to set the VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING since we are 'cheating' with this fake
  // zero-length VMO and so the checks that the pages in that VMO are pinned would otherwise fail.
  zx::result<VmAddressRegion::MapResult> r =
      CreateVmMapping(offset, size, 0, VMAR_FLAG_SPECIFIC | VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING,
                      vmo, 0, arch_mmu_flags, name);
  if (r.is_error()) {
    return r.status_value();
  }
  // Directly invoke a protect on the hardware aspace to modify the protection of the existing
  // mappings. If the desired protection flags is "no permissions" then we need to use unmap instead
  // of protect since a mapping with no permissions is not valid on most architectures.
  if ((arch_mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK) == 0) {
    return aspace_->arch_aspace().Unmap(base, size / kPageSize, ArchUnmapOptions::None);
  } else {
    // This method should only be called during early system init prior to the bringup of other
    // CPUs. In this case it is safe to allow the Protect operations to temporarily enlarge.
    const cpu_mask_t online = mp_get_online_mask();
    const cpu_num_t curr = arch_curr_cpu_num();
    DEBUG_ASSERT_MSG((online & ~cpu_num_to_mask(curr)) == 0,
                     "Online mask %u has more than current cpu %u", online, curr);
    return aspace_->arch_aspace().Protect(base, size / kPageSize, arch_mmu_flags,
                                          ArchUnmapOptions::Enlarge);
  }
}

zx_status_t VmAddressRegion::SetMemoryPriority(MemoryPriority priority) {
  canary_.Assert();
  bool have_children = false;
  {
    Guard<CriticalMutex> guard{lock()};
    zx_status_t status = SetMemoryPriorityLocked(priority);
    if (status != ZX_OK) {
      return status;
    }
    have_children = !subregions_locked().IsEmpty();
  }
  // If a high memory priority was set, perform another pass through any mappings to commit it,
  // unless we know we didn't have any children at the point we set the priority to avoid a needless
  // lock acquisition and pass.
  if (priority == MemoryPriority::HIGH && have_children) {
    CommitHighMemoryPriority();
  }
  return ZX_OK;
}

zx_status_t VmAddressRegion::SetMemoryPriorityLocked(MemoryPriority priority) {
  if (state_locked() != LifeCycleState::ALIVE) {
    DEBUG_ASSERT(memory_priority_ == MemoryPriority::DEFAULT);
    return ZX_ERR_BAD_STATE;
  }

  auto set_region_priority = [priority](VmAddressRegion* region) {
    AssertHeld(region->lock_ref());
    if (priority == region->memory_priority_) {
      return;
    }
    region->memory_priority_ = priority;
    // As a region we only need to inform the VmAspace of the change.
    region->aspace_->ChangeHighPriorityCountLocked(priority == MemoryPriority::HIGH ? 1 : -1);
  };

  // Do our own priority change.
  set_region_priority(this);

  // Enumerate all of the subregions below us.
  VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::VmarsAndMappings, VmAddressRegion>
      enumerator(*this, 0, UINT64_MAX);
  AssertHeld(enumerator.lock_ref());
  while (auto next = enumerator.next()) {
    if (VmMapping* map = next->region_or_mapping->as_vm_mapping_ptr(); map) {
      AssertHeld(map->lock_ref());
      map->SetMemoryPriorityLocked(priority);
    } else {
      set_region_priority(next->region_or_mapping->as_vm_address_region_ptr());
    }
  }
  return ZX_OK;
}

void VmAddressRegion::CommitHighMemoryPriority() {
  canary_.Assert();

  Guard<CriticalMutex> guard{lock()};
  // Capture the validation that we need to do whenever the lock is acquired.
  auto validate = [this]() TA_REQ(lock()) -> bool {
    if (state_locked() != LifeCycleState::ALIVE) {
      return false;
    }

    if (memory_priority_ != MemoryPriority::HIGH) {
      return false;
    }

    return true;
  };
  if (!validate()) {
    return;
  }

  VmAddressRegionEnumerator<VmAddressRegionEnumeratorType::VmarsAndMappings, VmAddressRegion>
      enumerator(*this, 0, UINT64_MAX);
  AssertHeld(enumerator.lock_ref());
  while (auto map = enumerator.next()) {
    // Presently we hold the lock, so we know that region_or_mapping is valid, but we want to use
    // this outside of the lock later on, and so we must upgrade it to a RefPtr.
    fbl::RefPtr<VmMapping> mapping = map->region_or_mapping->as_vm_mapping();
    if (!mapping) {
      continue;
    }
    enumerator.pause();
    guard.CallUnlocked(
        [mapping = ktl::move(mapping)]() mutable { mapping->CommitHighMemoryPriority(); });
    // Since the lock was dropped we must re-validate before doing anything else.
    if (!validate()) {
      return;
    }
    enumerator.resume();
  }
}

extern "C" {
fbl::RefCounted<VmAddressRegionOrMapping>* cpp_vm_address_region_get_ref_counted(
    VmAddressRegion* vmar);
void cpp_vm_address_region_free(VmAddressRegion* vmar);
zx_status_t cpp_vm_address_region_destroy(VmAddressRegion* vmar);
vaddr_t cpp_vm_address_region_base(VmAddressRegion* vmar);
size_t cpp_vm_address_region_size(VmAddressRegion* vmar);
uint32_t cpp_vm_address_region_flags(VmAddressRegion* vmar);
const char* cpp_vm_address_region_name(VmAddressRegion* vmar);
bool cpp_vm_address_region_has_parent(VmAddressRegion* vmar);
zx_status_t cpp_vm_address_region_set_memory_priority(VmAddressRegion* vmar,
                                                      VmAddressRegion::MemoryPriority priority);
zx_status_t cpp_vm_address_region_unmap(VmAddressRegion* vmar, vaddr_t base, size_t size,
                                        VmAddressRegionOpChildren op_children);
zx_status_t cpp_vm_address_region_protect(VmAddressRegion* vmar, vaddr_t base, size_t size,
                                          arch_mmu_flags_t new_arch_mmu_flags,
                                          VmAddressRegionOpChildren op_children);
zx_status_t cpp_vm_address_region_reserve_space(VmAddressRegion* vmar, const char* name,
                                                size_t base, size_t size,
                                                arch_mmu_flags_t arch_mmu_flags);
VmAddressRegion* cpp_vm_address_region_create_sub_vmar(VmAddressRegion* vmar, size_t offset,
                                                       size_t size, uint8_t align_pow2,
                                                       uint32_t vmar_flags, const char* name,
                                                       zx_status_t* out_status);
VmMapping* cpp_vm_address_region_create_vm_mapping(
    VmAddressRegion* vmar, size_t mapping_offset, size_t size, uint8_t align_pow2,
    uint32_t vmar_flags, const VmObject* vmo, uint64_t vmo_offset, arch_mmu_flags_t arch_mmu_flags,
    const char* name, vaddr_t* out_base, zx_status_t* out_status);
const fbl::RefPtr<VmAspace>* cpp_vm_address_region_aspace(VmAddressRegion* vmar);

fbl::RefCounted<VmAddressRegionOrMapping>* cpp_vm_address_region_get_ref_counted(
    VmAddressRegion* vmar) {
  return vmar;
}
void cpp_vm_address_region_free(VmAddressRegion* vmar) { delete vmar; }
zx_status_t cpp_vm_address_region_destroy(VmAddressRegion* vmar) { return vmar->Destroy(); }
const fbl::RefPtr<VmAspace>* cpp_vm_address_region_aspace(VmAddressRegion* vmar) {
  return &vmar->aspace();
}
vaddr_t cpp_vm_address_region_base(VmAddressRegion* vmar) { return vmar->base(); }
size_t cpp_vm_address_region_size(VmAddressRegion* vmar) { return vmar->size(); }
uint32_t cpp_vm_address_region_flags(VmAddressRegion* vmar) { return vmar->flags(); }
const char* cpp_vm_address_region_name(VmAddressRegion* vmar) { return vmar->name(); }
bool cpp_vm_address_region_has_parent(VmAddressRegion* vmar) { return vmar->has_parent(); }
zx_status_t cpp_vm_address_region_set_memory_priority(VmAddressRegion* vmar,
                                                      VmAddressRegion::MemoryPriority priority) {
  return vmar->SetMemoryPriority(priority);
}
zx_status_t cpp_vm_address_region_unmap(VmAddressRegion* vmar, vaddr_t base, size_t size,
                                        VmAddressRegionOpChildren op_children) {
  return vmar->Unmap(base, size, op_children);
}
zx_status_t cpp_vm_address_region_protect(VmAddressRegion* vmar, vaddr_t base, size_t size,
                                          arch_mmu_flags_t new_arch_mmu_flags,
                                          VmAddressRegionOpChildren op_children) {
  return vmar->Protect(base, size, new_arch_mmu_flags, op_children);
}
zx_status_t cpp_vm_address_region_reserve_space(VmAddressRegion* vmar, const char* name,
                                                size_t base, size_t size,
                                                arch_mmu_flags_t arch_mmu_flags) {
  return vmar->ReserveSpace(name, base, size, arch_mmu_flags);
}
VmAddressRegion* cpp_vm_address_region_create_sub_vmar(VmAddressRegion* vmar, size_t offset,
                                                       size_t size, uint8_t align_pow2,
                                                       uint32_t vmar_flags, const char* name,
                                                       zx_status_t* out_status) {
  fbl::RefPtr<VmAddressRegion> sub_vmar;
  *out_status = vmar->CreateSubVmar(offset, size, align_pow2, vmar_flags, name, &sub_vmar);
  return fbl::ExportToRawPtr(&sub_vmar);
}
VmMapping* cpp_vm_address_region_create_vm_mapping(
    VmAddressRegion* vmar, size_t mapping_offset, size_t size, uint8_t align_pow2,
    uint32_t vmar_flags, const VmObject* vmo, uint64_t vmo_offset, arch_mmu_flags_t arch_mmu_flags,
    const char* name, vaddr_t* out_base, zx_status_t* out_status) {
  fbl::RefPtr<VmObject> vmo_ref = fbl::ImportFromRawPtr(const_cast<VmObject*>(vmo));
  auto result = vmar->CreateVmMapping(mapping_offset, size, align_pow2, vmar_flags,
                                      ktl::move(vmo_ref), vmo_offset, arch_mmu_flags, name);
  if (result.is_error()) {
    *out_status = result.status_value();
    return nullptr;
  }
  *out_status = ZX_OK;
  *out_base = result->base;
  fbl::RefPtr<VmMapping> mapping = ktl::move(result->mapping);
  return fbl::ExportToRawPtr(&mapping);
}
}
