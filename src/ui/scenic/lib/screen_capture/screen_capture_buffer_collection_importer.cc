// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "screen_capture_buffer_collection_importer.h"

#include <lib/async/default.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include <cstdint>
#include <optional>
#include <utility>

#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"

namespace {

using allocation::BufferCollectionUsage;
// Image formats supported by Scenic in a priority order.
const vk::Format kSupportedImageFormats[] = {vk::Format::eR8G8B8A8Srgb, vk::Format::eB8G8R8A8Srgb};

// Creates a new BufferCollectionTokenGroup from |token|. Then creates |num_tokens| number of
// children from |token_group|, calls AllChildrenPresent() and closes |token_group|.
std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> CreateChildTokens(
    fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, uint32_t num_tokens) {
  fidl::Arena arena;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionTokenGroup>::Create();
  fidl::OneWayStatus result = fidl::WireCall(token)->CreateBufferCollectionTokenGroup(
      fuchsia_sysmem2::wire::BufferCollectionTokenCreateBufferCollectionTokenGroupRequest::Builder(
          arena)
          .group_request(std::move(server_end))
          .Build());
  if (!result.ok()) {
    FX_LOGS(WARNING) << "Cannot create buffer collection token group: " << result.status_string();
    return {};
  }

  fidl::WireClient<fuchsia_sysmem2::BufferCollectionTokenGroup> token_group{
      std::move(client_end), async_get_default_dispatcher()};
  auto sync_result = token_group.sync()->Sync();
  if (!sync_result.ok()) {
    FX_LOGS(WARNING) << "Cannot sync token group: " << sync_result.status_string();
    return {};
  }

  std::vector<zx_rights_t> rights_attenuation_masks(num_tokens, ZX_RIGHT_SAME_RIGHTS);
  auto create_children_result = token_group.sync()->CreateChildrenSync(
      fuchsia_sysmem2::wire::BufferCollectionTokenGroupCreateChildrenSyncRequest::Builder(arena)
          .rights_attenuation_masks(
              fidl::VectorView<zx_rights_t>::FromExternal(rights_attenuation_masks))
          .Build());
  if (!create_children_result.ok()) {
    FX_LOGS(WARNING) << "Cannot create buffer collection token group children: "
                     << create_children_result.status_string();
    return {};
  }

  result = token_group.sync()->AllChildrenPresent();
  if (!result.ok()) {
    FX_LOGS(WARNING) << "Cannot call AllChildrenPresent: " << result.status_string();
    return {};
  }

  result = token_group.sync()->Release();
  if (!result.ok()) {
    FX_LOGS(WARNING) << "Cannot call Release: " << result.status_string();
    return {};
  }

  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> tokens;
  tokens.reserve(create_children_result->tokens().size());
  std::ranges::move(create_children_result->tokens(), std::back_inserter(tokens));
  return tokens;
}

// Consumes |token| to create a BufferCollectionSyncPtr and sets empty constraints on it.
std::optional<fuchsia::sysmem2::BufferCollectionSyncPtr>
CreateBufferCollectionSyncPtrAndSetEmptyConstraints(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token) {
  fuchsia::sysmem2::BufferCollectionSyncPtr local_buffer_collection;
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(token))
          .buffer_collection_request(fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>(
              local_buffer_collection.NewRequest().TakeChannel()))
          .Build());
  if (!result.ok()) {
    FX_LOGS(WARNING) << __func__
                     << " failed, could not BindSharedCollection: " << result.status_string();
    return std::nullopt;
  }

  fuchsia::sysmem2::Node_Sync_Result sync_result;
  zx_status_t status = local_buffer_collection->Sync(&sync_result);
  if (status != ZX_OK) {
    FX_LOGS(WARNING) << __func__ << " failed, could not bind buffer collection: " << status;
    return std::nullopt;
  }

  fuchsia::sysmem2::BufferCollectionSetConstraintsRequest request;
  request.set_constraints({});
  status = local_buffer_collection->SetConstraints(std::move(request));
  if (status != ZX_OK) {
    FX_LOGS(WARNING) << __func__ << " failed, could not set constraints: " << status;
    return std::nullopt;
  }
  return std::move(local_buffer_collection);
}

}  // anonymous namespace

namespace screen_capture {

ScreenCaptureBufferCollectionImporter::ScreenCaptureBufferCollectionImporter(
    fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator,
    std::shared_ptr<flatland::Renderer> renderer)
    : sysmem_allocator_(std::move(sysmem_allocator)), renderer_(std::move(renderer)) {}

ScreenCaptureBufferCollectionImporter::~ScreenCaptureBufferCollectionImporter() {
  for (auto id : buffer_collections_) {
    renderer_->ReleaseBufferCollection(id, BufferCollectionUsage::kRenderTarget);
  }
  buffer_collections_.clear();
}

fpromise::promise<> ScreenCaptureBufferCollectionImporter::ImportBufferCollection(
    allocation::GlobalBufferCollectionId collection_id,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
    allocation::BufferCollectionUsage usage, std::optional<fuchsia_math::SizeU> size) {
  TRACE_DURATION("gfx", "ScreenCaptureBufferCollectionImporter::ImportBufferCollection");
  // Expect only RenderTarget usage.
  FX_DCHECK(usage == BufferCollectionUsage::kRenderTarget);

  if (!token.is_valid()) {
    FX_LOGS(WARNING) << "ImportBufferCollection called with invalid token";
    return fpromise::make_error_promise();
  }

  if (buffer_collections_.find(collection_id) != buffer_collections_.end()) {
    FX_LOGS(WARNING) << __func__ << " failed, called with pre-existing collection_id "
                     << collection_id << ".";
    return fpromise::make_error_promise();
  }

  // We are looking for a buffer that either satisfies render target requirements or readback
  // requirements. Buffer that satisfy render target and client requirements gives us a zero copy
  // path for screen capture, so it is preferred. If not, we fall back to readback requirements,
  // which is as minimal. To express this, we create a token group hierarchy defined below and
  // skip setting constraints on |token|.
  // * token / local_buffer_collection
  // . * token_group
  // . . * out_tokens[0] / render_target_token
  // . . * out_tokens[1] / readback_token
  auto child_tokens = CreateChildTokens(token, 2);
  if (child_tokens.size() != 2) {
    return fpromise::make_error_promise();
  }

  auto local_buffer_collection =
      CreateBufferCollectionSyncPtrAndSetEmptyConstraints(sysmem_allocator, std::move(token));
  if (!local_buffer_collection) {
    return fpromise::make_error_promise();
  }

  fpromise::promise<> render_target_token =
      renderer_
          ->ImportBufferCollection(collection_id, sysmem_allocator, std::move(child_tokens[0]),
                                   BufferCollectionUsage::kRenderTarget, std::nullopt)
          .or_else([] {
            FX_LOGS(WARNING) << "Could not register render target token with VkRenderer";
            return fpromise::error();
          });

  fpromise::promise<> readback_token =
      renderer_
          ->ImportBufferCollection(collection_id, sysmem_allocator, std::move(child_tokens[1]),
                                   BufferCollectionUsage::kReadback, std::nullopt)
          .or_else([] {
            FX_LOGS(WARNING) << "Could not register readback token with VkRenderer";
            return fpromise::error();
          });

  return fpromise::join_promises(std::move(render_target_token), std::move(readback_token))
      .and_then([this, collection_id, local_buffer_collection = std::move(local_buffer_collection)](
                    std::tuple<fpromise::result<>, fpromise::result<>>& results) mutable
                    -> fpromise::result<> {
        if (auto& [result, protected_result] = results;
            result.is_error() || protected_result.is_error()) {
          return fpromise::error();
        }
        buffer_collection_sync_ptrs_[collection_id] = std::move(*local_buffer_collection);
        buffer_collections_.insert(collection_id);
        return fpromise::ok();
      });
}

void ScreenCaptureBufferCollectionImporter::ReleaseBufferCollection(
    allocation::GlobalBufferCollectionId collection_id, BufferCollectionUsage usage) {
  TRACE_DURATION("gfx", "ScreenCaptureBufferCollectionImporter::ReleaseBufferCollection");

  // If the collection is not in the map, then there's nothing to do.
  if (buffer_collections_.find(collection_id) == buffer_collections_.end()) {
    FX_LOGS(WARNING) << "Attempting to release a non-existent buffer collection.";
    return;
  }

  buffer_collections_.erase(collection_id);
  reset_render_targets_.erase(collection_id);

  if (buffer_collection_sync_ptrs_.find(collection_id) != buffer_collection_sync_ptrs_.end()) {
    buffer_collection_sync_ptrs_.erase(collection_id);
  };

  if (buffer_collection_buffer_counts_.find(collection_id) !=
      buffer_collection_buffer_counts_.end()) {
    buffer_collection_buffer_counts_.erase(collection_id);
  };

  renderer_->ReleaseBufferCollection(collection_id, usage);
}

fpromise::promise<> ScreenCaptureBufferCollectionImporter::ImportBufferImage(
    const allocation::ImageMetadata& metadata, BufferCollectionUsage usage) {
  TRACE_DURATION("gfx", "ScreenCaptureBufferCollectionImporter::ImportBufferImage");

  // The metadata can't have an invalid |collection_id|.
  if (metadata.collection_id == allocation::kInvalidId) {
    FX_LOGS(WARNING) << "Image has invalid collection id.";
    return fpromise::make_error_promise();
  }

  // The metadata can't have an invalid identifier.
  if (metadata.identifier == allocation::kInvalidImageId) {
    FX_LOGS(WARNING) << "Image has invalid identifier.";
    return fpromise::make_error_promise();
  }

  // Check for valid dimensions.
  if (metadata.width == 0 || metadata.height == 0) {
    FX_LOGS(WARNING) << "Image has invalid dimensions: "
                     << "(" << metadata.width << ", " << metadata.height << ").";
    return fpromise::make_error_promise();
  }

  // Make sure that the collection that will back this image's memory
  // is actually registered.
  auto collection_itr = buffer_collections_.find(metadata.collection_id);
  if (collection_itr == buffer_collections_.end()) {
    FX_LOGS(WARNING) << "Collection with id " << metadata.collection_id << " does not exist.";
    return fpromise::make_error_promise();
  }

  const std::optional<uint32_t> buffer_count =
      GetBufferCollectionBufferCount(metadata.collection_id);

  if (buffer_count == std::nullopt) {
    FX_LOGS(WARNING) << __func__ << " failed, buffer_count invalid";
    return fpromise::make_error_promise();
  }

  if (metadata.vmo_index >= buffer_count.value()) {
    FX_LOGS(WARNING) << __func__ << " failed, vmo_index " << metadata.vmo_index << " is invalid";
    return fpromise::make_error_promise();
  }

  FX_DCHECK(BufferCollectionUsage::kRenderTarget == usage);
  return renderer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget)
      .and_then([this, metadata]() -> fpromise::promise<> {
        // Render target allocation succeeded. We can use the client buffer as render target and
        // there is no need for readback buffers.
        if (!reset_render_targets_.contains(metadata.collection_id)) {
          renderer_->ReleaseBufferCollection(metadata.collection_id,
                                             BufferCollectionUsage::kReadback);
          return fpromise::make_ok_promise();
        }
        // Render target allocation succeeded on a buffer, where ResetRenderTargetsForReadback()
        // was called, so we need to set the client buffer as a readback buffer.
        return renderer_->ImportBufferImage(metadata, BufferCollectionUsage::kReadback);
      })
      .or_else([this, metadata, buffer_count = buffer_count.value()]() -> fpromise::promise<> {
        // Render target allocation failed, so we need to set the client buffer as a readback
        // buffer. Reset the imported buffer collections, reallocate a render target buffer and
        // re-import.
        return ResetRenderTargetsForReadback(metadata, buffer_count)
            .or_else([] {
              FX_LOGS(WARNING) << "Could not reallocate readback render targets";
              return fpromise::error();
            })
            .and_then([this, metadata]() -> fpromise::promise<> {
              return renderer_->ImportBufferImage(metadata, BufferCollectionUsage::kReadback);
            })
            .or_else([] {
              FX_LOGS(WARNING) << "Could not import fallback readback to VkRenderer";
              return fpromise::error();
            })
            .and_then([this, metadata]() -> fpromise::promise<> {
              return renderer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget);
            })
            .or_else([] {
              FX_LOGS(WARNING) << "Could not import fallback render target to VkRenderer";
              return fpromise::error();
            });
      });
}

void ScreenCaptureBufferCollectionImporter::ReleaseBufferImage(allocation::GlobalImageId image_id) {
  TRACE_DURATION("gfx", "ScreenCaptureBufferCollectionImporter::ReleaseBufferImage");
  renderer_->ReleaseBufferImage(image_id);
}

std::optional<BufferCount> ScreenCaptureBufferCollectionImporter::GetBufferCollectionBufferCount(
    allocation::GlobalBufferCollectionId collection_id) {
  // If the collection info has not been retrieved before, wait for the buffers to be allocated
  // and populate the map/delete the reference to the |collection_id| from
  // |collection_id_sync_ptrs_|.
  if (auto it = buffer_collection_buffer_counts_.find(collection_id);
      it != buffer_collection_buffer_counts_.end()) {
    return it->second;
  }

  auto it = buffer_collection_sync_ptrs_.find(collection_id);
  if (it == buffer_collection_sync_ptrs_.end()) {
    FX_LOGS(WARNING) << "Collection with id " << collection_id << " does not exist.";
    return std::nullopt;
  }
  fuchsia::sysmem2::BufferCollectionSyncPtr buffer_collection = std::move(it->second);

  fuchsia::sysmem2::BufferCollection_CheckAllBuffersAllocated_Result check_allocated_result;
  zx_status_t status = buffer_collection->CheckAllBuffersAllocated(&check_allocated_result);
  if (status != ZX_OK) {
    FX_LOGS(WARNING) << __func__ << " failed, no buffers allocated - status: " << status;
    return std::nullopt;
  }

  if (check_allocated_result.is_framework_err()) {
    FX_LOGS(WARNING) << __func__ << " failed, no buffers allocated - framework_err: "
                     << fidl::ToUnderlying(check_allocated_result.framework_err());
    return std::nullopt;
  }
  if (check_allocated_result.is_err()) {
    ZX_DEBUG_ASSERT(check_allocated_result.is_err());
    FX_LOGS(WARNING) << __func__ << " failed, no buffers allocated - err: "
                     << static_cast<uint32_t>(check_allocated_result.err());
    return std::nullopt;
  }

  fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
  status = buffer_collection->WaitForAllBuffersAllocated(&wait_result);
  if (status != ZX_OK) {
    FX_LOGS(WARNING) << __func__ << " failed, waiting on no buffers allocated - status: " << status;
    return std::nullopt;
  }
  if (wait_result.is_framework_err()) {
    FX_LOGS(WARNING) << __func__ << " failed, waiting on no buffers allocated - framework_err: "
                     << fidl::ToUnderlying(wait_result.framework_err());
    return std::nullopt;
  }
  if (wait_result.is_err()) {
    FX_LOGS(WARNING) << __func__ << " failed, waiting on no buffers allocated - err: "
                     << static_cast<uint32_t>(wait_result.framework_err());
    return std::nullopt;
  }
  auto buffer_collection_info = std::move(*wait_result.response().mutable_buffer_collection_info());

  buffer_collection_sync_ptrs_.erase(it);
  buffer_collection->Release();

  const size_t buffer_count = buffer_collection_info.buffers().size();
  buffer_collection_buffer_counts_[collection_id] = buffer_count;
  return buffer_count;
}

fpromise::promise<> ScreenCaptureBufferCollectionImporter::ResetRenderTargetsForReadback(
    const allocation::ImageMetadata& metadata, uint32_t buffer_count) {
  // Resetting render target for readback only should happen once at the first ImportBufferImage
  // from that BufferCollection. Don't do it again if this method had already been called for this
  // |metadata.collection_id|.
  if (reset_render_targets_.contains(metadata.collection_id)) {
    return fpromise::make_ok_promise();
  }

  FX_LOGS(INFO) << "Could not import render target to VkRenderer; attempting to create fallback";
  renderer_->ReleaseBufferCollection(metadata.collection_id, BufferCollectionUsage::kRenderTarget);

  auto deregister_collection =
      fit::defer([renderer = renderer_, collection_id = metadata.collection_id] {
        renderer->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kReadback);
      });

  fuchsia::sysmem2::BufferCollectionTokenSyncPtr fallback_render_target_sync_token;
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator_->AllocateSharedCollection(
      fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
          .token_request(fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken>(
              fallback_render_target_sync_token.NewRequest().TakeChannel()))
          .Build());
  if (!result.ok()) {
    FX_LOGS(WARNING) << "Cannot allocate fallback render target sync token: "
                     << result.status_string();
    return fpromise::make_error_promise();
  }

  fuchsia::sysmem2::BufferCollectionTokenHandle fallback_render_target_token;
  fuchsia::sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
  dup_request.set_rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);
  dup_request.set_token_request(fallback_render_target_token.NewRequest());
  zx_status_t status = fallback_render_target_sync_token->Duplicate(std::move(dup_request));
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Cannot duplicate fallback render target sync token: "
                   << zx_status_get_string(status);
    return fpromise::make_error_promise();
  }

  fuchsia::sysmem2::BufferCollectionSyncPtr buffer_collection;
  result = sysmem_allocator_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(
              fallback_render_target_sync_token.Unbind().TakeChannel()))
          .buffer_collection_request(fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>(
              buffer_collection.NewRequest().TakeChannel()))
          .Build());
  if (!result.ok()) {
    return fpromise::make_error_promise();
  }

  return renderer_
      ->ImportBufferCollection(metadata.collection_id, sysmem_allocator_,
                               fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>{
                                   fallback_render_target_token.TakeChannel()},
                               BufferCollectionUsage::kRenderTarget,
                               {{metadata.width, metadata.height}})
      .or_else([] {
        FX_LOGS(WARNING) << "Could not register fallback render target with VkRenderer";
        return fpromise::error();
      })
      .and_then([this, collection_id = metadata.collection_id, buffer_count,
                 deregister_collection = std::move(deregister_collection),
                 buffer_collection = std::move(buffer_collection)]() mutable -> fpromise::result<> {
        fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
        auto& constraints = *set_constraints_request.mutable_constraints();
        constraints.set_min_buffer_count(buffer_count);
        constraints.mutable_usage()->set_none(fuchsia::sysmem2::NONE_USAGE);
        zx_status_t status = buffer_collection->SetConstraints(std::move(set_constraints_request));
        if (status != ZX_OK) {
          FX_LOGS(WARNING) << "Cannot set constraints on fallback render target collection: "
                           << zx_status_get_string(status);
          return fpromise::error();
        }

        fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
        status = buffer_collection->WaitForAllBuffersAllocated(&wait_result);
        if (status != ZX_OK) {
          FX_LOGS(WARNING)
              << "Could not wait on allocation for fallback render target collection - status: "
              << zx_status_get_string(status);
          return fpromise::error();
        }
        if (wait_result.is_framework_err()) {
          FX_LOGS(WARNING)
              << "Could not wait on allocation for fallback render target collection - framework_err: "
              << fidl::ToUnderlying(wait_result.framework_err());
          return fpromise::error();
        }
        if (wait_result.is_err()) {
          FX_LOGS(WARNING)
              << "Could not wait on allocation for fallback render target collection - err: "
              << static_cast<uint32_t>(wait_result.err());
          return fpromise::error();
        }

        status = buffer_collection->Release();
        if (status != ZX_OK) {
          FX_LOGS(WARNING) << "Could not close fallback render target collection: "
                           << zx_status_get_string(status);
          return fpromise::error();
        }

        reset_render_targets_.insert(collection_id);
        deregister_collection.cancel();
        return fpromise::ok();
      });
}

}  // namespace screen_capture
