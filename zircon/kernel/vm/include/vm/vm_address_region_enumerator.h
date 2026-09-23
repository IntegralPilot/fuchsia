// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_ENUMERATOR_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_ENUMERATOR_H_

#include <assert.h>
#include <stdint.h>
#include <zircon/types.h>

#include <ktl/optional.h>
#include <ktl/type_traits.h>
#include <vm/vm_address_region.h>

enum class VmAddressRegionEnumeratorType : bool {
  // Both vmars and mappings will be yielded in calls to |next|. Only vmars that are wholly
  // contained within the requested range will be yielded, and it is considered an error to
  // knowingly attempt an enumeration that partially overlaps any sub vmars.
  VmarsAndMappings,
  // Only mappings will be yielded in calls to |next|, and there are no restrictions on the kind of
  // range that can be specified.
  MappingsOnly,
};
// Helper class for performing enumeration of a VMAR. Although this is intended to be
// internal, it is declared here for exposing to unit tests.
//
// The purpose of having a stateful enumerator is to have the option to not need to hold the aspace
// lock over the entire enumeration, whilst guaranteeing forward progress and termination. If the
// vmar is modified whilst enumeration is paused (due to dropping the lock or otherwise) then it is
// not well defined whether the enumerator will return any new mappings. However, the enumerator
// will never return DEAD mappings, and will not return mappings in ranges it has already
// enumerated.
//
// Except between calls to |pause| and |resume|, the vmar should be considered immutable, and
// sub-vmars and mappings should not be modified.
template <VmAddressRegionEnumeratorType Type, typename VMAR>
class VmAddressRegionEnumerator {
  static_assert(ktl::is_same_v<ktl::remove_const_t<VMAR>, VmAddressRegion>);
  // Helper to apply any constness of VMAR to the provided type T.
  template <typename T>
  using maybe_const = ktl::conditional_t<ktl::is_const_v<VMAR>, const T, T>;

 public:
  // This requires the vmar lock to be held over the lifetime of the object, except where explicitly
  // stated otherwise.
  VmAddressRegionEnumerator(VMAR& vmar, vaddr_t min_addr, vaddr_t max_addr) TA_REQ(vmar.lock())
      : min_addr_(min_addr),
        max_addr_(max_addr),
        vmar_(vmar),
        itr_(vmar_.subregions_locked().IncludeOrHigher(min_addr_)) {
    if constexpr (Type == VmAddressRegionEnumeratorType::VmarsAndMappings) {
      // If enumerating vmars and mappings validate that, unless the range is empty, that any
      // vmar is wholly contained within the range. This will not catch all errors in users
      // potentially providing a range that only partially overlaps a vmar, but should catch
      // most accidental misuses.
      // This can only be tested here, and not when walking, since due to how pausing and
      // resuming works we may end up with a min_addr_ that is partially in a vmar, either
      // because the vmars were modified while paused or because we need to resume at a
      // mapping part way into a vmar.
      ASSERT(!itr_.IsValid() || (*itr_).second->is_mapping() ||
             ((*itr_).second->base() >= min_addr &&
              (*itr_).second->base() + (*itr_).second->size() <= max_addr));
    }
  }

  struct NextResult {
    maybe_const<VmAddressRegionOrMapping>* region_or_mapping;
    uint depth;
  };

  // Yield the next region or mapping, or a nullopt if enumeration has completed. Regions are
  // yielded in depth-first pre-order. The regions are yielded as raw pointers, which are guaranteed
  // to be valid since the lock is held. It is the callers responsibility to keep these pointers
  // alive, by upgrading to RefPtrs or otherwise, if they want to |pause| and drop the lock.
  ktl::optional<NextResult> next() TA_REQ(vmar_.lock()) {
    ASSERT(!state_.paused_);
    ktl::optional<NextResult> ret = ktl::nullopt;
    while (!ret && itr_.IsValid()) {
      AssertHeld((*itr_).second->lock_ref());
      if ((*itr_).second->base() >= max_addr_)
        break;

      auto curr = itr_++;
      AssertHeld((*curr).second->lock_ref());
      DEBUG_ASSERT((*curr).second->IsAliveLocked());
      maybe_const<VmAddressRegion>* up = (*curr).second->parent_;

      if (auto* mapping = (*curr).second->as_vm_mapping_ptr(); mapping) {
        DEBUG_ASSERT(mapping != nullptr);
        AssertHeld(mapping->lock_ref());
        // If the mapping is entirely before |min_addr|, do not run on_mapping.
        // This can happen when a vmar contains min_addr but has mappings entirely below it.
        // (Base addresses above max_addr_ are strictly handled by the short-circuit earlier in the
        // loop)
        if (mapping->base() < min_addr_ && mapping->base() + mapping->size() <= min_addr_) {
          // Mapping is out of bounds below our range, do not yield it. We do not `continue` here,
          // as we must fall through to the ascending logic below in case this was the last child.
        } else {
          // The const_cast here allows for returning a pointer with the same constness as the
          // originally provided VmAddressRegion, but allowing for the fact that we use a
          // const_iterator for traversal. This is safe since we have a non-const references to the
          // tree, and we are allowed to manipulate the underlying objects, however since we do not
          // hold the region_lock_ we cannot manipulate the subregion_ tree itself, hence we have to
          // use const_iterator.
          ret = NextResult{const_cast<maybe_const<VmMapping*>>(mapping), depth_};
        }

      } else {
        auto* vmar = (*curr).second->as_vm_address_region_ptr();
        DEBUG_ASSERT(vmar != nullptr);
        AssertHeld(vmar->lock_ref());
        // Yield the vmar if its base is greater than or equal to our min. As we only yield vmars if
        // requested, and a condition of it being requested is that iteration not be happening part
        // way into a VMAR, we can simply validate that the vmar base is >= our min.
        if constexpr (Type == VmAddressRegionEnumeratorType::VmarsAndMappings) {
          if (vmar->base() >= min_addr_) {
            ret = NextResult{const_cast<maybe_const<VmAddressRegion*>>(vmar), depth_};
          }
        }
        if (!vmar->subregions_locked().IsEmpty()) {
          // If the sub-VMAR is not empty, iterate through its children.
          // Optimization: skip children entirely below min_addr_.
          auto next_itr = vmar->subregions_locked().IncludeOrHigher(min_addr_);
          if (next_itr.IsValid()) {
            itr_ = next_itr;
            depth_++;
            continue;
          }
          // If next_itr is invalid, all children in this sub-VMAR are < min_addr_.
          // We do not descend. Instead, we fall through to the ascending logic below
          // to correctly advance to the next sibling of this sub-VMAR.
        }
      }
      if (depth_ > kStartDepth && !itr_.IsValid()) {
        AssertHeld(up->lock_ref());
        // If we are at a depth greater than the minimum, and have reached
        // the end of a sub-VMAR range, we ascend and continue iteration.
        do {
          itr_ = up->subregions_locked().UpperBound((*curr).second->base());
          if (itr_.IsValid()) {
            break;
          }
          up = up->parent_;
        } while (depth_-- != kStartDepth);
        if (!itr_.IsValid()) {
          // If we have reached the end after ascending all the way up,
          // break out of the loop.
          break;
        }
      }
    }
    return ret;
  }

  // Pause enumeration. Until |resume| is called |next| may not be called, but the vmar lock is
  // permitted to be dropped, and the vmar is permitted to be modified.
  void pause() TA_REQ(vmar_.lock()) {
    ASSERT(!state_.paused_);
    // Save information of the next iteration we should return.
    if (itr_.IsValid()) {
      AssertHeld((*itr_).second->lock_ref());
      // itr_ represents the next item that needs to be checked / yielded after we |resume|. We
      // know that everything up to itr_ has already been yielded, so if |itr_| becomes invalid
      // between now and |resume| we know the following:
      //  1. If itr_ was a mapping then this mapping was deleted and, even if a new mapping has
      //     since replaced it, we are not obligated to return it. As such we can skip it.
      //  2. If itr_ was a mapping then since a mapping cannot cross a vmar boundary, resuming at
      //     the end of the mapping will not result in missing a vmar.
      //  3. If itr_ was a vmar then it being deleted can only happen if the entire subtree was
      //     deleted. Now, similar to a mapping, even if a new subtree was created we are not
      //     obligated to return it and can skip.
      // For these reasons we prepare our |next_offset_| to be the end of the current |itr_|,
      // meaning that if itr_ is deleted the next object to be yielded is whatever starts after it.
      state_.next_offset_ = (*itr_).second->base() + (*itr_).second->size();
      state_.region_or_mapping_ = fbl::RefPtr<VmAddressRegionOrMapping>(
          const_cast<VmAddressRegionOrMapping*>((*itr_).second));
    } else {
      state_.next_offset_ = max_addr_;
      state_.region_or_mapping_ = nullptr;
    }
    state_.paused_ = true;
  }

  // Resume enumeration allowing |next| to be called again.
  void resume() TA_REQ(vmar_.lock()) {
    ASSERT(state_.paused_);
    if (state_.region_or_mapping_) {
      AssertHeld(state_.region_or_mapping_->lock_ref());
      if (!state_.region_or_mapping_->IsAliveLocked()) {
        // Generate a new iterator that starts at the right offset, but back at the top. The next
        // call to next() will walk back down if necessary to find the next mapping / VMAR.
        min_addr_ = state_.next_offset_;
        itr_ = vmar_.subregions_locked().IncludeOrHigher(min_addr_);
        depth_ = kStartDepth;
      } else {
        ASSERT(state_.region_or_mapping_->parent_);
        AssertHeld(state_.region_or_mapping_->parent_->lock_ref());
        itr_ = state_.region_or_mapping_->parent_->subregions_locked().IncludeOrHigher(
            state_.region_or_mapping_->base());
        DEBUG_ASSERT((*itr_).second == state_.region_or_mapping_.get());
      }
      // Free the refptr. Note that the actual destructors of VmAddressRegionOrMapping objects
      // themselves do very little, so we are safe to potential invoke the destructor here.
      state_.region_or_mapping_.reset();
    } else {
      // There was no refptr, meaning itr_ was already not valid, and should still not be valid.
      ASSERT(!itr_.IsValid());
    }
    state_.paused_ = false;
  }

  // Expose our backing lock for annotation purposes.
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(vmar_.lock_ref()) { return vmar_.lock_ref(); }

 private:
  struct PauseState {
    bool paused_ = false;
    vaddr_t next_offset_ = 0;
    fbl::RefPtr<VmAddressRegionOrMapping> region_or_mapping_;
  };

  PauseState state_;
  vaddr_t min_addr_;
  const vaddr_t max_addr_;
  static constexpr uint kStartDepth = 1;
  uint depth_ = kStartDepth;
  // Root vmar being enumerated.
  VMAR& vmar_;
  // This iterator represents the object at which |next| should use to find the next item to return.
  // An invalid itr_ therefore represents no next object, and means enumeration has finished.
  RegionList<VmAddressRegionOrMapping>::ChildList::const_iterator itr_;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_ADDRESS_REGION_ENUMERATOR_H_
