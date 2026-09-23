// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_TESTING_UTIL_BUFFERS_HELPER_H_
#define SRC_UI_TESTING_UTIL_BUFFERS_HELPER_H_

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>

namespace ui_testing {

enum class HostPointerAccessMode : uint32_t {
  kReadOnly = 0b01,
  kWriteOnly = 0b10,
  kReadWrite = 0b11,
};

// Maps a sysmem vmo's bytes into host memory that can be accessed via a callback function. The
// callback provides the caller with a raw pointer to the vmo memory as well as an int for the
// number of bytes. If an out of bounds vmo_idx is provided, the callback function will call the
// user callback with mapped_ptr equal to nullptr. Once the callback function returns, the host
// pointer is unmapped and so cannot continue to be used outside of the scope of the callback.
void MapHostPointer(const fuchsia::sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t* mapped_ptr, uint32_t num_bytes)> callback);

// Maps a sysmem vmo's bytes into host memory that can be accessed via a callback function. The
// callback provides the caller with a raw pointer to the vmo memory as well as an int for the
// number of bytes. If an out of bounds vmo_idx is provided, the callback function will call the
// user callback with mapped_ptr equal to nullptr. Once the callback function returns, the host
// pointer is unmapped and so cannot continue to be used outside of the scope of the callback.
void MapHostPointer(const fuchsia_sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t* mapped_ptr, uint32_t num_bytes)> callback);

// Maps a given vmo's bytes into host memory that can be accessed via a callback function. The
// callback provides the caller with a raw pointer to the vmo memory as well as an int for the
// number of bytes. Once the callback function returns, the host
// pointer is unmapped and so cannot continue to be used outside of the scope of the callback.
void MapHostPointer(const zx::vmo& vmo, HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t* mapped_ptr, uint32_t num_bytes)> callback,
                    uint64_t vmo_bytes = 0);

}  // namespace ui_testing

#endif  // SRC_UI_TESTING_UTIL_BUFFERS_HELPER_H_
