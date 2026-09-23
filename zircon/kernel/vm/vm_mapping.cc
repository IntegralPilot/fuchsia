// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <assert.h>
#include <inttypes.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <ktl/algorithm.h>
#include <ktl/iterator.h>
#include <ktl/utility.h>
#include <vm/fault.h>
#include <vm/physmap.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>
#include <vm/vm_object_paged.h>
#include <vm/vm_object_physical.h>

#include "vm/vm_address_region.h"
#include "vm_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE VM_GLOBAL_TRACE(0)

namespace {

KCOUNTER(vm_mapping_attribution_queries, "vm.attributed_memory.mapping.queries")
KCOUNTER(vm_mappings_merged, "vm.aspace.mapping.merged_neighbors")
KCOUNTER(vm_mappings_protect_no_write, "vm.aspace.mapping.protect_without_write")
KCOUNTER(vm_mappings_state_dead, "vm.aspace.mapping.state.dead")

}  // namespace

// Helper class for managing the logic of skipping certain unmap operations for in progress faults.
// This is expected to be stack allocated under the object lock and the object lock must not be
// dropped over its lifetime.
// Creating this object creates a contract where the caller will either update the mapping for this
// location and call Success, or this object will automatically unmap the location if necessary.
class VmMapping::CurrentlyFaulting {
 public:
  CurrentlyFaulting(VmMapping* mapping, uint64_t object_offset, uint64_t len)
      TA_REQ(mapping->object_->lock())
      : mapping_(mapping), object_offset_(object_offset), len_(len) {
    DEBUG_ASSERT(mapping->currently_faulting_ == nullptr);
    // CurrentlyFaulting is typically allocated on the stack and GCCs diagnostics can get confused
    // and fail to realize that the destructor will clear the pointing, causing GCC to believe that
    // there might be a dangling pointer.
#if !defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdangling-pointer"
#endif
    mapping->currently_faulting_ = this;
#if !defined(__clang__)
#pragma GCC diagnostic pop
#endif
  }
  ~CurrentlyFaulting() {
    // If the caller did not call Success, and an unmap was skipped, then we must unmap the range
    // ourselves. We only do the unmap here if a prior unmap was skipped to avoid needless unmaps
    // due to transient errors such as needing to wait on a page request.
    if (state_ == State::UnmapSkipped) {
      vaddr_t base;
      size_t new_len;
      bool valid_range = mapping_->ObjectRangeToVaddrRange(object_offset_, len_, &base, &new_len);
      ASSERT(valid_range);
      ASSERT(new_len == len_);
      zx_status_t status = mapping_->aspace_->arch_aspace().Unmap(
          base, new_len / kPageSize, mapping_->aspace_->EnlargeArchUnmap());
      ASSERT(status == ZX_OK);
    }
    mapping_->currently_faulting_ = nullptr;
  }

  // Called to say that the given range needs to be unmapped. This returns true if updating the
  // range will be handled by the faulting thread and that the unmap can therefore be skipped.
  // Returns false if the caller should unmap themselves.
  bool UnmapRange(uint64_t object_offset, uint64_t len) {
    DEBUG_ASSERT(state_ != State::Completed);
    if (Intersects(object_offset, len, object_offset_, len_)) {
      state_ = State::UnmapSkipped;
      return true;
    }
    return false;
  }

  // Called to indicate that the mapping for the fault location has been updated successfully. This
  // acts to cancel the unmap that would otherwise happen when this object goes out of scope.
  void MappingUpdated() { state_ = State::Completed; }

  DISALLOW_COPY_ASSIGN_AND_MOVE(CurrentlyFaulting);

 private:
  // Reference back to the original mapping.
  VmMapping* mapping_;
  // The offset, in object space, of the page fault.
  uint64_t object_offset_;
  uint64_t len_;
  enum class State {
    NoUnmapNeeded,
    UnmapSkipped,
    Completed,
  };
  State state_ = State::NoUnmapNeeded;
};

VmMapping::VmMapping(VmAddressRegion& parent, bool private_clone, vaddr_t base, size_t size,
                     uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset,
                     arch_mmu_flags_t first_mmu_flags,
                     btree::BTree<vaddr_t, arch_mmu_flags_t>&& ranges, Mergeable mergeable)
    : VmAddressRegionOrMapping(base, size, vmar_flags, parent.aspace_.get(), &parent, true),
      mergeable_(mergeable),
      private_clone_(private_clone),
      first_region_arch_mmu_flags_(first_mmu_flags),
      rest_protection_ranges_(ktl::move(ranges)),
      object_(ktl::move(vmo)),
      object_offset_(vmo_offset) {
  LTRACEF("%p aspace %p base %#" PRIxPTR " size %#zx offset %#" PRIx64 "\n", this, aspace_.get(),
          base_, size_, vmo_offset);
}

VmMapping::VmMapping(VmAddressRegion& parent, bool private_clone, vaddr_t base, size_t size,
                     uint32_t vmar_flags, fbl::RefPtr<VmObject> vmo, uint64_t vmo_offset,
                     arch_mmu_flags_t arch_mmu_flags, Mergeable mergeable)
    : VmMapping(parent, private_clone, base, size, vmar_flags, vmo, vmo_offset, arch_mmu_flags,
                btree::BTree<vaddr_t, arch_mmu_flags_t>(), mergeable) {}

VmMapping::~VmMapping() {
  canary_.Assert();
  LTRACEF("%p aspace %p base %#" PRIxPTR " size %#zx\n", this, aspace_.get(), base_, size_);
  vm_mappings_state_dead.Add(-1);
}

fbl::RefPtr<VmObject> VmMapping::vmo() const {
  Guard<CriticalMutex> guard{lock()};
  return vmo_locked();
}

VmMapping::AttributionCounts VmMapping::GetAttributedMemoryLocked(
    Guard<CriticalMutex>& guard) const {
  canary_.Assert();

  if (!IsAliveLocked()) {
    return AttributionCounts{};
  }

  vm_mapping_attribution_queries.Add(1);

  fbl::RefPtr<VmObject> vmo = object_;
  const uint64_t object_offset = object_offset_;
  VmMapping::AttributionCounts page_counts;
  guard.CallUnlocked(
      [&]() { page_counts = vmo->GetAttributedMemoryInRange(object_offset, size_); });
  return page_counts;
}

VmMapping::AttributionCounts VmMapping::GetAttributedMemory() const {
  canary_.Assert();

  Guard<CriticalMutex> guard{lock()};
  return GetAttributedMemoryLocked(guard);
}

void VmMapping::DumpLocked(uint depth, bool verbose) const {
  canary_.Assert();
  for (uint i = 0; i < depth; ++i) {
    printf("  ");
  }
  char vmo_name[32];
  object_->get_name(vmo_name, sizeof(vmo_name));
  printf("map %p [%#" PRIxPTR " %#" PRIxPTR "] sz %#zx state %d mergeable %s\n", this, base_,
         base_ + size_ - 1, size_, (int)state_locked(),
         mergeable_ == Mergeable::YES ? "true" : "false");
  EnumerateProtectionRangesLocked(
      base_, size_, [depth](vaddr_t base, size_t len, arch_mmu_flags_t mmu_flags) {
        for (uint i = 0; i < depth + 1; ++i) {
          printf("  ");
        }
        printf(" [%#" PRIxPTR " %#" PRIxPTR "] mmufl %#x\n", base, base + len - 1, mmu_flags);
        return ZX_ERR_NEXT;
      });
  for (uint i = 0; i < depth + 1; ++i) {
    printf("  ");
  }
  AttributionCounts counts = object_->GetAttributedMemoryInRange(object_offset_, size_);
  printf("vmo %p/k%" PRIu64 " off %#" PRIx64 " bytes (%zu/%zu) ref %d '%s'\n", object_.get(),
         object_->user_id(), object_offset_, counts.uncompressed_bytes, counts.compressed_bytes,
         ref_count_debug(), vmo_name);
  if (verbose) {
    object_->Dump(depth + 1, false);
  }
}

using ArchUnmapOptions = ArchVmAspaceInterface::ArchUnmapOptions;

// static
zx_status_t VmMapping::ProtectOrUnmap(const fbl::RefPtr<VmAspace>& aspace, vaddr_t base,
                                      size_t size, arch_mmu_flags_t new_arch_mmu_flags) {
  // This can never be used to set a WRITE permission since it does not ask the underlying VMO to
  // perform the copy-on-write step. The underlying VMO might also support dirty tracking, which
  // requires write permission faults in order to track pages as dirty when written.
  ASSERT(!(new_arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE));
  // If not removing all permissions do the protect, otherwise skip straight to unmapping the entire
  // region.
  if ((new_arch_mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK) != 0) {
    zx_status_t status = aspace->arch_aspace().Protect(
        base, size / kPageSize, new_arch_mmu_flags,
        aspace->can_enlarge_arch_unmap() ? ArchUnmapOptions::Enlarge : ArchUnmapOptions::None);
    // If the unmap failed and we are allowed to unmap extra portions of the aspace then fall
    // through and unmap, otherwise return with whatever the status is.
    if (likely(status == ZX_OK) || !aspace->can_enlarge_arch_unmap()) {
      return status;
    }
  }

  return aspace->arch_aspace().Unmap(base, size / kPageSize, aspace->EnlargeArchUnmap());
}

zx_status_t VmMapping::CopyProtectionRangesLocked(
    vaddr_t base, size_t size, arch_mmu_flags_t* out_first_flags,
    btree::BTree<vaddr_t, arch_mmu_flags_t>* out_ranges) const {
  DEBUG_ASSERT(is_in_range(base, size));
  DEBUG_ASSERT(out_first_flags);
  DEBUG_ASSERT(out_ranges);
  return EnumerateProtectionRangesLocked(
      base, size, [base, out_first_flags, out_ranges](vaddr_t b, size_t s, arch_mmu_flags_t flags) {
        if (b == base) {
          *out_first_flags = flags;
        } else {
          if (!out_ranges->insert(b, flags)) {
            return ZX_ERR_NO_MEMORY;
          }
        }
        return ZX_ERR_NEXT;
      });
}

zx_status_t VmMapping::MergeProtectionRangesLocked(const VmMapping& right) {
  DEBUG_ASSERT(right.base() == base() + size());
  // Lookup the flags for the end of the range.
  auto it = rest_protection_ranges_.end();
  it--;
  arch_mmu_flags_t last_flags = it ? (*it).second : first_region_arch_mmu_flags_;
  if (last_flags != right.first_region_arch_mmu_flags_) {
    it = rest_protection_ranges_.insert(it, right.base(), right.first_region_arch_mmu_flags_);
    if (!it) {
      return ZX_ERR_NO_MEMORY;
    }
  }
  for (auto [key, value] : right.rest_protection_ranges_) {
    it = rest_protection_ranges_.insert(it, key, value);
    if (!it) {
      return ZX_ERR_NO_MEMORY;
    }
  }
  return ZX_OK;
}

void VmMapping::ClearProtectionRangeTransitionsLocked(vaddr_t base, vaddr_t end) {
  auto it = rest_protection_ranges_.lower_bound(base);
  while (it.IsValid() && (*it).first < end) {
    it = rest_protection_ranges_.erase(it);
  }
}

arch_mmu_flags_t VmMapping::RemoveAfterSplitLocked(vaddr_t split_addr) {
  arch_mmu_flags_t flags = first_region_arch_mmu_flags_;
  auto it = rest_protection_ranges_.begin();
  while (it.IsValid() && (*it).first <= split_addr) {
    flags = (*it).second;
    it = rest_protection_ranges_.erase(it);
  }
  return flags;
}

zx_status_t VmMapping::ProtectLocked(vaddr_t base, size_t size,
                                     arch_mmu_flags_t new_arch_mmu_flags) {
  // Assert a few things that should already have been checked by the caller.
  DEBUG_ASSERT(size != 0 && IsPageRounded(base) && IsPageRounded(size));
  DEBUG_ASSERT(!(new_arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK));
  DEBUG_ASSERT(is_valid_mapping_flags(new_arch_mmu_flags));

  DEBUG_ASSERT(object_);
  // grab the lock for the vmo
  Guard<CriticalMutex> guard{object_->lock()};

  // Persist our current caching mode. Every protect region will have the same caching mode so we
  // can acquire this from any region.
  new_arch_mmu_flags |= (first_region_arch_mmu_flags_ & ARCH_MMU_FLAG_CACHE_MASK);

  // This will get called by UpdateProtectionRange below for every existing unique protection range
  // that gets changed and allows us to fine tune the protect action based on the previous flags.
  auto protect_callback = [new_arch_mmu_flags, this](vaddr_t base, size_t size,
                                                     arch_mmu_flags_t old_arch_mmu_flags) {
    // Perform an early return if the new and old flags are the same, as there's nothing to be done.
    if (new_arch_mmu_flags == old_arch_mmu_flags) {
      return ZX_ERR_NEXT;
    }

    arch_mmu_flags_t flags = new_arch_mmu_flags;
    // Check if the new flags have the write permission. This is problematic as we cannot just
    // change any existing hardware mappings to have the write permission, as any individual mapping
    // may be the result of a read fault and still need to have a copy-on-write step performed. This
    // could also map a dirty tracked VMO which requires write permission faults to track pages as
    // dirty when written.
    if (new_arch_mmu_flags & ARCH_MMU_FLAG_PERM_WRITE) {
      // Whatever happens, we're not going to be protecting the arch aspace to have write mappings,
      // so this has to be a user aspace so that we can lazily take write faults in the future.
      ASSERT(aspace_->is_user() || aspace_->is_guest_physical());
      flags &= ~ARCH_MMU_FLAG_PERM_WRITE;
      vm_mappings_protect_no_write.Add(1);
      // If the new flags without write permission are the same as the old flags, then skip the
      // protect step since it will be a no-op.
      if (flags == old_arch_mmu_flags) {
        return ZX_ERR_NEXT;
      }
    }

    zx_status_t status = ProtectOrUnmap(aspace_, base, size, flags);
    // If the protect failed then we do not have sufficient information left to rollback in order to
    // return an error, nor can we claim success, so require the protect to have succeeded to
    // continue.
    ASSERT(status == ZX_OK);
    return ZX_ERR_NEXT;
  };

  // To handle the protection change, we need to ensure that the start and end of the requested
  // range are represented as boundary nodes in the protection_ranges_ B-tree if they are not
  // already. This ensures that any change in protections for the sub-range does not
  // unintentionally leak into adjacent sub-ranges.

  // Efficiently calculate the flags before, at, and after the requested range using
  // minimal BTree searches.
  arch_mmu_flags_t flags_before = first_region_arch_mmu_flags_;
  arch_mmu_flags_t flags_at_base = first_region_arch_mmu_flags_;
  bool base_exists = false;

  auto it_base = rest_protection_ranges_.upper_bound(base);
  it_base--;
  if (it_base.IsValid()) {
    auto [addr, flags] = *it_base;
    if (addr == base) {
      base_exists = true;
      flags_at_base = flags;
      it_base--;
      if (it_base.IsValid()) {
        auto [prev_addr, prev_flags] = *it_base;
        flags_before = prev_flags;
      }
    } else {
      flags_at_base = flags;
      flags_before = flags_at_base;
    }
  }

  arch_mmu_flags_t flags_end = first_region_arch_mmu_flags_;
  bool end_exists = false;

  auto it_end = rest_protection_ranges_.upper_bound(base + size);
  it_end--;
  if (it_end.IsValid()) {
    auto [addr, flags] = *it_end;
    if (addr == base + size) {
      end_exists = true;
      flags_end = flags;
    } else {
      flags_end = flags;
    }
  }

  // We only need a boundary node at 'base' if it's not the start of the mapping and the
  // preceding flags are different from the new ones.
  bool need_base = base > base_ && flags_before != new_arch_mmu_flags;
  bool insert_base = need_base && !base_exists;

  // Similarly, we need a boundary node at 'base + size' if it's not the end of the mapping and
  // the new flags for the range are different from the existing flags that follow it.
  bool need_end = base + size < base_ + size_ && new_arch_mmu_flags != flags_end;
  bool insert_end = need_end && !end_exists;

  // Perform any required insertions first. By inserting the *old* flags initially, we make
  // the insertion a structural change only (a no-op for effective permissions). This allows
  // us to handle OOM errors before we begin any non-reversible mutations.
  if (insert_base) {
    auto it = rest_protection_ranges_.insert(base, flags_at_base);
    if (!it.IsValid()) {
      return ZX_ERR_NO_MEMORY;
    }
  }
  if (insert_end) {
    auto it = rest_protection_ranges_.insert(base + size, flags_end);
    if (!it.IsValid()) {
      if (insert_base) {
        rest_protection_ranges_.erase(rest_protection_ranges_.find(base));
      }
      return ZX_ERR_NO_MEMORY;
    }
  }

  // Now that all structural nodes are guaranteed to exist, we can safely perform the
  // architectural protection changes. Calling this after successful insertions ensures
  // that we don't have to rollback MMU changes if an allocation fails.
  EnumerateProtectionRangesLocked(base, size, protect_callback);

  // Update the protection flags for the start of the range.
  if (base == base_) {
    first_region_arch_mmu_flags_ = new_arch_mmu_flags;
  } else if (need_base) {
    rest_protection_ranges_.update(rest_protection_ranges_.find(base), new_arch_mmu_flags);
  } else {
    // If the boundary node is no longer needed (e.g., flags now match), remove it.
    auto it = rest_protection_ranges_.find(base);
    if (it.IsValid()) {
      rest_protection_ranges_.erase(it);
    }
  }

  // If a boundary node at 'base + size' is not needed, remove it to keep the tree minimal.
  if (!need_end) {
    auto it = rest_protection_ranges_.find(base + size);
    if (it.IsValid()) {
      rest_protection_ranges_.erase(it);
    }
  }

  // Finally, erase any internal boundary nodes that were entirely within the protected range,
  // as the entire range [base, base + size) now has uniform protections.
  ClearProtectionRangeTransitionsLocked(base + 1, base + size);

  return ZX_OK;
}

zx_status_t VmMapping::UnmapLocked(vaddr_t base, size_t size) {
  canary_.Assert();
  DEBUG_ASSERT(size != 0 && IsPageRounded(size) && IsPageRounded(base));
  DEBUG_ASSERT(base >= base_ && base - base_ < size_);
  DEBUG_ASSERT(size_ - (base - base_) >= size);
  DEBUG_ASSERT(parent_);

  // Take a ref to ourselves in case we drop the last one when removing from our parent.
  fbl::RefPtr<VmMapping> self(this);

  if (state_ != LifeCycleState::ALIVE) {
    return ZX_ERR_BAD_STATE;
  }

  AssertHeld(parent_->lock_ref());
  AssertHeld(parent_->region_lock_ref());

  // Should never be unmapping everything, otherwise should destroy.
  DEBUG_ASSERT(base != base_ || size != size_);

  LTRACEF("%p\n", this);

  // First create any new mapping. One or two might be required depending on whether unmapping from
  // an end or the middle.
  fbl::RefPtr<VmMapping> left, right;
  if (base_ != base) {
    // Insert empty protection ranges for now as we will transform the existing protection_ranges_
    // tree into the target.
    fbl::AllocChecker ac;
    left = fbl::AdoptRef(new (&ac) VmMapping(
        *parent_, private_clone_, base_, base - base_, flags_, object_, object_offset_, 0,
        btree::BTree<vaddr_t, arch_mmu_flags_t>(), Mergeable::YES));
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
      AssertHeld(parent_->region_lock_ref());
    }
  }
  if (base + size != base_ + size_) {
    // If we also have a left mapping then we need to build a copy of the subset of the protection
    // ranges for this right mapping. We do this first here as this might need to allocate and can
    // fail.
    arch_mmu_flags_t right_first_mmu_flags = 0;
    btree::BTree<vaddr_t, arch_mmu_flags_t> right_prot_ranges;
    const vaddr_t offset = base + size - base_;
    if (left) {
      zx_status_t status = CopyProtectionRangesLocked(base + size, size_ - offset,
                                                      &right_first_mmu_flags, &right_prot_ranges);
      if (status != ZX_OK) {
        return status;
      }
    }
    fbl::AllocChecker ac;
    right = fbl::AdoptRef(new (&ac) VmMapping(*parent_, private_clone_, base_ + offset,
                                              size_ - offset, flags_, object_,
                                              object_offset_ + offset, right_first_mmu_flags,
                                              ktl::move(right_prot_ranges), Mergeable::YES));
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
  }

  // Grab the lock for the vmo. This is acquired here so that it is held continuously over both the
  // architectural unmap and removing the current mapping from the VMO.
  DEBUG_ASSERT(object_);
  Guard<CriticalMutex> guard{object_->lock()};

  // With the object allocations done the last action that could fail is insertion into the
  // subregions_ list and object mapping list. Notionally we want to remove the old mapping, then
  // install our new mapping(s), but if our installation fails then we cannot necessarily rollback
  // and install the old mapping. To work around this we instead first install any right mapping,
  // which could fail and then if there is a left mapping replace the current mapping, which cannot
  // fail. This approach has the result of temporarily causing the subregions_ list to have an
  // overlapping entry and for the parent to have unactivated mappings, however we hold both the
  // main and subregion lock over the entire operation, and so this is never visible. Similarly we
  // hold the object lock over the entire manipulation of mappings, and so the mappings being added
  // before being in the alive state is never visible.
  if (right) {
    zx_status_t status = parent_->subregions_.InsertRegion(right);
    if (status != ZX_OK) {
      return status;
    }
    status = object_->AddMappingLocked(right.get());
    if (status != ZX_OK) {
      parent_->subregions_.RemoveRegion(right.get());
      return status;
    }
  }
  if (left) {
    zx_status_t status = object_->AddMappingLocked(left.get());
    if (status != ZX_OK) {
      if (right) {
        object_->RemoveMappingLocked(right.get());
        parent_->subregions_.RemoveRegion(right.get());
      }
      return status;
    }
    // Replace can never fail as it does not need to allocate.
    parent_->subregions_.ReplaceRegion(this, left);
  } else {
    parent_->subregions_.RemoveRegion(this);
  }

  zx_status_t status =
      aspace_->arch_aspace().Unmap(base, size / kPageSize, aspace_->EnlargeArchUnmap());
  ASSERT(status == ZX_OK);

  const MemoryPriority old_priority = memory_priority_;
  auto set_priority =
      [old_priority](VmMapping& self) TA_REQ(self.lock()) TA_REQ(self.object_->lock()) {
        if (old_priority == VmAddressRegion::MemoryPriority::HIGH) {
          self.SetMemoryPriorityHighAlreadyPositiveLockedObject</*SplitOnUnmap=*/true>();
        } else {
          DEBUG_ASSERT(old_priority == VmAddressRegion::MemoryPriority::DEFAULT);
          self.SetMemoryPriorityDefaultLockedObject</*SplitOnUnmap=*/true>();
        }
      };

  // Split the protection_ranges_ from this mapping into the new mapping(s). This has be done after
  // the mapping construction as this step is destructive and hard to rollback.
  //
  // Need to set memory priorities before we call DestroyLockedObject. If we have
  // MemoryPriority::HIGH, then we need to pass that on to left and right before object_ and
  // aspace_ suffer any dynamic reclamation.
  if (right) {
    AssertHeld(right->lock_ref());
    AssertHeld(right->object_lock_ref());
    // If there was a left mapping we already populated a new protection_ranges above when we
    // created the right mapping, for the other case we need to erase the unneeded parts of our
    // protection_ranges_ and move it over.
    if (!left) {
      right->first_region_arch_mmu_flags_ = RemoveAfterSplitLocked(base + size);
      right->rest_protection_ranges_ = ktl::move(rest_protection_ranges_);
    }
    set_priority(*right);
  }
  if (left) {
    AssertHeld(left->lock_ref());
    AssertHeld(left->object_lock_ref());
    // Erase the top part of protection_ranges_ and then move it to the left mapping.
    ClearProtectionRangeTransitionsLocked(base, base_ + size_);
    left->first_region_arch_mmu_flags_ = first_region_arch_mmu_flags_;
    left->rest_protection_ranges_ = ktl::move(rest_protection_ranges_);
    set_priority(*left);
  }

  // Now finish destroying this mapping. We already updated the subregion_ list in the parent.
  status = DestroyLockedObject(DestroyUnmap::No, DestroyRemoveFromParent::No);
  ASSERT(status == ZX_OK);

  // Install the new mappings.
  auto finish_mapping = [](fbl::RefPtr<VmMapping>& mapping) {
    if (mapping) {
      AssertHeld(mapping->lock_ref());
      AssertHeld(mapping->object_lock_ref());
      AssertHeld(mapping->region_lock_ref());
      // We already updated the parent region list and object mapping list.
      mapping->ActivateNoInsertLocked();
    }
  };
  finish_mapping(left);
  finish_mapping(right);
  return ZX_OK;
}

bool VmMapping::ObjectRangeToVaddrRange(uint64_t offset, uint64_t len, vaddr_t* base,
                                        uint64_t* virtual_len) const {
  DEBUG_ASSERT(IsPageRounded(offset));
  DEBUG_ASSERT(IsPageRounded(len));
  DEBUG_ASSERT(base);
  DEBUG_ASSERT(virtual_len);

  // Zero sized ranges are considered to have no overlap.
  if (len == 0) {
    *base = 0;
    *virtual_len = 0;
    return false;
  }

  // compute the intersection of the passed in vmo range and our mapping
  uint64_t offset_new;
  if (!GetIntersect(object_offset_, static_cast<uint64_t>(size()), offset, len, &offset_new,
                    virtual_len)) {
    return false;
  }

  DEBUG_ASSERT(*virtual_len > 0 && *virtual_len <= SIZE_MAX);
  DEBUG_ASSERT(offset_new >= object_offset_);

  LTRACEF("intersection offset %#" PRIx64 ", len %#" PRIx64 "\n", offset_new, *virtual_len);

  // make sure the base + offset is within our address space
  // should be, according to the range stored in base_ + size_
  bool overflowed = add_overflow(this->base(), offset_new - object_offset_, base);
  ASSERT(!overflowed);

  // make sure we're only operating within our window
  ASSERT(*base >= this->base());
  ASSERT((*base + *virtual_len - 1) <= (this->base() + size() - 1));

  return true;
}

void VmMapping::AspaceUnmapLockedObject(uint64_t offset, uint64_t len, UnmapOptions options) const {
  canary_.Assert();

  // NOTE: must be acquired with the vmo lock held, but doesn't need to take
  // the address space lock, since it will not manipulate its location in the
  // vmar tree. However, it must be held in the ALIVE state across this call.
  //
  // Avoids a race with DestroyLocked() since it removes ourself from the VMO's
  // mapping list with the VMO lock held before dropping this state to DEAD. The
  // VMO cant call back to us once we're out of their list.
  DEBUG_ASSERT(get_state_locked_object() == LifeCycleState::ALIVE);

  // |object_| itself is not accessed in this method, and we do not hold the correct lock for it,
  // but we know the object_->lock() is held and so therefore object_ is valid and will not be
  // modified. Therefore it's correct to read object_ here for the purposes of an assert, but cannot
  // be expressed nicely with regular annotations.
  [&]() TA_NO_THREAD_SAFETY_ANALYSIS { DEBUG_ASSERT(object_); }();

  // In the case of unmapping known instances of the zero page check if this range intersects with
  // an in progress fault. If it does we can skip the unmap with the knowledge that the mapping will
  // be updated later. This is safe since the zero page is, by definition, only mapped read only,
  // and is never modified so delaying the update of the mapping cannot cause either any users to
  // see incorrect data, or users to be able to modify an old mapping.
  if ((options & UnmapOptions::OnlyHasZeroPages) && currently_faulting_ &&
      currently_faulting_->UnmapRange(offset, len)) {
    return;
  }

  LTRACEF("region %p obj_offset %#" PRIx64 " size %zu, offset %#" PRIx64 " len %#" PRIx64 "\n",
          this, object_offset_, size_, offset, len);

  // See if there's an intersect.
  vaddr_t base;
  uint64_t new_len;
  if (!ObjectRangeToVaddrRange(offset, len, &base, &new_len)) {
    return;
  }

  // If this is a kernel mapping then we should not be removing mappings out of the arch aspace,
  // unless this mapping has explicitly opted out of this check.
  DEBUG_ASSERT(aspace_->is_user() || aspace_->is_guest_physical() ||
               flags_ & VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING);

  auto aspace_op = aspace_->EnlargeArchUnmap();
  if (options & UnmapOptions::Harvest) {
    aspace_op |= ArchUnmapOptions::Harvest;
  }

  zx_status_t status = aspace_->arch_aspace().Unmap(base, new_len / kPageSize, aspace_op);
  ASSERT(status == ZX_OK);
}

void VmMapping::AspaceRemoveWriteLockedObject(uint64_t offset,
                                              uint64_t len) const TA_NO_THREAD_SAFETY_ANALYSIS {
  LTRACEF("region %p obj_offset %#" PRIx64 " size %zu, offset %#" PRIx64 " len %#" PRIx64 "\n",
          this, object_offset_, size_, offset, len);

  canary_.Assert();

  // NOTE: must be acquired with the vmo lock held, but doesn't need to take
  // the address space lock, since it will not manipulate its location in the
  // vmar tree. However, it must be held in the ALIVE state across this call.
  //
  // Avoids a race with DestroyLocked() since it removes ourself from the VMO's
  // mapping list with the VMO lock held before dropping this state to DEAD. The
  // VMO cant call back to us once we're out of their list.
  DEBUG_ASSERT(get_state_locked_object() == LifeCycleState::ALIVE);

  // |object_| itself is not accessed in this method, and we do not hold the correct lock for it,
  // but we know the object_->lock() is held and so therefore object_ is valid and will not be
  // modified. Therefore it's correct to read object_ here for the purposes of an assert, but cannot
  // be expressed nicely with regular annotations.
  [&]() TA_NO_THREAD_SAFETY_ANALYSIS { DEBUG_ASSERT(object_); }();

  // If this doesn't support writing then nothing to be done, as we know we have no write mappings.
  if (!(flags_ & VMAR_FLAG_CAN_MAP_WRITE)) {
    return;
  }

  // See if there's an intersect.
  vaddr_t base;
  uint64_t new_len;
  if (!ObjectRangeToVaddrRange(offset, len, &base, &new_len)) {
    return;
  }

  // If this is a kernel mapping then we should not be modify mappings in the arch aspace,
  // unless this mapping has explicitly opted out of this check.
  DEBUG_ASSERT_MSG(aspace_->is_user() || aspace_->is_guest_physical() ||
                       flags_ & VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING,
                   "region %p obj_offset %#" PRIx64 " size %zu, offset %#" PRIx64 " len %#" PRIx64
                   "\n",
                   this, object_offset_, size(), offset, len);

  zx_status_t status = EnumerateProtectionRangesLockedObject(
      base, new_len, [this](vaddr_t region_base, size_t region_len, arch_mmu_flags_t mmu_flags) {
        // If this range doesn't currently support being writable then we can skip.
        if (!(mmu_flags & ARCH_MMU_FLAG_PERM_WRITE)) {
          return ZX_ERR_NEXT;
        }

        // Build new mmu flags without writing.
        mmu_flags &= ~(ARCH_MMU_FLAG_PERM_WRITE);

        zx_status_t result = ProtectOrUnmap(aspace_, region_base, region_len, mmu_flags);
        if (result == ZX_OK) {
          return ZX_ERR_NEXT;
        }
        return result;
      });
  ASSERT(status == ZX_OK);
}

void VmMapping::AspaceDebugUnpinLockedObject(uint64_t offset, uint64_t len) const {
  LTRACEF("region %p obj_offset %#" PRIx64 " size %zu, offset %#" PRIx64 " len %#" PRIx64 "\n",
          this, object_offset_, size_, offset, len);

  canary_.Assert();

  // NOTE: must be acquired with the vmo lock held, but doesn't need to take
  // the address space lock, since it will not manipulate its location in the
  // vmar tree. However, it must be held in the ALIVE state across this call.
  //
  // Avoids a race with DestroyLocked() since it removes ourself from the VMO's
  // mapping list with the VMO lock held before dropping this state to DEAD. The
  // VMO cant call back to us once we're out of their list.
  DEBUG_ASSERT(get_state_locked_object() == LifeCycleState::ALIVE);

  // See if there's an intersect.
  vaddr_t base;
  uint64_t new_len;
  if (!ObjectRangeToVaddrRange(offset, len, &base, &new_len)) {
    return;
  }

  // This unpin is not allowed for kernel mappings, unless the mapping has specifically opted out of
  // this debug check due to it performing its own dynamic management.
  DEBUG_ASSERT(aspace_->is_user() || aspace_->is_guest_physical() ||
               flags_ & VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING);
}

namespace {

// Helper class for batching installing mappings into the arch aspace. The mappings object lock must
// be held over the entirety of the lifetime of this object, without ever being released.
template <size_t NumPages>
class VmMappingCoalescer {
 public:
  VmMappingCoalescer(VmMapping* mapping, vaddr_t base, arch_mmu_flags_t mmu_flags,
                     ArchVmAspace::ExistingEntryAction existing_entry_action)
      TA_REQ(mapping->object_lock());
  ~VmMappingCoalescer();

  // Add a page to the mapping run.
  zx_status_t Append(vaddr_t vaddr, paddr_t paddr) {
    // If this isn't the expected vaddr, flush the run we have first.
    if (!can_append(vaddr)) {
      zx_status_t status = Flush();
      if (status != ZX_OK) {
        return status;
      }
      base_ = vaddr;
    }
    phys_[count_] = paddr;
    ++count_;
    return ZX_OK;
  }

  zx_status_t AppendOrAdjustMapping(vaddr_t vaddr, paddr_t paddr, arch_mmu_flags_t mmu_flags) {
    // If this isn't the expected vaddr or mmu_flags have changed, flush the run we have first.
    if (!can_append(vaddr) || mmu_flags != mmu_flags_) {
      zx_status_t status = Flush();
      if (status != ZX_OK) {
        return status;
      }
      base_ = vaddr;
      mmu_flags_ = mmu_flags;
    }

    phys_[count_] = paddr;
    ++count_;
    return ZX_OK;
  }

  // How much space remains in the phys_ array, starting from vaddr, that can be used to
  // opportunistically map additional pages.
  size_t ExtraPageCapacityFrom(vaddr_t vaddr) {
    // vaddr must be appendable & the coalescer can't be empty.
    return (can_append(vaddr) && count_ != 0) ? NumPages - count_ : 0;
  }

  // Functions for the user to manually manage the pages array. It is up to the user to manage the
  // page count and ensure the coalescer doesn't overflow, maintains the correct page count and that
  // the pages are contiguous.
  paddr_t* GetNextPageSlot() { return &phys_[count_]; }

  arch_mmu_flags_t GetMmuFlags() { return mmu_flags_; }

  void IncrementCount(size_t i) { count_ += i; }

  // Submit any outstanding mappings to the MMU.
  zx_status_t Flush();

  size_t TotalMapped() { return total_mapped_; }

  // Drop the current outstanding mappings without sending them to the MMU.
  void Drop() { count_ = 0; }

 private:
  // Vaddr can be appended if it's the next free slot and the coalescer isn't full.
  bool can_append(vaddr_t vaddr) {
    return count_ < ktl::size(phys_) && vaddr == base_ + count_ * kPageSize;
  }

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmMappingCoalescer);

  VmMapping* mapping_;
  vaddr_t base_;
  paddr_t phys_[NumPages];
  size_t count_;
  size_t total_mapped_ = 0;
  arch_mmu_flags_t mmu_flags_;
  const ArchVmAspace::ExistingEntryAction existing_entry_action_;
};

template <size_t NumPages>
VmMappingCoalescer<NumPages>::VmMappingCoalescer(
    VmMapping* mapping, vaddr_t base, arch_mmu_flags_t mmu_flags,
    ArchVmAspace::ExistingEntryAction existing_entry_action)
    : mapping_(mapping),
      base_(base),
      count_(0),
      mmu_flags_(mmu_flags),
      existing_entry_action_(existing_entry_action) {
  // Mapping is only valid if there is at least some access in the flags.
  DEBUG_ASSERT(mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK);
}

template <size_t NumPages>
VmMappingCoalescer<NumPages>::~VmMappingCoalescer() {
  // Make sure no outstanding mappings.
  DEBUG_ASSERT(count_ == 0);
}

template <size_t NumPages>
zx_status_t VmMappingCoalescer<NumPages>::Flush() {
  if (count_ == 0) {
    return ZX_OK;
  }

  VM_KTRACE_DURATION(2, "map_page", ("va", ktrace::Pointer{base_}), ("count", count_),
                     ("mmu_flags", mmu_flags_));

  // Assert that we're not accidentally mapping the zero page writable. Unless called from a kernel
  // aspace, as the zero page can be mapped writeable from the kernel aspace in mexec.
  DEBUG_ASSERT(
      !(mmu_flags_ & ARCH_MMU_FLAG_PERM_WRITE) ||
      ktl::all_of(phys_, &phys_[count_], [](paddr_t p) { return p != vm_get_zero_page_paddr(); }) ||
      !mapping_->aspace()->is_user());

  // The caller likely overwrote the previous contents of the new pages before supplying them to
  // this VmMappingCoalescer. It's important that these writes are observed before the page table
  // entries corresponding to these pages are written out. Otherwise, there is a risk of leaking the
  // previous content.
  //
  // This site needs special synchronization; other sites guarantee this order by at least releasing
  // and acquiring the object lock between manipulating the new pages and modifying mappings. See
  // https://fxrev.dev/517176302.
  arch::StoreMemoryBarrier();

  zx_status_t ret = mapping_->aspace()->arch_aspace().Map(base_, phys_, count_, mmu_flags_,
                                                          existing_entry_action_);
  if (ret != ZX_OK) {
    TRACEF("error %d mapping %zu pages starting at va %#" PRIxPTR "\n", ret, count_, base_);
  }
  base_ += count_ * kPageSize;
  total_mapped_ += count_;
  count_ = 0;
  return ret;
}

}  // namespace

zx_status_t VmMapping::MapRange(size_t offset, size_t len, bool commit, bool ignore_existing) {
  Guard<CriticalMutex> aspace_guard{lock()};
  canary_.Assert();

  len = RoundUpPageSize(len);
  if (len == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (state_locked() != LifeCycleState::ALIVE) {
    return ZX_ERR_BAD_STATE;
  }

  LTRACEF("region %p, offset %#zx, size %#zx, commit %d\n", this, offset, len, commit);

  DEBUG_ASSERT(object_);
  if (!IsPageRounded(offset) || !is_in_range(base_ + offset, len)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // If this is a kernel mapping then validate that all pages being mapped are currently pinned,
  // ensuring that they cannot be taken away for any reason, unless the mapping has specifically
  // opted out of this debug check due to it performing its own dynamic management.
  DEBUG_ASSERT(aspace_->is_user() || aspace_->is_guest_physical() ||
               (flags_ & VMAR_FLAG_DEBUG_DYNAMIC_KERNEL_MAPPING) ||
               object_->DebugIsRangePinned(object_offset_ + offset, len));

  // Cache whether the object is dirty tracked, we need to know this when computing mmu flags later.
  const bool dirty_tracked = object_->is_dirty_tracked();

  // The region to map could have multiple different current arch mmu flags, so we need to iterate
  // over them to ensure we install mappings with the correct permissions.
  return EnumerateProtectionRangesLocked(
      base_ + offset, len,
      [this, commit, dirty_tracked, ignore_existing](vaddr_t base, size_t len,
                                                     arch_mmu_flags_t mmu_flags) {
        AssertHeld(lock_ref());

        // Remove the write permission if this maps a vmo that supports dirty tracking, in order to
        // trigger write permission faults when writes occur, enabling us to track when pages are
        // dirtied.
        if (dirty_tracked) {
          mmu_flags &= ~ARCH_MMU_FLAG_PERM_WRITE;
        }

        // If there are no access permissions on this region then mapping has no effect, so skip.
        if (!(mmu_flags & ARCH_MMU_FLAG_PERM_RWX_MASK)) {
          return ZX_ERR_NEXT;
        }

        // In the scenario where we are committing, and calling RequireOwnedPage, we are supposed to
        // pass in a non-null LazyPageRequest. Technically we could get away with not passing in a
        // PageRequest since:
        //  * Only internal kernel VMOs will have the 'commit' flag passed in for their mappings
        //  * Only pager backed VMOs or VMOs that support delayed memory allocations need to fill
        //    out a PageRequest
        //  * Internal kernel VMOs are never pager backed or have the delayed memory allocation flag
        //    set.
        // However, should these assumptions ever get violated it's better to catch this gracefully
        // than have RequireOwnedPage error/crash internally, and it costs nothing to create and
        // pass in.
        __UNINITIALIZED MultiPageRequest page_request;

        const uint64_t map_offset = base - base_;
        const uint64_t vmo_offset = object_offset_ + map_offset;
        if (VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object_.get()); likely(paged)) {
          // grab the lock for the vmo
          __UNINITIALIZED VmCowPages::DeferredOps deferred(paged->MakeDeferredOps());
          Guard<CriticalMutex> guard{AssertOrderedAliasedLock, paged->lock(), object_->lock(),
                                     paged->lock_order()};

          // Trim our range to the current VMO size. Our mapping might exceed the VMO in the case
          // where the VMO is resizable, and this should not be considered an error.
          len = TrimmedObjectRangeLocked(map_offset, len);
          if (len == 0) {
            return ZX_ERR_STOP;
          }

          VmMappingCoalescer<16> coalescer(this, base, mmu_flags,
                                           ignore_existing
                                               ? ArchVmAspace::ExistingEntryAction::Skip
                                               : ArchVmAspace::ExistingEntryAction::Error);

          const bool writing = mmu_flags & ARCH_MMU_FLAG_PERM_WRITE;
          __UNINITIALIZED auto cursor = paged->GetLookupCursorLocked(vmo_offset, len);
          if (cursor.is_error()) {
            return cursor.error_value();
          }
          // Do not consider pages touched when mapping in, if they are actually touched they will
          // get an accessed bit set in the hardware.
          cursor->DisableMarkAccessed();
          AssertHeld(cursor->lock_ref());
          for (uint64_t off = 0; off < len; off += kPageSize) {
            vm_page_t* page = nullptr;
            if (commit) {
              __UNINITIALIZED zx::result<VmCowPages::LookupCursor::RequireResult> result =
                  cursor->RequireOwnedPage(writing, 1, deferred, &page_request);
              if (result.is_error()) {
                zx_status_t status = result.error_value();
                // As per the comment above page_request definition, there should never be commit
                // + pager backed VMO and so we should never end up with a PageRequest needing to be
                // waited on.
                ASSERT(status != ZX_ERR_SHOULD_WAIT);
                // fail when we can't commit every requested page
                coalescer.Drop();
                return status;
              }
              page = result->page;
            } else {
              // Not committing so get a page if one exists. This increments the cursor, returning
              // nullptr if no page.
              page = cursor->MaybePage(writing);
              // This page was not present and if we are in a run of absent pages we would like to
              // efficiently skip them, instead of querying each virtual address individually. Due
              // to the assumptions of the cursor, we cannot call SkipMissingPages if we had just
              // requested the last page in the range of the cursor.
              if (!page && off + kPageSize < len) {
                // Increment |off| for the any pages we skip and let the original page from
                // MaybePage get incremented on the way around the loop before the range gets
                // checked.
                off += cursor->SkipMissingPages() * kPageSize;
              }
            }
            if (page) {
              zx_status_t status = coalescer.Append(base + off, page->paddr());
              if (status != ZX_OK) {
                return status;
              }
            }
          }
          zx_status_t status = coalescer.Flush();
          return status == ZX_OK ? ZX_ERR_NEXT : status;
        } else if (VmObjectPhysical* phys = DownCastVmObject<VmObjectPhysical>(object_.get());
                   phys) {
          // grab the lock for the vmo
          Guard<CriticalMutex> object_guard{AliasedLock, phys->lock(), object_->lock()};
          // Physical VMOs are never resizable, so do not need to worry about trimming the range.
          DEBUG_ASSERT(!phys->is_resizable());
          VmMappingCoalescer<16> coalescer(this, base, mmu_flags,
                                           ignore_existing
                                               ? ArchVmAspace::ExistingEntryAction::Skip
                                               : ArchVmAspace::ExistingEntryAction::Error);

          // Physical VMOs are always allocated and contiguous, just need to get the paddr.
          paddr_t phys_base = 0;
          zx_status_t status = phys->LookupContiguousLocked(vmo_offset, len, &phys_base);
          ASSERT(status == ZX_OK);

          for (size_t offset = 0; offset < len; offset += kPageSize) {
            status = coalescer.Append(base + offset, phys_base + offset);
            if (status != ZX_OK) {
              return status;
            }
          }
          status = coalescer.Flush();
          return status == ZX_OK ? ZX_ERR_NEXT : status;
        } else {
          panic("VmObject should be paged or physical");
          return ZX_ERR_INTERNAL;
        }
      });
}

zx_status_t VmMapping::DecommitRange(size_t offset, size_t len) {
  canary_.Assert();
  LTRACEF("%p [%#zx+%#zx], offset %#zx, len %#zx\n", this, base_, size_, offset, len);

  Guard<CriticalMutex> guard{lock()};
  if (state_locked() != LifeCycleState::ALIVE) {
    return ZX_ERR_BAD_STATE;
  }
  if (offset + len < offset || offset + len > size_) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  // VmObject::DecommitRange will typically call back into our instance's
  // VmMapping::AspaceUnmapLockedObject.
  return object_->DecommitRange(object_offset_ + offset, len);
}

zx_status_t VmMapping::DestroyLocked() {
  canary_.Assert();
  // Keep a refptr to the object_ so we know our lock remains valid.
  fbl::RefPtr<VmObject> object(object_);
  Guard<CriticalMutex> guard{object_->lock()};
  return DestroyLockedObject(DestroyUnmap::Yes, DestroyRemoveFromParent::Yes);
}

zx_status_t VmMapping::DestroyLockedObject(DestroyUnmap unmap,
                                           DestroyRemoveFromParent remove_region) {
  // Take a reference to ourself, so that we do not get destructed after
  // dropping our last reference in this method (e.g. when calling
  // subregions_.erase below).
  fbl::RefPtr<VmMapping> self(this);

  // If this is the last_fault_ then clear it before removing from the VMAR tree. Even if this
  // destroy fails, it's always safe to clear last_fault_, so we preference doing it upfront for
  // clarity.
  if (aspace_->last_fault_ == this) {
    aspace_->last_fault_ = nullptr;
  }

  // The vDSO code mapping can never be unmapped, not even
  // by VMAR destruction (except for process exit, of course).
  // TODO(mcgrathr): Turn this into a policy-driven process-fatal case
  // at some point.  teisenbe@ wants to eventually make zx_vmar_destroy
  // never fail.
  if (aspace_->vdso_code_mapping_ == self) {
    return ZX_ERR_ACCESS_DENIED;
  }

  if (unmap == DestroyUnmap::Yes) {
    zx_status_t status =
        aspace_->arch_aspace().Unmap(base_, size_ / kPageSize, aspace_->EnlargeArchUnmap());
    if (status != ZX_OK) {
      return status;
    }
  }

  // Remove any priority.
  SetMemoryPriorityDefaultLockedObject();

  rest_protection_ranges_.clear();
  object_->RemoveMappingLocked(this);

  // Detach the region from the parent.
  if (parent_ && remove_region == DestroyRemoveFromParent::Yes) {
    AssertHeld(parent_->lock_ref());
    AssertHeld(parent_->region_lock_ref());
    parent_->subregions_.RemoveRegion(this);
  }

  // detach from any object we have mapped. Note that we are holding the aspace_->lock() so we
  // will not race with other threads calling vmo()
  object_.reset();
  object_reset_ = true;

  // mark ourself as dead
  parent_ = nullptr;
  state_ = LifeCycleState::DEAD;
  vm_mappings_state_dead.Add(1);
  return ZX_OK;
}

template <typename T>
ktl::pair<zx_status_t, uint32_t> VmMapping::PageFaultLockedObject(vaddr_t va, uint pf_flags,
                                                                  size_t additional_pages,
                                                                  T* object,
                                                                  VmCowPages::DeferredOps* deferred,
                                                                  MultiPageRequest* page_request) {
  // Ensure the 'object' type is exactly the form we expect so that is_paged is calculated
  // correctly.
  static_assert(ktl::is_same_v<decltype(object), VmObjectPaged*> ||
                ktl::is_same_v<decltype(object), VmObjectPhysical*>);
  constexpr bool is_paged = ktl::is_same_v<decltype(object), VmObjectPaged*>;

  // Fault batch size when num_pages > 1.
  static constexpr uint64_t kBatchPages = 16;
  static constexpr uint64_t coalescer_size = ktl::max(kPageFaultMaxOptimisticPages, kBatchPages);

  const uint64_t vmo_offset = va - base_ + object_offset_;

  [[maybe_unused]] char pf_string[5];
  LTRACEF("%p va %#" PRIxPTR " vmo_offset %#" PRIx64 ", pf_flags %#x (%s)\n", this, va, vmo_offset,
          pf_flags, vmm_pf_flags_to_string(pf_flags, pf_string));

  // Need to look up the mmu flags for this virtual address, as well as how large a region those
  // flags are for so we can cap the extra mappings we create.
  const FlagsRange range = FlagsRangeAtAddrLockedObject(va);

  // Build the mmu flags we need to have based on the page fault. This strategy of building the
  // flags and then comparing all at once allows the compiler to provide much better code gen.
  arch_mmu_flags_t needed_mmu_flags = 0;
  if (pf_flags & VMM_PF_FLAG_USER) {
    needed_mmu_flags |= ARCH_MMU_FLAG_PERM_USER;
  }
  const bool write = pf_flags & VMM_PF_FLAG_WRITE;
  if (write) {
    needed_mmu_flags |= ARCH_MMU_FLAG_PERM_WRITE;
  } else {
    needed_mmu_flags |= ARCH_MMU_FLAG_PERM_READ;
  }
  if (pf_flags & VMM_PF_FLAG_INSTRUCTION) {
    needed_mmu_flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
  }
  // Check that all the needed flags are present.
  if (unlikely((range.mmu_flags & needed_mmu_flags) != needed_mmu_flags)) {
    if ((pf_flags & VMM_PF_FLAG_USER) && !(range.mmu_flags & ARCH_MMU_FLAG_PERM_USER)) {
      // user page fault on non user mapped region
      LTRACEF("permission failure: user fault on non user region\n");
    }
    if ((pf_flags & VMM_PF_FLAG_WRITE) && !(range.mmu_flags & ARCH_MMU_FLAG_PERM_WRITE)) {
      // write to a non-writeable region
      LTRACEF("permission failure: write fault on non-writable region\n");
    }
    if (!(pf_flags & VMM_PF_FLAG_WRITE) && !(range.mmu_flags & ARCH_MMU_FLAG_PERM_READ)) {
      // read to a non-readable region
      LTRACEF("permission failure: read fault on non-readable region\n");
    }
    if ((pf_flags & VMM_PF_FLAG_INSTRUCTION) && !(range.mmu_flags & ARCH_MMU_FLAG_PERM_EXECUTE)) {
      // instruction fetch from a no execute region
      LTRACEF("permission failure: execute fault on no execute region\n");
    }
    return {ZX_ERR_ACCESS_DENIED, 0};
  }

  // Calculate the number of pages from va until the end of the protection range.
  const size_t num_protection_range_pages = (range.region_top - va) / kPageSize;

  uint64_t vmo_size = object->size_locked();
  if constexpr (is_paged) {
    // If fault-beyond-stream-size is set, throw exception on memory accesses past the page
    // containing the user defined stream size.
    if (flags_ & VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE) {
      if (auto size = object->saturating_stream_size_locked()) {
        vmo_size = *size;
      }
    }
  }

  if (vmo_offset >= vmo_size) {
    return {ZX_ERR_OUT_OF_RANGE, 0};
  }

  // Calculate the maximum number of pages we can legally look at, i.e. are valid, in the vmo
  // taking into account the protection range, which is implicitly taking into account the mapping
  // size.
  const size_t num_vmo_pages = (vmo_size - vmo_offset) / kPageSize;
  const size_t num_valid_pages = ktl::min(num_protection_range_pages, num_vmo_pages);

  // Number of requested pages, trimmed to protection range & VMO.
  const size_t num_required_pages = ktl::min(num_valid_pages, additional_pages + 1);
  DEBUG_ASSERT(num_required_pages > 0);
  // Helper to calculate the remaining pt pages if we need them.
  auto calc_pt_pages = [](uint64_t va) {
    const uint64_t next_pt_base = ArchVmAspace::NextUserPageTableOffset(va);
    const size_t num_pt_pages = (next_pt_base - va) / kPageSize;
    return num_pt_pages;
  };
  // Number of pages we're aiming to fault. If a range > 1 page is supplied, it is assumed the
  // user knows the appropriate range, so opportunistic pages will not be added.
  const size_t num_fault_pages =
      additional_pages == 0
          ? ktl::min({kPageFaultMaxOptimisticPages, num_valid_pages, calc_pt_pages(va)})
          : num_required_pages;

  // Opportunistic pages are not considered in currently_faulting optimisation, as it is not
  // guaranteed the mappings will be updated.
  CurrentlyFaulting currently_faulting(this, vmo_offset, num_required_pages * kPageSize);

  __UNINITIALIZED VmMappingCoalescer<coalescer_size> coalescer(
      this, va, range.mmu_flags, ArchVmAspace::ExistingEntryAction::Upgrade);

  if constexpr (ktl::is_same_v<decltype(object), VmObjectPaged*>) {
    // fault in or grab existing pages.
    const size_t cursor_size = num_fault_pages * kPageSize;
    __UNINITIALIZED auto cursor = object->GetLookupCursorLocked(vmo_offset, cursor_size);
    if (cursor.is_error()) {
      return {cursor.error_value(), coalescer.TotalMapped()};
    }
    // Do not consider pages touched when mapping in, if they are actually touched they will
    // get an accessed bit set in the hardware.
    cursor->DisableMarkAccessed();
    AssertHeld(cursor->lock_ref());

    // Fault requested pages.
    uint64_t offset = 0;
    for (; offset < (num_required_pages * kPageSize); offset += kPageSize) {
      arch_mmu_flags_t curr_mmu_flags = range.mmu_flags;

      uint64_t num_curr_pages = num_required_pages - (offset / kPageSize);
      __UNINITIALIZED zx::result<VmCowPages::LookupCursor::RequireResult> result =
          cursor->RequirePage(write, num_curr_pages, *deferred, page_request);
      if (result.is_error()) {
        coalescer.Flush();
        return {result.error_value(), coalescer.TotalMapped()};
      }

      DEBUG_ASSERT(!write || result->writable);

      // We looked up in order to write. Mark as modified. Only need to do this once.
      if (write && offset == 0) {
        object->mark_modified_locked();
      }

      // If we read faulted, and lookup didn't say that this is always writable, then we map or
      // modify the page without any write permissions. This ensures we will fault again if a
      // write is attempted so we can potentially replace this page with a copy or a new one, or
      // update the page's dirty state.
      if (!write && !result->writable) {
        // we read faulted, so only map with read permissions
        curr_mmu_flags &= ~ARCH_MMU_FLAG_PERM_WRITE;
      }

      zx_status_t status =
          coalescer.AppendOrAdjustMapping(va + offset, result->page->paddr(), curr_mmu_flags);
      if (status != ZX_OK) {
        // Flush any existing pages in the coalescer.
        coalescer.Flush();
        return {status, coalescer.TotalMapped()};
      }
    }

    // Fault opportunistic pages. If a range is supplied, it is assumed the user knows the
    // appropriate range, so opportunistic pages will not be fault.
    if (additional_pages == 0) {
      DEBUG_ASSERT(num_fault_pages > 0);
      // Check how much space the coalescer has for faulting additional pages.
      size_t extra_pages = coalescer.ExtraPageCapacityFrom(va + kPageSize);
      extra_pages = ktl::min(extra_pages, num_fault_pages - 1);

      // Acquire any additional pages, but only if they already exist as the user has not
      // attempted to use these pages yet.
      if (extra_pages > 0) {
        bool writeable = (coalescer.GetMmuFlags() & ARCH_MMU_FLAG_PERM_WRITE);
        size_t num_extra_pages =
            cursor->IfExistPages(writeable, extra_pages, coalescer.GetNextPageSlot());
        coalescer.IncrementCount(num_extra_pages);
      }
    }
  }
  if constexpr (!is_paged) {
    // Already validated the size, and since physical VMOs are always allocated, and not
    // resizable, we know we can always retrieve the maximum number of pages without failure.
    uint64_t phys_len = num_fault_pages * kPageSize;
    paddr_t phys_base = 0;
    zx_status_t status = object->LookupContiguousLocked(vmo_offset, phys_len, &phys_base);

    ASSERT(status == ZX_OK);

    // Extrapolate the pages from the base address.
    for (size_t offset = 0; offset < phys_len; offset += kPageSize) {
      status = coalescer.Append(va + offset, phys_base + offset);
      if (status != ZX_OK) {
        return {status, coalescer.TotalMapped()};
      }
    }
  }
  zx_status_t status = coalescer.Flush();
  if (status == ZX_OK) {
    // Mapping has been successfully updated by us. Inform the faulting helper so that it knows
    // not to unmap the range instead.
    currently_faulting.MappingUpdated();
  }
  return {status, coalescer.TotalMapped()};
}

ktl::pair<zx_status_t, uint32_t> VmMapping::PageFault(vaddr_t va, const uint pf_flags,
                                                      const size_t additional_pages,
                                                      VmObject* object,
                                                      MultiPageRequest* page_request) {
  VM_KTRACE_DURATION(2, "VmMapping::PageFault", ("user_id", object->user_id()),
                     ("va", ktrace::Pointer{va}));
  canary_.Assert();

  DEBUG_ASSERT(IsPageRounded(va));

  if (VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object); likely(paged)) {
    __UNINITIALIZED VmCowPages::DeferredOps deferred(paged->MakeDeferredOps());
    Guard<CriticalMutex> guard{AssertOrderedLock, paged->lock(), paged->lock_order()};
    if (object_reset_) {
      return {ZX_ERR_UNAVAILABLE, 0};
    }
    // The caller was obliged pass us the value of |object_| in as |object|, whose lock we now
    // hold. Since we know that object_ can only hold one of two values, |object| or |nullptr| then
    // if object_reset_ is false, i.e. |object_| is still equal to |object|, then we know that:
    //  * Our read of object_reset_ did not race, since it is written under object_->lock(), which
    //    we presently hold
    //  * object_ == object since it's not null
    //  * object_ cannot transition to null since we hold its lock.
    assert_object_lock();
    return PageFaultLockedObject(va, pf_flags, additional_pages, paged, &deferred, page_request);
  }
  VmObjectPhysical* phys = DownCastVmObject<VmObjectPhysical>(object);
  ASSERT(phys);
  Guard<CriticalMutex> guard{phys->lock()};
  if (object_reset_) {
    return {ZX_ERR_UNAVAILABLE, 0};
  }
  // See comment in paged case for explanation.
  assert_object_lock();
  return PageFaultLockedObject(va, pf_flags, additional_pages, phys, nullptr, page_request);
}

ktl::pair<zx_status_t, uint32_t> VmMapping::PageFaultLocked(vaddr_t va, const uint pf_flags,
                                                            const size_t additional_pages,
                                                            MultiPageRequest* page_request) {
  // As the aspace lock is held we can safely just use the direct raw value of object_, knowing that
  // it cannot be destructed, and call the regular PageFault method. This is explicitly safe to call
  // with the aspace lock held.
  return PageFault(va, pf_flags, additional_pages, object_.get(), page_request);
}

zx_status_t VmMapping::ActivateLocked(ActivateInsertRegions insert_region) {
  DEBUG_ASSERT(state_ == LifeCycleState::NOT_READY);
  DEBUG_ASSERT(parent_);

  AssertHeld(parent_->lock_ref());
  AssertHeld(parent_->region_lock_ref());
  // If inserting into the regions attempt that first before modifying any state, as it can fail.
  if (insert_region == ActivateInsertRegions::Yes) {
    zx_status_t status =
        parent_->subregions_.InsertRegion(fbl::RefPtr<VmAddressRegionOrMapping>(this));
    if (status != ZX_OK) {
      return status;
    }
    status = object_->AddMappingLocked(this);
    if (status != ZX_OK) {
      parent_->subregions_.RemoveRegion(this);
      return status;
    }
  }

  state_ = LifeCycleState::ALIVE;

  // Now that we have added a mapping to the VMO it's cache policy becomes fixed, and we can read it
  // and augment our arch_mmu_flags.
  arch_mmu_flags_t cache_policy = object_->GetMappingCachePolicyLocked();
  arch_mmu_flags_t arch_mmu_flags = first_region_arch_mmu_flags_;
  if ((arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK) != cache_policy) {
    // Warn in the event that we somehow receive a VMO that has a cache
    // policy set while also holding cache policy flags within the arch
    // flags. The only path that should be able to achieve this is if
    // something in the kernel maps into their aspace incorrectly.
    if ((arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK) != 0) {
      TRACEF(
          "warning: mapping has conflicting cache policies: vmo %#02x "
          "arch_mmu_flags %#02x.\n",
          cache_policy, arch_mmu_flags & ARCH_MMU_FLAG_CACHE_MASK);
      // Clear the existing cache policy and use the new one.
      arch_mmu_flags &= ~ARCH_MMU_FLAG_CACHE_MASK;
    }
    // If we are changing the cache policy then this can only happen if this is a new mapping region
    // and not a new mapping occurring as a result of an unmap split. In the case of a new mapping
    // region we know there cannot yet be any protection ranges.
    DEBUG_ASSERT(rest_protection_ranges_.is_empty());
    arch_mmu_flags |= cache_policy;
    first_region_arch_mmu_flags_ = arch_mmu_flags;
  }
  return ZX_OK;
}

zx_status_t VmMapping::Activate() {
  Guard<CriticalMutex> guard{object_->lock()};
  return ActivateLocked(ActivateInsertRegions::Yes);
}

fbl::RefPtr<VmMapping> VmMapping::TryMergeRightNeighborLocked(VmMapping* right_candidate)
    TA_NO_THREAD_SAFETY_ANALYSIS {
  // Take a reference to ourself, so that we do not get destructed if we drop the last reference to
  // ourself due to erasing from our parent.
  fbl::RefPtr<VmMapping> self(this);

  AssertHeld(right_candidate->lock_ref());
  AssertHeld(right_candidate->region_lock_ref());

  // This code is tolerant of many 'miss calls' if mappings aren't mergeable or are not neighbours
  // etc, but the caller should not be attempting to merge if these mappings are not actually from
  // the same vmar parent. Doing so indicates something structurally wrong with the hierarchy.
  DEBUG_ASSERT(parent_ == right_candidate->parent_);

  // Should not be able to have the same parent yet have gotten a different memory priority.
  DEBUG_ASSERT(memory_priority_ == right_candidate->memory_priority_);

  // These tests are intended to be ordered such that we fail as fast as possible. As such testing
  // for mergeability, which we commonly expect to succeed and not fail, is done last.

  // Need to refer to the same object.
  if (object_.get() != right_candidate->object_.get()) {
    return nullptr;
  }
  DEBUG_ASSERT(private_clone_ == right_candidate->private_clone_);
  // Aspace and VMO ranges need to be contiguous. Validate that the right candidate is actually to
  // the right in addition to checking that base+size lines up for single scenario where base_+size_
  // can overflow and becomes zero.
  if (base_ + size_ != right_candidate->base_ || right_candidate->base_ < base_) {
    return nullptr;
  }
  if (object_offset_ + size_ != right_candidate->object_offset_) {
    return nullptr;
  }
  // All flags need to be consistent.
  if (flags_ != right_candidate->flags_) {
    return nullptr;
  }
  // Although we can combine the protect_region_list_rest_ of the two mappings, we require that they
  // be of the same cacheability, as this is an assumption that mapping has a single cacheability
  // type. Since all protection regions have the same cacheability we can check any arbitrary one in
  // each of the mappings. Note that this check is technically redundant, since a VMO can only have
  // one kind of cacheability and we already know this is the same VMO, but some extra paranoia here
  // does not hurt.
  if ((first_region_arch_mmu_flags_ & ARCH_MMU_FLAG_CACHE_MASK) !=
      (right_candidate->first_region_arch_mmu_flags_ & ARCH_MMU_FLAG_CACHE_MASK)) {
    return nullptr;
  }

  // Only merge live mappings.
  if (state_ != LifeCycleState::ALIVE || right_candidate->state_ != LifeCycleState::ALIVE) {
    return nullptr;
  }
  // Both need to be mergeable.
  if (mergeable_ == Mergeable::NO || right_candidate->mergeable_ == Mergeable::NO) {
    return nullptr;
  }

  fbl::AllocChecker ac;
  fbl::RefPtr<VmMapping> new_mapping = fbl::AdoptRef(new (&ac) VmMapping(
      *parent_, private_clone_, base_, size_ + right_candidate->size_, flags_, object_,
      object_offset_, 0, btree::BTree<vaddr_t, arch_mmu_flags_t>(), Mergeable::YES));
  if (!ac.check()) {
    return nullptr;
  }
  AssertHeld(new_mapping->lock_ref());

  const MemoryPriority old_priority = memory_priority_;
  // Although it is somewhat awkward and verbose, we use a lambda here instead of just a subscope to
  // prevent the usages of `AssertHeld` from 'leaking' beyond the actual guard scope.
  const bool failure = [&]() TA_REQ(lock()) TA_REQ(right_candidate->lock()) TA_REQ(
                           new_mapping->lock()) {
    // Although it was safe to read size_ without holding the object lock, we need to acquire it
    // to perform changes.
    Guard<CriticalMutex> guard{AliasedLock, object_->lock(), right_candidate->object_->lock()};

    AssertHeld(new_mapping->object_lock_ref());
    zx_status_t status = new_mapping->object_->AddMappingLocked(new_mapping.get());
    if (status != ZX_OK) {
      return true;
    }

    // Attempt to copy all the protection ranges first as this might fail due to an allocation.
    // If it fails we can still abort with a fairly minor roll-back procedure.
    if (MergeProtectionRangesLocked(*right_candidate) != ZX_OK) {
      new_mapping->object_->RemoveMappingLocked(new_mapping.get());
      return true;
    }

    AssertHeld(region_lock_ref());

    new_mapping->first_region_arch_mmu_flags_ = first_region_arch_mmu_flags_;
    new_mapping->rest_protection_ranges_ = ktl::move(rest_protection_ranges_);

    AssertHeld(right_candidate->region_lock_ref());
    // First destroy the right hand mapping, and remove it from the parent.
    status = right_candidate->DestroyLockedObject(DestroyUnmap::No, DestroyRemoveFromParent::Yes);
    ASSERT(status == ZX_OK);
    AssertHeld(parent_->lock_ref());
    AssertHeld(parent_->region_lock_ref());
    // To avoid ActivateLocked from failing we use ReplaceRegion to swap the left mapping for
    // the new mapping in the subregions_ list. This temporarily results in the subregions_
    // list having overlapping mappings and an unactivated mapping, but as we hold both the main
    // lock and subregion lock over the entire operation this state cannot be observed.
    parent_->subregions_.ReplaceRegion(this, new_mapping);
    status = DestroyLockedObject(DestroyUnmap::No, DestroyRemoveFromParent::No);
    ASSERT(status == ZX_OK);
    AssertHeld(new_mapping->region_lock_ref());
    new_mapping->ActivateNoInsertLocked();
    return false;
  }();
  if (failure) {
    // On failure roll back any protection ranges that might have been inserted. Erase cannot fail.
    ClearProtectionRangeTransitionsLocked(base_ + size_, base_ + size_ + right_candidate->size_);
    return nullptr;
  }

  new_mapping->SetMemoryPriorityLocked(old_priority);

  vm_mappings_merged.Add(1);
  return new_mapping;
}

void VmMapping::TryMergeNeighborsLocked() {
  canary_.Assert();

  // Check that this mapping is mergeable and is currently in the correct lifecycle state.
  if (mergeable_ == Mergeable::NO || state_ != LifeCycleState::ALIVE) {
    return;
  }
  // As a VmMapping if we we are alive we by definition have a parent.
  DEBUG_ASSERT(parent_);

  // We expect there to be a RefPtr to us held beyond the one for the wavl tree ensuring that we
  // cannot trigger our own destructor should we remove ourselves from the hierarchy.
  DEBUG_ASSERT(ref_count_debug() > 1);

  AssertHeld(parent_->lock_ref());
  AssertHeld(parent_->region_lock_ref());

  // Find our two merge candidates.
  fbl::RefPtr<VmMapping> left, right;
  if (auto left_candidate = parent_->subregions_.LeftOf(this); left_candidate.IsValid()) {
    left = (*left_candidate).second->as_vm_mapping();
  }
  if (auto right_candidate = parent_->subregions_.RightOf(this); right_candidate.IsValid()) {
    right = (*right_candidate).second->as_vm_mapping();
  }

  // Attempt to merge with each candidate. Any successful merge will produce a new mapping and
  // invalidate this.
  if (right) {
    right = TryMergeRightNeighborLocked(right.get());
  }
  if (left) {
    // We either merge the left with our result of the right merge, or if that was not successful
    // with |this|.
    AssertHeld(left->lock_ref());
    AssertHeld(left->region_lock_ref());
    left->TryMergeRightNeighborLocked(right ? right.get() : this);
  }
}

void VmMapping::MarkMergeable(fbl::RefPtr<VmMapping> mapping) {
  Guard<CriticalMutex> region_guard{mapping->region_lock()};
  Guard<CriticalMutex> guard{mapping->lock()};
  // Now that we have the lock check this mapping is still alive and we haven't raced with some
  // kind of destruction.
  if (mapping->state_ != LifeCycleState::ALIVE) {
    return;
  }
  // Skip marking any vdso segments mergeable. Although there is currently only one vdso segment and
  // so it would never actually get merged, marking it mergeable is technically incorrect.
  if (mapping->aspace_->vdso_code_mapping_ == mapping) {
    return;
  }
  mapping->mergeable_ = Mergeable::YES;
  mapping->TryMergeNeighborsLocked();
}

template <bool SplitOnUnmap>
void VmMapping::SetMemoryPriorityLocked(VmAddressRegion::MemoryPriority priority) {
  if constexpr (SplitOnUnmap) {
    // all that's required to set our priority is to have object_ and aspace_ set up
    DEBUG_ASSERT(state_locked() == LifeCycleState::NOT_READY && object_ && aspace_);
  } else {
    DEBUG_ASSERT(state_locked() == LifeCycleState::ALIVE);
  }
  const bool to_high = priority == VmAddressRegion::MemoryPriority::HIGH;
  const int64_t delta = to_high ? 1 : -1;
  if (priority == memory_priority_) {
    return;
  }
  memory_priority_ = priority;
  aspace_->ChangeHighPriorityCountLocked(delta);
  if (VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object_.get()); paged) {
    PriorityChanger pc = paged->MakePriorityChanger(delta);
    if (priority == VmAddressRegion::MemoryPriority::HIGH) {
      pc.PrepareMayNotAlreadyBeHighPriority();
    }
    Guard<CriticalMutex> guard{AliasedLock, object_->lock(), pc.lock()};
    pc.ChangeHighPriorityCountLocked();
  }
}

template <bool SplitOnUnmap>
void VmMapping::SetMemoryPriorityDefaultLockedObject() {
  if constexpr (SplitOnUnmap) {
    // all that's required to set our priority is to have object_ and aspace_ set up
    DEBUG_ASSERT(state_locked() == LifeCycleState::NOT_READY && object_ && aspace_);
  } else {
    DEBUG_ASSERT(state_locked() == LifeCycleState::ALIVE);
  }
  if (memory_priority_ == VmAddressRegion::MemoryPriority::DEFAULT) {
    return;
  }
  memory_priority_ = VmAddressRegion::MemoryPriority::DEFAULT;
  aspace_->ChangeHighPriorityCountLocked(-1);
  if (VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object_.get()); paged) {
    PriorityChanger pc = paged->MakePriorityChanger(-1);
    AssertHeld(pc.lock_ref());  // we have the object lock
    pc.ChangeHighPriorityCountLocked();
  }
}

template <bool SplitOnUnmap>
void VmMapping::SetMemoryPriorityHighAlreadyPositiveLockedObject() {
  if constexpr (SplitOnUnmap) {
    // all that's required to set our priority is to have object_ and aspace_ set up
    DEBUG_ASSERT(state_locked() == LifeCycleState::NOT_READY && object_ && aspace_);
  } else {
    DEBUG_ASSERT(state_locked() == LifeCycleState::ALIVE);
  }
  if (memory_priority_ == VmAddressRegion::MemoryPriority::HIGH) {
    return;
  }
  memory_priority_ = VmAddressRegion::MemoryPriority::HIGH;
  aspace_->ChangeHighPriorityCountLocked(1);
  if (VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object_.get()); paged) {
    PriorityChanger pc = paged->MakePriorityChanger(1);
    AssertHeld(pc.lock_ref());  // we have the object lock
    pc.PrepareIsAlreadyHighPriorityLocked();
    pc.ChangeHighPriorityCountLocked();
  }
}

void VmMapping::CommitHighMemoryPriority() {
  fbl::RefPtr<VmObject> vmo;
  uint64_t offset;
  uint64_t len;
  {
    Guard<CriticalMutex> guard{lock()};
    if (state_locked() != LifeCycleState::ALIVE || memory_priority_ != MemoryPriority::HIGH) {
      return;
    }
    vmo = object_;
    offset = object_offset_;
    len = size();
  }
  DEBUG_ASSERT(vmo);
  vmo->CommitHighPriorityPages(offset, len);
  // Ignore the return result of MapRange as this is just best effort.
  MapRange(offset, len, false, true);
}

zx::result<fbl::RefPtr<VmMapping>> VmMapping::ForceWritable() {
  canary_.Assert();
  // Take a ref to ourselves in case we drop the last one when removing from our parent.
  fbl::RefPtr<VmMapping> self(this);
  Guard<CriticalMutex> region_guard{region_lock()};
  Guard<CriticalMutex> guard{lock()};
  if (state_locked() != LifeCycleState::ALIVE) {
    return zx::error{ZX_ERR_BAD_STATE};
  }
  DEBUG_ASSERT(object_);
  DEBUG_ASSERT(parent_);

  // Never allow writes to the vdso.
  if (aspace_->vdso_code_mapping_.get() == this) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  // If we have already re-directed to a private clone then there is no need to do so again.
  if (private_clone_) {
    return zx::ok(ktl::move(self));
  }
  // If the mapping is already possible to write to (even if disabled by current protections), then
  // writing is already safe.
  if (is_valid_mapping_flags(ARCH_MMU_FLAG_PERM_WRITE)) {
    return zx::ok(ktl::move(self));
  }
  // A physical VMO cannot be cloned and so we cannot make this safe, just allow the write.
  if (!object_->is_paged()) {
    return zx::ok(ktl::move(self));
  }

  // Create a clone of our VMO that covers the size of our mapping.
  fbl::RefPtr<VmMapping> writable;
  {
    fbl::RefPtr<VmObject> clone;
    zx_status_t status = object_->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite,
                                              object_offset_, size_, true, &clone);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    if (flags_ & VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE) {
      VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object_.get());
      DEBUG_ASSERT(paged);
      uint64_t original_stream_size;
      {
        Guard<CriticalMutex> object_guard{AliasedLock, paged->lock(), object_lock()};
        ktl::optional<uint64_t> stream_size = paged->user_stream_size_locked();
        DEBUG_ASSERT(stream_size.has_value());
        original_stream_size = *stream_size;
      }
      const uint64_t stream_size_over_offset =
          ktl::max(original_stream_size, object_offset_) - object_offset_;
      const uint64_t stream_size_limited_by_map_size = ktl::min(stream_size_over_offset, size_);
      zx::result<fbl::RefPtr<StreamSizeManager>> result =
          StreamSizeManager::Create(stream_size_limited_by_map_size);
      if (result.is_error()) {
        return zx::error(result.error_value());
      }
      clone->SetUserStreamSize(ktl::move(*result));
    }
    // TODO(https://fxbug.dev/503042881) Support a more efficient deep copy.
    btree::BTree<vaddr_t, arch_mmu_flags_t> protection_ranges;
    for (auto [key, value] : rest_protection_ranges_locked()) {
      auto result = protection_ranges.insert(key, value);
      if (!result) {
        return zx::error(ZX_ERR_NO_MEMORY);
      }
    }

    fbl::AllocChecker ac;
    // We created the clone starting at object_offset_ in the old object, so that makes the
    // equivalent start object_offset_ be 0 in the clone.
    writable = fbl::AdoptRef(new (&ac) VmMapping(
        *parent_, true, base_, size_, flags_, ktl::move(clone), 0,
        first_region_arch_mmu_flags_locked(), ktl::move(protection_ranges), mergeable_));
    if (!ac.check()) {
      return zx::error(ZX_ERR_NO_MEMORY);
    }
    // First transfer any memory priority from the current mapping to the new mapping.
    AssertHeld(writable->lock_ref());
    AssertHeld(writable->region_lock_ref());
    // Use SplitOnUnmap=true because writable hasn't been activated yet.
    writable->SetMemoryPriorityLocked</*SplitOnUnmap=*/true>(memory_priority_);

    Guard<CriticalMutex> object_guard{writable->object_lock()};
    status = writable->object_->AddMappingLocked(writable.get());
    if (status != ZX_OK) {
      writable->SetMemoryPriorityDefaultLockedObject</*SplitOnUnmap=*/true>();
      return zx::error(status);
    }

    AssertHeld(parent_->lock_ref());
    AssertHeld(parent_->region_lock_ref());
    parent_->subregions_.ReplaceRegion(this, writable);
    writable->ActivateNoInsertLocked();
  }
  // Now acquire the original object lock and destroy ourself.
  {
    // Keep a refptr to the object_ so we know our lock remains valid.
    fbl::RefPtr<VmObject> object(object_);
    Guard<CriticalMutex> object_guard{object_lock()};
    zx_status_t status = DestroyLockedObject(DestroyUnmap::Yes, DestroyRemoveFromParent::No);
    ASSERT(status == ZX_OK);
  }
  return zx::ok(ktl::move(writable));
}

uint64_t VmMapping::TrimmedObjectRangeLocked(uint64_t offset, uint64_t len) const TA_REQ(lock())
    TA_REQ(object_->lock()) {
  const uint64_t vmo_offset = object_offset_ + offset;
  const uint64_t vmo_size = object_->size_locked();
  if (vmo_offset >= vmo_size) {
    return 0;
  }

  uint64_t trim_len = vmo_size - vmo_offset;

  if (flags_ & VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE) {
    VmObjectPaged* paged = DownCastVmObject<VmObjectPaged>(object_.get());
    DEBUG_ASSERT(paged);
    AssertHeld(paged->lock_ref());
    auto stream_size_res = paged->saturating_stream_size_locked();
    // Creating a fault-beyond-stream-size mapping should have allocated a SSM.
    DEBUG_ASSERT(stream_size_res);
    size_t stream_size = stream_size_res.value();
    DEBUG_ASSERT(stream_size <= vmo_size);
    if (vmo_offset >= stream_size) {
      return 0;
    }
    trim_len = stream_size - vmo_offset;
  }

  return ktl::min(trim_len, len);
}

extern "C" {
FFI_ALWAYS_INLINE fbl::RefCounted<VmAddressRegionOrMapping>* cpp_vm_mapping_get_ref_counted(
    VmMapping* mapping);
void cpp_vm_mapping_free(VmMapping* mapping);
zx_status_t cpp_vm_mapping_destroy(VmMapping* mapping);
const fbl::RefPtr<VmAspace>* cpp_vm_mapping_aspace(VmMapping* mapping);
vaddr_t cpp_vm_mapping_base(VmMapping* mapping);
size_t cpp_vm_mapping_size(VmMapping* mapping);
uint32_t cpp_vm_mapping_flags(VmMapping* mapping);
uint64_t cpp_vm_mapping_object_offset(VmMapping* mapping);
zx_status_t cpp_vm_mapping_decommit_range(VmMapping* mapping, size_t offset, size_t len);
zx_status_t cpp_vm_mapping_map_range(VmMapping* mapping, size_t offset, size_t len, bool commit,
                                     bool ignore_existing);
zx_status_t cpp_vm_mapping_debug_unmap(VmMapping* mapping, vaddr_t base, size_t size);
zx_status_t cpp_vm_mapping_debug_protect(VmMapping* mapping, vaddr_t base, size_t size,
                                         arch_mmu_flags_t new_arch_mmu_flags);
const VmObject* cpp_vm_mapping_vmo(VmMapping* mapping);
zx_status_t cpp_vm_mapping_force_writable(VmMapping* mapping, VmMapping** out_mapping);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE fbl::RefCounted<VmAddressRegionOrMapping>* cpp_vm_mapping_get_ref_counted(
    VmMapping* mapping) {
  return mapping;
}
void cpp_vm_mapping_free(VmMapping* mapping) { delete mapping; }
zx_status_t cpp_vm_mapping_destroy(VmMapping* mapping) { return mapping->Destroy(); }
const fbl::RefPtr<VmAspace>* cpp_vm_mapping_aspace(VmMapping* mapping) {
  return &mapping->aspace();
}
vaddr_t cpp_vm_mapping_base(VmMapping* mapping) { return mapping->base(); }
size_t cpp_vm_mapping_size(VmMapping* mapping) { return mapping->size(); }
uint32_t cpp_vm_mapping_flags(VmMapping* mapping) { return mapping->flags(); }
uint64_t cpp_vm_mapping_object_offset(VmMapping* mapping) { return mapping->object_offset(); }
zx_status_t cpp_vm_mapping_decommit_range(VmMapping* mapping, size_t offset, size_t len) {
  return mapping->DecommitRange(offset, len);
}
zx_status_t cpp_vm_mapping_map_range(VmMapping* mapping, size_t offset, size_t len, bool commit,
                                     bool ignore_existing) {
  return mapping->MapRange(offset, len, commit, ignore_existing);
}
zx_status_t cpp_vm_mapping_debug_unmap(VmMapping* mapping, vaddr_t base, size_t size) {
  return mapping->DebugUnmap(base, size);
}
zx_status_t cpp_vm_mapping_debug_protect(VmMapping* mapping, vaddr_t base, size_t size,
                                         arch_mmu_flags_t new_arch_mmu_flags) {
  return mapping->DebugProtect(base, size, new_arch_mmu_flags);
}
const VmObject* cpp_vm_mapping_vmo(VmMapping* mapping) {
  fbl::RefPtr<VmObject> vmo = mapping->vmo();
  return fbl::ExportToRawPtr(&vmo);
}
zx_status_t cpp_vm_mapping_force_writable(VmMapping* mapping, VmMapping** out_mapping) {
  auto result = mapping->ForceWritable();
  if (result.is_error()) {
    return result.error_value();
  }
  auto res = *result;
  *out_mapping = fbl::ExportToRawPtr(&res);
  return ZX_OK;
}
}
