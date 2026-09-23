// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/riscv64/user_copy.h>
#include <arch/user_copy.h>

// FFI wrappers to help marshall riscv64 specific return args into UserCopyCaptureFaultsResult
// which contain a std::optional that is currently not possible to get across the language
// barrier.
UserCopyCaptureFaultsResult arch_copy_from_user_capture_faults(void* dst, const void* src,
                                                               size_t len, CopyContext context) {
  Riscv64UserCopyRet ret = rust_arch_copy_from_user_capture_faults(dst, src, len);
  if (ret.status == ZX_OK) {
    return UserCopyCaptureFaultsResult{ZX_OK};
  }
  // The rust copy routine returns OUT_OF_RANGE if the src/len is outside of user space.
  // The UserCopyCaptureFaultsResult contract still specifies ERR_INVALID_ARGS for both
  // cases, so convert it back to an ERR_INVALID_ARG but with the std::optional<FaultInfo>
  // unset.
  if (ret.status == ZX_ERR_OUT_OF_RANGE) {
    return UserCopyCaptureFaultsResult{ZX_ERR_INVALID_ARGS};
  }
  return {ret.status, {ret.pf_va, ret.pf_flags}};
}

UserCopyCaptureFaultsResult arch_copy_to_user_capture_faults(void* dst, const void* src, size_t len,
                                                             CopyContext context) {
  Riscv64UserCopyRet ret = rust_arch_copy_to_user_capture_faults(dst, src, len);
  if (ret.status == ZX_OK) {
    return UserCopyCaptureFaultsResult{ZX_OK};
  }
  // The rust copy routine returns OUT_OF_RANGE if the dst/len is outside of user space.
  // The UserCopyCaptureFaultsResult contract still specifies ERR_INVALID_ARGS for both
  // cases, so convert it back to an ERR_INVALID_ARG but with the std::optional<FaultInfo>
  // unset.
  if (ret.status == ZX_ERR_OUT_OF_RANGE) {
    return UserCopyCaptureFaultsResult{ZX_ERR_INVALID_ARGS};
  }
  return {ret.status, {ret.pf_va, ret.pf_flags}};
}
