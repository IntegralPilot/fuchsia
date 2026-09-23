// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_BUFFERS_UTIL_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_BUFFERS_UTIL_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/flatland/buffers/buffer_collection.h"

namespace flatland {

fuchsia_sysmem2::BufferUsage get_none_usage();

struct SysmemTokens {
  // Token for setting client side constraints.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> local_token;

  // Token for setting server side constraints.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> dup_token;

  static SysmemTokens Create(fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator) {
    auto [local_token_client, local_token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->AllocateSharedCollection(
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
            .token_request(std::move(local_token_server))
            .Build());
    FX_DCHECK(result.ok());

    auto [dup_token_client, dup_token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    result =
        fidl::WireCall(local_token_client)
            ->Duplicate(fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateRequest::Builder(arena)
                            .rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS)
                            .token_request(std::move(dup_token_server))
                            .Build());
    FX_DCHECK(result.ok());

    auto sync_result = fidl::WireCall(local_token_client)->Sync();
    FX_DCHECK(sync_result.ok());

    return {
        .local_token = std::move(local_token_client),
        .dup_token = std::move(dup_token_client),
    };
  }
};

// TODO(https://fxbug.dev/42132796): The default memory constraints set by Sysmem only allows using
// CPU domain for buffers with CPU usage, while Mali driver asks for only
// RAM and Inaccessible domains for buffer allocation, which caused failure in
// sysmem allocation. So here we add RAM domain support to clients in order
// to get buffer allocated correctly.
const std::pair<fuchsia_sysmem2::BufferUsage, fuchsia_sysmem2::BufferMemoryConstraints>
GetUsageAndMemoryConstraintsForCpuWriteOften();

// Sets the client constraints on a sysmem buffer collection, including the number of images,
// the dimensionality (width, height) of those images, the usage and memory constraints. This
// is a blocking function that will wait until the constraints have been fully set.
void SetClientConstraintsAndWaitForAllocated(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, uint32_t image_count = 1,
    uint32_t width = 64, uint32_t height = 32,
    fuchsia_sysmem2::BufferUsage usage = get_none_usage(),
    const std::vector<fuchsia_images2::PixelFormatModifier>& additional_format_modifiers = {},
    std::optional<fuchsia_sysmem2::BufferMemoryConstraints> memory_constraints = std::nullopt);

// Sets the constraints on a client buffer collection pointer and returns that pointer back to
// the caller, *without* waiting for the constraint setting to finish. It is up to the caller
// to wait until constraints are set.
fidl::SyncClient<fuchsia_sysmem2::BufferCollection> CreateBufferCollectionSyncPtrAndSetConstraints(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, uint32_t image_count = 1,
    uint32_t width = 64, uint32_t height = 32,
    fuchsia_sysmem2::BufferUsage usage = get_none_usage(),
    fuchsia_images2::PixelFormat pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8,
    std::optional<fuchsia_sysmem2::BufferMemoryConstraints> memory_constraints = std::nullopt,
    std::optional<fuchsia_images2::PixelFormatModifier> pixel_format_modifier = std::nullopt);

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

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_BUFFERS_UTIL_H_
