// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_INSPECTOR_GWP_ASAN_H_
#define ZIRCON_SYSTEM_ULIB_INSPECTOR_GWP_ASAN_H_

#include <lib/fit/result.h>
#include <lib/zx/process.h>
#include <zircon/syscalls/exception.h>

#include <array>
#include <variant>
#include <vector>

namespace inspector {

struct GwpAsanInfo {
  // Human-readable string about the error. nullptr means there's no GWP-ASan error.
  const char* error_type = nullptr;
  // The access address that causes the exception.
  uintptr_t faulting_addr = 0;
  // The address of the allocation.
  uintptr_t allocation_address;
  // The size of the allocation.
  size_t allocation_size;
  // The allocation trace if there's an error.
  std::vector<uintptr_t> allocation_trace;
  // The free trace if there's an error and the allocation is freed.
  std::vector<uintptr_t> deallocation_trace;
};

/// Below are structs that can be returned from `inspector_get_gwp_asan_info`
/// via the `fit::result`. Each one contains relevant error information
/// whenever `inspector_get_gwp_asan_info` is unable to retrieve GWP-ASan
/// information.

// Returned when the GWP-ASan info address (__libc_gwp_asan_info symbol or ELF note)
// could not be found in libc.so.
struct GwpAsanInfoAddressNotFound {};

// Returned when reading the LibcGwpAsanInfo struct from the process memory failed.
struct LibcGwpAsanInfoReadFailed {
  // The Zircon status returned by the memory read operation.
  zx_status_t status;
  // The target virtual address (__libc_gwp_asan_info) in the process we attempted to read.
  uintptr_t libc_gwp_asan_info_addr;
  // The actual number of bytes read (which was not equal to the expected size).
  size_t actual_size;
};

// Returned when reading the AllocatorState struct from the process memory failed.
struct AllocatorStateReadFailed {
  // The Zircon status returned by the memory read operation.
  zx_status_t status;
  // The target virtual address in the process we attempted to read.
  uintptr_t address;
  // The actual number of bytes read (which was not equal to the expected size).
  size_t actual_size;
};

// Returned when GWP-ASan validation checks failed (e.g. magic number or version mismatch,
// or max allocations is zero).
struct ValidationFailed {
  // The GWP-ASan magic bytes as a 4-byte array.
  std::array<uint8_t, 4> magic;
  // The GWP-ASan allocator state version.
  uint16_t version;
  // The maximum number of simultaneous allocations.
  size_t max_allocations;
};

// Returned when reading the AllocationMetadata array from the process memory failed.
struct MetadataReadFailed {
  // The Zircon status returned by the memory read operation.
  zx_status_t status;
  // The target virtual address in the process we attempted to read.
  uintptr_t address;
  // The expected size to read in bytes.
  size_t expected_size;
  // The actual number of bytes read.
  size_t actual_size;
};

// Returned when the faulting address could not be mapped back to any GWP-ASan
// allocation metadata slot.
struct MetadataMappingFailed {
  // The faulting address that caused the crash.
  uintptr_t faulting_addr;
};

using GwpAsanError =
    std::variant<GwpAsanInfoAddressNotFound, LibcGwpAsanInfoReadFailed, AllocatorStateReadFailed,
                 ValidationFailed, MetadataReadFailed, MetadataMappingFailed>;

// Get the GWP-ASan info from the given process and thread.
//
// Returns a fit::result indicating whether the read is successful. If it is success, |info| is
// filled with the appropriate information. If it is failure, possibilities are
//   * the process is not available for read.
//   * there's no libc.so, or no GWP-ASan note in the libc.so.
//   * GWP-ASan is not enabled.
fit::result<GwpAsanError> inspector_get_gwp_asan_info(const zx::process& process,
                                                      const zx_exception_report_t& exception_report,
                                                      GwpAsanInfo* info);

}  // namespace inspector

#endif  // ZIRCON_SYSTEM_ULIB_INSPECTOR_GWP_ASAN_H_
