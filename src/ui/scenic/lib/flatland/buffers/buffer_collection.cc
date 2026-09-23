// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/buffers/buffer_collection.h"

#include <lib/zx/result.h>
#include <zircon/errors.h>

namespace flatland {

BufferCollectionInfo::~BufferCollectionInfo() {
  if (buffer_collection_ptr_.is_valid()) {
    auto release_result = buffer_collection_ptr_->Release();
    (void)release_result;
  }
}

BufferCollectionInfo& BufferCollectionInfo::operator=(BufferCollectionInfo&& other) noexcept {
  if (buffer_collection_ptr_.is_valid()) {
    auto release_result = buffer_collection_ptr_->Release();
    (void)release_result;
  }
  buffer_collection_ptr_ = std::move(other.buffer_collection_ptr_);
  buffer_collection_info_ = std::move(other.buffer_collection_info_);
  return *this;
}

BufferCollectionInfo::BufferCollectionInfo(BufferCollectionInfo&& other) noexcept {
  if (buffer_collection_ptr_.is_valid()) {
    auto release_result = buffer_collection_ptr_->Release();
    (void)release_result;
  }
  buffer_collection_ptr_ = std::move(other.buffer_collection_ptr_);
  buffer_collection_info_ = std::move(other.buffer_collection_info_);
}

fit::result<fit::failed, BufferCollectionInfo> BufferCollectionInfo::New(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token,
    std::optional<fuchsia_sysmem2::ImageFormatConstraints> image_format_constraints,
    fuchsia_sysmem2::BufferUsage buffer_usage,
    allocation::BufferCollectionUsage buffer_collection_usage) {
  if (!buffer_collection_token.is_valid()) {
    FX_LOGS(ERROR) << "Buffer collection token is not valid.";
    return fit::failed();
  }

  auto [collection_client_end, collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          // Bind the buffer collection token to get the local token. Valid tokens can
          // always be bound, so we do not do any error checking at this stage.
          .token(std::move(buffer_collection_token))
          // Use local token to create a BufferCollection and then sync. We can trust
          // |buffer_collection->Sync()| to tell us if we have a bad or malicious channel.
          // So if this call passes, then we know we have a valid BufferCollection.
          .buffer_collection_request(std::move(collection_server_end))
          .Build());

  if (!result.ok()) {
    FX_LOGS(ERROR) << "Could not bind buffer collection. Status: " << result.status_string();
    return fit::failed();
  }

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
      std::move(collection_client_end));
  auto sync_result = buffer_collection->Sync();
  if (!sync_result.is_ok()) {
    FX_LOGS(ERROR) << "Could not sync buffer collection. Status: "
                   << sync_result.error_value().status_string();
    return fit::failed();
  }

  fuchsia_sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.priority(10u);
  set_name_request.name("FlatlandImageMemory");
  auto set_name_result = buffer_collection->SetName(std::move(set_name_request));
  if (!set_name_result.is_ok()) {
    FX_LOGS(ERROR) << "Could not set name on buffer collection.";
  }

  // Set basic usage constraints, such as requiring at least one buffer and using Vulkan. This is
  // necessary because all clients with a token need to set constraints before the buffer collection
  // can be allocated.
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  constraints.min_buffer_count(1);

  if (buffer_usage.cpu().has_value()) {
    fuchsia_sysmem2::BufferUsage usage_to_set = buffer_usage;
    if (buffer_collection_usage == allocation::BufferCollectionUsage::kRenderTarget) {
      usage_to_set.cpu(usage_to_set.cpu().value() | fuchsia_sysmem2::kCpuUsageWrite);
    } else {
      usage_to_set.cpu(usage_to_set.cpu().value() | fuchsia_sysmem2::kCpuUsageRead);
    }
    constraints.usage(std::move(usage_to_set));
  } else if (buffer_usage.vulkan().has_value()) {
    fuchsia_sysmem2::BufferUsage usage_to_set = buffer_usage;
    usage_to_set.vulkan(usage_to_set.vulkan().value() | fuchsia_sysmem2::kVulkanImageUsageSampled |
                        fuchsia_sysmem2::kVulkanImageUsageTransferSrc);
    constraints.usage(std::move(usage_to_set));
  } else {
    constraints.usage(std::move(buffer_usage));
  }

  if (image_format_constraints.has_value()) {
    constraints.image_format_constraints({{std::move(image_format_constraints.value())}});
    image_format_constraints.reset();
  }

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints(std::move(constraints));
  auto set_constraints_result =
      buffer_collection->SetConstraints(std::move(set_constraints_request));

  // From this point on, if we fail, we DCHECK, because we should have already caught errors
  // pertaining to both invalid tokens and wrong/malicious tokens/channels above, meaning that if
  // a failure occurs now, then there is some underlying issue unrelated to user input.
  FX_DCHECK(set_constraints_result.is_ok()) << "Could not set constraints on buffer collection.";

  return fit::ok(BufferCollectionInfo(std::move(buffer_collection)));
}

bool BufferCollectionInfo::BuffersAreAllocated() {
  // If the buffer_collection_info_ struct is already populated, then we know the
  // collection is allocated and we can skip over this code.
  if (!buffer_collection_info_.buffers().has_value()) {
    // Check to see if the buffers are allocated and return false if not.
    auto check_result = buffer_collection_ptr_->CheckAllBuffersAllocated();
    if (!check_result.is_ok()) {
      FX_LOGS(ERROR) << "Collection was not allocated - status: "
                     << check_result.error_value().FormatDescription();
      return false;
    }

    // We still have to call WaitForBuffersAllocated() here in order to fill in
    // the data for buffer_collection_info_. This won't block, since we've already
    // guaranteed that the collection is allocated above.
    auto wait_result = buffer_collection_ptr_->WaitForAllBuffersAllocated();
    // Failures here would be an issue with sysmem, and so we DCHECK.
    FX_DCHECK(wait_result.is_ok());

    buffer_collection_info_ = std::move(wait_result->buffer_collection_info().value());

    // Perform a DCHECK here as well to insure the collection has at least one vmo, because
    // it shouldn't have been able to be allocated with less than that.
    FX_DCHECK(buffer_collection_info_.buffers().value().size() > 0);
  }
  return true;
}

}  // namespace flatland
