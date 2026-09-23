// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SHAREABLE_PROCESS_STATE_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SHAREABLE_PROCESS_STATE_H_

#include <fbl/ref_counted.h>
#include <kernel/mutex.h>
#include <ktl/atomic.h>
#include <object/futex_context.h>
#include <object/handle_table.h>
#include <vm/vm_aspace.h>

// This class is logically private to ProcessDispatcher.
//
// |ShareableProcessState| contains all the state that can belong to more than one process.
//
// This class is logically private to ProcessDispatcher.
//
// The objects contained in this class have lifetimes that are decoupled from their lifecycle.
//
// A ShareableProcessState is always constructed with a process count of 1, meaning that the creator
// should issues a matching |DecrementShareCount| before the |ShareableProcessState| is destroyed.
class ShareableProcessState : public fbl::RefCounted<ShareableProcessState> {
 public:
  ShareableProcessState(const ShareableProcessState&) = delete;
  ShareableProcessState& operator=(const ShareableProcessState&) = delete;

  ShareableProcessState() = default;
  ~ShareableProcessState() {
    DEBUG_ASSERT(!aspace_ || aspace_->is_destroyed());
    DEBUG_ASSERT(process_count_.load(ktl::memory_order_relaxed) == 0);
  }

  // Shares this state with a process, effectively incrementing the number of calls to
  // |RemoveFromProcess| that can be made before the shared resources are cleaned up.
  //
  // Returns whether or not the share count was incremented successfully. Can fail if the process
  // state has been destroyed.
  bool IncrementShareCount() {
    uint64_t prev = process_count_.load(ktl::memory_order_relaxed);
    do {
      if (prev == 0) {
        return false;
      }
    } while (!process_count_.compare_exchange_weak(prev, prev + 1, ktl::memory_order_relaxed));
    return true;
  }

  // Removes this state from a process. If the state is not shared with any other processes, the
  // shared resources are cleaned.
  void DecrementShareCount() {
    DEBUG_ASSERT(process_count_ > 0);

    const uint64_t prev = process_count_.fetch_sub(1, ktl::memory_order_relaxed);

    if (prev > 1) {
      return;
    }

    handle_table_.Clean();
    if (aspace_) {
      zx_status_t result = aspace_->Destroy();
      ASSERT_MSG(result == ZX_OK, "%d\n", result);
    }
  }

  // Initializes the shared state.
  //
  // It is an error to call initialize on a shared state that has already been initialized, or one
  // that has been destroyed.
  bool Initialize(vaddr_t aspace_base, vaddr_t aspace_size, const char* aspace_name,
                  VmAspace::ShareOpt share_opt) {
    DEBUG_ASSERT(!aspace_);
    DEBUG_ASSERT(process_count_.load(ktl::memory_order_relaxed) == 1);
    aspace_ =
        VmAspace::Create(aspace_base, aspace_size, VmAspace::Type::User, aspace_name, share_opt);
    if (aspace_ == nullptr) {
      return false;
    }

    DEBUG_ASSERT_MSG(
        share_opt == VmAspace::ShareOpt::None || share_opt == VmAspace::ShareOpt::Shared,
        "unknown share_opt %u", static_cast<uint32_t>(share_opt));
    const FutexContext::ProcessType type = share_opt == VmAspace::ShareOpt::None
                                               ? FutexContext::ProcessType::Regular
                                               : FutexContext::ProcessType::Shared;
    zx_status_t status = futex_context_.Init(type);
    return status == ZX_OK;
  }

  uint64_t share_count() const { return process_count_.load(ktl::memory_order_relaxed); }

  HandleTable& handle_table() { return handle_table_; }
  const HandleTable& handle_table() const { return handle_table_; }

  FutexContext& futex_context() { return futex_context_; }
  const FutexContext& futex_context() const { return futex_context_; }

  VmAspace* aspace() { return aspace_.get(); }
  const VmAspace* aspace() const { return aspace_.get(); }

 private:
  // The layout of the fields below is intended to eliminating padding.
  //
  // The number of processes currently sharing this state.
  HandleTable handle_table_;
  ktl::atomic<uint64_t> process_count_ = 1;
  // This field is logically const and may not be changed after initialization.  Resetting or
  // assigning to this field post-initialization is a programming error.
  fbl::RefPtr<VmAspace> aspace_;
  FutexContext futex_context_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SHAREABLE_PROCESS_STATE_H_
