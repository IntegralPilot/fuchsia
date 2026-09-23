// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "user-copy.h"

#include <lib/user_copy/internal.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <arch/arch_thread.h>
#include <arch/user_copy.h>
#include <kernel/thread.h>
#include <vm/vm.h>

#define ARM64_USER_COPY_CAPTURE_ALL_FAULTS \
  (~((1ull << ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT) | (1ull << ARM64_DFR_RUN_FAULT_HANDLER_BIT)))
#define ARM64_USER_COPY_CAPTURE_PAGE_FAULTS (~(1ull << ARM64_DFR_RUN_FAULT_HANDLER_BIT))
#define ARM64_USER_COPY_DO_FAULTS (~0ull)

namespace {

// Wrapper function to make sure we disable and enable pan around each copy operation
inline Arm64UserCopyRet arm64_user_copy_pan_wrapped(void* dst, const void* src, size_t len,
                                                    uint64_t* fault_return,
                                                    uint64_t fault_return_mask) {
  if (arm64_mmu_features.pan) {
    arm64_disable_pan();
  }
  Arm64UserCopyRet ret = _arm64_user_copy(dst, src, len, fault_return, fault_return_mask);
  if (arm64_mmu_features.pan) {
    arm64_enable_pan();
  }
  return ret;
}

}  // namespace

zx_status_t arch_copy_from_user(void* dst, const void* src, size_t len) {
  DEBUG_ASSERT(!arch_blocking_disallowed());
  DEBUG_ASSERT(arch_num_spinlocks_held() == 0);
  // The copy might trigger a page fault that needs to block on a user pager request, in which case
  // it is not permitted to be holding locks. To ensure this doesn't accidentally work if a copy
  // does not happen to trigger a pager, we insist that all copies do not hold locks. This method is
  // an empty inline function if lockdep is not enabled.
  lockdep::AssertNoLocksHeld();

  // The assembly code just does memcpy with fault handling.  This is
  // the security check that an address from the user is actually a
  // valid userspace address so users can't access kernel memory.
  if (!is_user_accessible_range(reinterpret_cast<vaddr_t>(src), len)) {
    return ZX_ERR_INVALID_ARGS;
  }
  // Spectre V1: Confine {src, len} to user addresses to prevent the kernel from speculatively
  // reading user-controlled addresses.
  internal::validate_user_accessible_range(reinterpret_cast<vaddr_t*>(&src), &len);

  zx_status_t ret =
      arm64_user_copy_pan_wrapped(dst, src, len, &Thread::Current::Get()->arch().data_fault_resume,
                                  ARM64_USER_COPY_DO_FAULTS)
          .status;

  return ret;
}

zx_status_t arch_copy_to_user(void* dst, const void* src, size_t len) {
  DEBUG_ASSERT(!arch_blocking_disallowed());
  DEBUG_ASSERT(arch_num_spinlocks_held() == 0);
  // See comment above.
  lockdep::AssertNoLocksHeld();

  if (!is_user_accessible_range(reinterpret_cast<vaddr_t>(dst), len)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Spectre V1.1: Confine {dst, len} to user addresses to prevent the kernel from speculatively
  // storing to user-controlled addresses.
  internal::validate_user_accessible_range(reinterpret_cast<vaddr_t*>(&dst), &len);

  zx_status_t ret =
      arm64_user_copy_pan_wrapped(dst, src, len, &Thread::Current::Get()->arch().data_fault_resume,
                                  ARM64_USER_COPY_DO_FAULTS)
          .status;

  return ret;
}

UserCopyCaptureFaultsResult arch_copy_from_user_capture_faults(void* dst, const void* src,
                                                               size_t len, CopyContext context) {
  // The assembly code just does memcpy with fault handling.  This is
  // the security check that an address from the user is actually a
  // valid userspace address so users can't access kernel memory.
  if (!is_user_accessible_range(reinterpret_cast<vaddr_t>(src), len)) {
    return UserCopyCaptureFaultsResult{ZX_ERR_INVALID_ARGS};
  }
  // Spectre V1: Confine {src, len} to user addresses to prevent the kernel from speculatively
  // reading user-controlled addresses.
  internal::validate_user_accessible_range(reinterpret_cast<vaddr_t*>(&src), &len);

  // Ensure that the calling thread is either holding no spinlocks, or has correctly specified a
  // non-blocking copy context.
  DEBUG_ASSERT(arch_num_spinlocks_held() == 0 || context == CopyContext::kBlockingNotAllowed);
  uint64_t fault_return_mask = ARM64_USER_COPY_CAPTURE_PAGE_FAULTS;
  if (context == CopyContext::kBlockingNotAllowed) {
    fault_return_mask = ARM64_USER_COPY_CAPTURE_ALL_FAULTS;
  }

  Arm64UserCopyRet ret = arm64_user_copy_pan_wrapped(
      dst, src, len, &Thread::Current::Get()->arch().data_fault_resume, fault_return_mask);

  // A return result of ZX_ERR_INVALID_ARGS means both that a fault occurred and there is valid
  // fault data to resolve it. Any other status is either success (ZX_OK), or a fault without data.
  if (ret.status == ZX_ERR_INVALID_ARGS) {
    return {ret.status, {ret.pf_va, ret.pf_flags}};
  }
  return UserCopyCaptureFaultsResult{ret.status};
}

UserCopyCaptureFaultsResult arch_copy_to_user_capture_faults(void* dst, const void* src, size_t len,
                                                             CopyContext context) {
  if (!is_user_accessible_range(reinterpret_cast<vaddr_t>(dst), len)) {
    return UserCopyCaptureFaultsResult{ZX_ERR_INVALID_ARGS};
  }

  // Spectre V1.1: Confine {dst, len} to user addresses to prevent the kernel from speculatively
  // storing to user-controlled addresses.
  internal::validate_user_accessible_range(reinterpret_cast<vaddr_t*>(&dst), &len);

  // Ensure that the calling thread is either holding no spinlocks, or has correctly specified a
  // non-blocking copy context.
  DEBUG_ASSERT(arch_num_spinlocks_held() == 0 || context == CopyContext::kBlockingNotAllowed);
  uint64_t fault_return_mask = ARM64_USER_COPY_CAPTURE_PAGE_FAULTS;
  if (context == CopyContext::kBlockingNotAllowed) {
    fault_return_mask = ARM64_USER_COPY_CAPTURE_ALL_FAULTS;
  }

  Arm64UserCopyRet ret = arm64_user_copy_pan_wrapped(
      dst, src, len, &Thread::Current::Get()->arch().data_fault_resume, fault_return_mask);

  // A return result of ZX_ERR_INVALID_ARGS means both that a fault occurred and there is valid
  // fault data to resolve it. Any other status is either success (ZX_OK), or a fault without data.
  if (ret.status == ZX_ERR_INVALID_ARGS) {
    return {ret.status, {ret.pf_va, ret.pf_flags}};
  }
  return UserCopyCaptureFaultsResult{ret.status};
}
