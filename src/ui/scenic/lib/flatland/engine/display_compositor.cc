// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/engine/display_compositor.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include <array>
#include <cstdint>
#include <vector>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/display/util.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"
#include "src/ui/scenic/lib/utils/fidl_array_cast.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/logging.h"

namespace flatland {

namespace {

// Debugging color used to highlight images that have gone through the GPU rendering path.
const std::array<float, 4> kGpuRenderingDebugColor = {0.9f, 0.5f, 0.5f, 1.f};

// Returns an image type that describes the tiling format used for buffer with
// this pixel format. The values are display driver specific and not documented
// in the display coordinator FIDL API.
// TODO(https://fxbug.dev/42108519): Remove this when image type is removed from the display
// coordinator API.
uint32_t BufferCollectionPixelFormatToImageTilingType(
    fuchsia_images2::PixelFormatModifier pixel_format_modifier) {
  switch (pixel_format_modifier) {
    case fuchsia_images2::PixelFormatModifier::kIntelI915XTiled:
      return 1;  // IMAGE_TILING_TYPE_X_TILED
    case fuchsia_images2::PixelFormatModifier::kIntelI915YTiled:
      return 2;  // IMAGE_TILING_TYPE_Y_LEGACY_TILED
    case fuchsia_images2::PixelFormatModifier::kIntelI915YfTiled:
      return 3;  // IMAGE_TILING_TYPE_YF_TILED
    case fuchsia_images2::PixelFormatModifier::kLinear:
    default:
      return fuchsia_hardware_display_types::kImageTilingTypeLinear;
  }
}

// Creates a duplicate of |token| in |duplicate|.
// Returns an error string if it fails, otherwise std::nullopt.
std::optional<std::string> DuplicateToken(
    fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken>& token,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>& duplicate) {
  fuchsia_sysmem2::BufferCollectionTokenDuplicateSyncRequest dup_sync_request;
  dup_sync_request.rights_attenuation_masks(std::vector{ZX_RIGHT_SAME_RIGHTS});
  auto dup_sync_result = token->DuplicateSync(std::move(dup_sync_request));
  if (dup_sync_result.is_error()) {
    return std::string("Could not duplicate token - error: ") +
           dup_sync_result.error_value().FormatDescription();
  }
  FX_DCHECK(dup_sync_result.value().tokens().value().size() == 1);
  duplicate = std::move(dup_sync_result.value().tokens().value()[0]);
  return std::nullopt;
}

// Returns a prunable subtree of |token| with |num_new_tokens| children.
// Returns std::nullopt on failure.
std::optional<std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>>
CreatePrunableChildren(fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                       fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
                       const size_t num_new_tokens) {
  fidl::SyncClient<fuchsia_sysmem2::BufferCollectionTokenGroup> token_group;
  {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionTokenGroup>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = fidl::WireCall(token)->CreateBufferCollectionTokenGroup(
        fuchsia_sysmem2::wire::BufferCollectionTokenCreateBufferCollectionTokenGroupRequest::
            Builder(arena)
                .group_request(std::move(server_end))
                .Build());
    if (!result.ok()) {
      FX_LOGS(ERROR) << "Could not create buffer collection token group: "
                     << result.status_string();
      return std::nullopt;
    }
    token_group.Bind(std::move(client_end));
  }

  // Create the requested children, then mark all children created and close out |token_group|.
  {
    std::vector<zx_rights_t> children_request_rights(num_new_tokens, ZX_RIGHT_SAME_RIGHTS);
    fuchsia_sysmem2::BufferCollectionTokenGroupCreateChildrenSyncRequest create_children_request;
    create_children_request.rights_attenuation_masks(std::move(children_request_rights));

    auto create_children_result =
        token_group->CreateChildrenSync(std::move(create_children_request));
    if (create_children_result.is_error()) {
      FX_LOGS(ERROR) << "Could not create buffer collection token group children: "
                     << create_children_result.error_value().FormatDescription();
      return std::nullopt;
    }
    if (const auto status = token_group->AllChildrenPresent(); status.is_error()) {
      FX_LOGS(ERROR) << "Could not call AllChildrenPresent: "
                     << status.error_value().FormatDescription();
      return std::nullopt;
    }
    if (const auto status = token_group->Release(); status.is_error()) {
      FX_LOGS(ERROR) << "Could not release token group: "
                     << status.error_value().FormatDescription();
      return std::nullopt;
    }

    auto out_tokens = std::move(create_children_result.value().tokens().value());
    FX_DCHECK(out_tokens.size() == num_new_tokens);
    return out_tokens;
  }
}

// Returns a BufferCollection duplicate of |token| with empty constraints set.
// Since it has the same failure domain as |token|, it can be used to check the status of
// allocations made from that collection.
std::optional<fidl::SyncClient<fuchsia_sysmem2::BufferCollection>>
CreateDuplicateBufferCollectionPtrWithEmptyConstraints(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken>& token) {
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_dup;
  if (auto error = DuplicateToken(token, token_dup)) {
    FX_LOGS(ERROR) << *error;
    return std::nullopt;
  }

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection;
  {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(token_dup))
            .buffer_collection_request(std::move(server_end))
            .Build());
    FX_DCHECK(result.ok());
    buffer_collection.Bind(std::move(client_end));
  }

  if (const auto status = buffer_collection->SetConstraints(
          fuchsia_sysmem2::BufferCollectionSetConstraintsRequest{});
      status.is_error()) {
    FX_LOGS(ERROR) << "Could not set constraints: " << status.error_value().FormatDescription();
    return std::nullopt;
  }

  return buffer_collection;
}

// Returns whether |metadata| describes a valid image.
bool IsValidBufferImage(const allocation::ImageMetadata& metadata) {
  if (metadata.identifier == display::kInvalidImageId) {
    FX_LOGS(ERROR) << "ImageMetadata identifier is invalid.";
    return false;
  }

  if (metadata.collection_id == allocation::kInvalidId) {
    FX_LOGS(ERROR) << "ImageMetadata collection ID is invalid.";
    return false;
  }

  if (metadata.width == 0 || metadata.height == 0) {
    FX_LOGS(ERROR) << "ImageMetadata has a null dimension: "
                   << "(" << metadata.width << ", " << metadata.height << ").";
    return false;
  }

  return true;
}

// Calls CheckBuffersAllocated |token| and returns whether the allocation succeeded.
bool CheckBuffersAllocated(fidl::SyncClient<fuchsia_sysmem2::BufferCollection>& token) {
  auto check_status = token->CheckAllBuffersAllocated();
  return check_status.is_ok();
}

// Calls WaitForBuffersAllocated() on |token| and returns the pixel format of the allocation.
// |token| must have already checked that buffers are allocated.
// TODO(https://fxbug.dev/42150686): Delete after we don't need the pixel format anymore.
fuchsia_images2::PixelFormatModifier GetPixelFormatModifier(
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection>& token) {
  auto wait_result = token->WaitForAllBuffersAllocated();
  FX_DCHECK(wait_result.is_ok()) << "WaitForBuffersAllocated failed: "
                                 << wait_result.error_value().FormatDescription();
  return wait_result.value()
      .buffer_collection_info()
      .value()
      .settings()
      .value()
      .image_format_constraints()
      .value()
      .pixel_format_modifier()
      .value();
}

// Consumes |token| and if its allocation is compatible with the display returns its pixel format.
// Otherwise returns std::nullopt.
// TODO(https://fxbug.dev/42150686): Just return a bool after we don't need the pixel format
// anymore.
std::optional<fuchsia_images2::PixelFormatModifier> DetermineDisplaySupportFor(
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> token) {
  std::optional<fuchsia_images2::PixelFormatModifier> result = std::nullopt;

  const bool image_supports_display = CheckBuffersAllocated(token);
  if (image_supports_display) {
    result = GetPixelFormatModifier(token);
  }

  auto release_status = token->Release();
  (void)release_status;
  return result;
}

}  // anonymous namespace

DisplayCompositor::DisplayCompositor(async_dispatcher_t* main_dispatcher,
                                     std::shared_ptr<display::CoordinatorProxy> coordinator_proxy,
                                     const std::shared_ptr<Renderer>& renderer,
                                     fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator,
                                     const DisplayCompositorConfig& config)
    : display_coordinator_shared_ptr_(std::move(coordinator_proxy)),
      display_coordinator_(*display_coordinator_shared_ptr_),
      renderer_(renderer),
      release_fence_manager_(main_dispatcher),
      sysmem_allocator_(std::move(sysmem_allocator)),
      main_dispatcher_(main_dispatcher),
      config_(config) {
  FX_CHECK(main_dispatcher_);
  FX_DCHECK(renderer_);
  FX_DCHECK(sysmem_allocator_);
  FX_DCHECK(display_coordinator_shared_ptr_);
}

DisplayCompositor::~DisplayCompositor() {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  // Destroy all of the display layers.
  //
  // TODO(https://fxbug.dev/447261550): this is really bad.  Luckily it doesn't impact production.
  {
    const fidl::OneWayStatus result = display_coordinator_.raw().sync()->DiscardConfig();
    if (!result.ok()) {
      FX_LOGS(ERROR) << "Failed to call FIDL DiscardConfig method: " << result.status_string();
    }
    for (const auto& [_, data] : display_engine_data_map_) {
      fidl::OneWayStatus result =
          display_coordinator_.raw()->DestroyLayer(data.empty_scene_layer.ToFidl());
      if (!result.ok()) {
        FX_LOGS(ERROR) << "Failed to call FIDL DestroyLayer method: " << result.status_string();
      }
      for (const display::LayerId& layer : data.layers) {
        result = display_coordinator_.raw()->DestroyLayer(layer.ToFidl());
        if (!result.ok()) {
          FX_LOGS(ERROR) << "Failed to call FIDL DestroyLayer method: " << result.status_string();
        }
      }
      for (const auto& event_data : data.frame_event_datas) {
        result = display_coordinator_.raw()->ReleaseEvent(event_data.wait_id.ToFidl());
        if (!result.ok()) {
          FX_LOGS(ERROR) << "Failed to call FIDL ReleaseEvent on wait event ("
                         << event_data.wait_id.value() << "): " << result.status_string();
        }
      }
    }
  }

  // TODO(https://fxbug.dev/42063495): Release |render_targets| and |protected_render_targets|
  // collections and images.
}

fpromise::promise<> DisplayCompositor::ImportBufferCollection(
    allocation::GlobalBufferCollectionId collection_id,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> renderer_token,
    BufferCollectionUsage usage, std::optional<fuchsia_math::SizeU> size) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::ImportBufferCollection");
  FX_DCHECK(usage == BufferCollectionUsage::kClientImage)
      << "Expected default buffer collection usage";

  // We want to achieve one of two outcomes:
  // 1. Allocate buffer that is compatible with both the renderer and the display
  // or, if that fails,
  // 2. Allocate a buffer that is only compatible with the renderer.
  // To do this we create two prunable children of the renderer token, one with display constraints
  // and one with no constraints. Only one of these children will be chosen during sysmem
  // negotiations.
  // Resulting tokens:
  // * renderer_token
  // . * token_group
  // . . * display_token (+ duplicate with no constraints to check allocation with, created below)
  // . . * Empty token
  fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken> display_token;
  if (auto prunable_tokens = CreatePrunableChildren(sysmem_allocator, renderer_token,
                                                    /*num_new_tokens*/ 2)) {
    // Display+Renderer should have higher priority than Renderer only.
    display_token.Bind(std::move(prunable_tokens->at(0)));

    // We close the second token without setting any constraints. If this gets chosen during sysmem
    // negotiations then the allocated buffers are display-incompatible and we don't need to keep a
    // reference to them here.
    fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken> empty_token(
        std::move(prunable_tokens->at(1)));
    if (const auto status = empty_token->Release(); status.is_error()) {
      FX_LOGS(ERROR) << "Could not close token: " << status.error_value().FormatDescription();
    }
  } else {
    return fpromise::make_error_promise();
  }

  // Set renderer constraints.
  // TODO(https://fxbug.dev/488038340): Parallelise setting the renderer and display constraints.
  return renderer_
      ->ImportBufferCollection(collection_id, sysmem_allocator, std::move(renderer_token), usage,
                               size)
      .or_else([] {
        FX_LOGS(ERROR) << "Renderer could not import buffer collection";
        return fpromise::error();
      })
      // This `and_then` handles the case where the renderer successfully imported the buffer
      // collection. It attempts to import the buffer collection into the display coordinator.
      .and_then([this, collection_id,
                 display_token = std::move(display_token)]() mutable -> fpromise::result<> {
        if (!config_.enable_direct_to_display) {
          // Forced fallback to using the renderer; don't attempt direct-to-display.
          // Close |display_token| without importing it to the display coordinator.
          if (const auto status = display_token->Release(); status.is_error()) {
            FX_LOGS(ERROR) << "Could not close token: " << status.error_value().FormatDescription();
          }
          return fpromise::ok();
        }

        // Create a BufferCollectionPtr from a duplicate of |display_token| with which to later
        // check if buffers allocated from the BufferCollection are display-compatible.
        auto collection_ptr = CreateDuplicateBufferCollectionPtrWithEmptyConstraints(
            sysmem_allocator_, display_token);
        if (!collection_ptr.has_value()) {
          return fpromise::error();
        }

        std::scoped_lock lock(lock_);
        {
          const auto [_, success] =
              display_buffer_collection_ptrs_.emplace(collection_id, std::move(*collection_ptr));
          FX_DCHECK(success);
        }

        // Import the buffer collection into the display coordinator, setting display constraints.
        fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> natural_display_token =
            display_token.TakeClientEnd();
        bool import_success = ImportBufferCollectionToDisplayCoordinator(
            collection_id, std::move(natural_display_token),
            fuchsia_hardware_display_types::wire::ImageBufferUsage{
                .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
            });
        if (!import_success) {
          return fpromise::error();
        }
        return fpromise::ok();
      });
}

void DisplayCompositor::ReleaseBufferCollection(
    const allocation::GlobalBufferCollectionId collection_id, const BufferCollectionUsage usage) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::ReleaseBufferCollection");
  FX_DCHECK(usage == BufferCollectionUsage::kClientImage);

  renderer_->ReleaseBufferCollection(collection_id, usage);

  std::scoped_lock lock(lock_);
  FX_DCHECK(display_coordinator_.is_valid());
  const display::WireBufferCollectionId display_collection_id =
      display::ToDisplayFidlBufferCollectionId(collection_id);
  const fidl::OneWayStatus result =
      display_coordinator_.raw().sync()->ReleaseBufferCollection(display_collection_id);
  if (!result.ok()) {
    FX_LOGS(ERROR) << "Failed to call FIDL ReleaseBufferCollection method: "
                   << result.status_string();
  }
  display_buffer_collection_ptrs_.erase(collection_id);
  buffer_collection_supports_display_.erase(collection_id);
}

fpromise::promise<> DisplayCompositor::ImportBufferImage(const allocation::ImageMetadata& metadata,
                                                         const BufferCollectionUsage usage) {
  // Called from main thread or Flatland threads.
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::ImportBufferImage");

  if (!IsValidBufferImage(metadata)) {
    return fpromise::make_error_promise();
  }

  // NOTE: The VkRenderer::ImportBufferImage() is currently synchronous. If we want to improve
  // latency, we'd need to make it asynchronous and start both import operations concurrently.
  return renderer_->ImportBufferImage(metadata, usage)
      .or_else([] {
        FX_LOGS(ERROR) << "Renderer could not import image.";
        return fpromise::error();
      })
      .and_then([this, metadata]() -> fpromise::result<> {
        std::scoped_lock lock(lock_);
        FX_DCHECK(display_coordinator_.is_valid());

        const allocation::GlobalBufferCollectionId collection_id = metadata.collection_id;
        const display::WireBufferCollectionId display_collection_id =
            display::ToDisplayFidlBufferCollectionId(collection_id);
        const bool display_support_already_set =
            buffer_collection_supports_display_.contains(collection_id);

        // When display composition is disabled, the only images that should be imported by the
        // display are the framebuffers, and their display support is already set in AddDisplay()
        // (instead of below). For every other image with display composition off mode we can early
        // exit.
        if (!config_.enable_direct_to_display &&
            (!display_support_already_set || !buffer_collection_supports_display_[collection_id])) {
          buffer_collection_supports_display_[collection_id] = false;
          return fpromise::ok();
        }

        if (!display_support_already_set) {
          // TODO(https://fxbug.dev/386263977): this makes blocking FIDL calls while `lock_` is
          // held. This isn't great, because means that a Flatland session thread can block the
          // render thread.
          auto node = display_buffer_collection_ptrs_.extract(collection_id);
          if (node.empty()) {
            FX_LOGS(ERROR) << "Display buffer collection token not found for collection ID "
                           << collection_id;
            return fpromise::error();
          }
          const auto pixel_format_modifier = DetermineDisplaySupportFor(std::move(node.mapped()));
          buffer_collection_supports_display_[collection_id] = pixel_format_modifier.has_value();
          if (pixel_format_modifier.has_value()) {
            buffer_collection_tiling_type_map_[collection_id] =
                BufferCollectionPixelFormatToImageTilingType(pixel_format_modifier.value());
          }
        }

        if (!buffer_collection_supports_display_[collection_id]) {
          // When display isn't supported we fallback to using the renderer.
          return fpromise::ok();
        }

        // TODO(https://fxbug.dev/42150686): Pixel format (and hence tiling type) should be ignored
        // when using sysmem. We do not want to have to deal with this default image format. Work
        // was in progress to address this, but is currently stalled: see fxr/716543.
        FX_DCHECK(buffer_collection_tiling_type_map_.contains(collection_id));
        const uint32_t image_tiling_type = buffer_collection_tiling_type_map_.at(collection_id);

        const auto image_extent = types::Extent2({.width = static_cast<int32_t>(metadata.width),
                                                  .height = static_cast<int32_t>(metadata.height)});

        zx::result<> result =
            display_coordinator_.ImportImage(image_extent, image_tiling_type, display_collection_id,
                                             metadata.vmo_index, metadata.identifier);
        if (result.is_ok()) {
          image_tiling_type_map_[metadata.identifier] = image_tiling_type;
          return fpromise::ok();
        }

        return fpromise::error();
      });
}

void DisplayCompositor::ReleaseBufferImage(const allocation::GlobalImageId image_id) {
  // Called from main thread or Flatland threads.
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::ReleaseBufferImage");
  FX_DCHECK(image_id != allocation::kInvalidImageId);

  renderer_->ReleaseBufferImage(image_id);

  std::scoped_lock lock(lock_);

  if (image_tiling_type_map_.erase(image_id) == 1) {
    FX_DCHECK(display_coordinator_.is_valid());
    display_coordinator_.ReleaseImage(display::ImageId(image_id));
  }
}

void DisplayCompositor::SetDisplayLayers(const display::DisplayId display_id,
                                         const std::span<display::LayerId>& layers) {
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::SetDisplayLayers");
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_DCHECK(display_coordinator_.is_valid());

  // Set all of the layers for each of the images on the display.
  display_coordinator_.SetDisplayLayers(display_id, layers);
}

bool DisplayCompositor::SetRenderDataOnDisplay(const RenderData& data) {
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::SetRenderDataOnDisplay", "display_id",
                 data.display_id.value(), "rectangle_count", data.layers.size());

  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  const uint32_t num_layers = static_cast<uint32_t>(data.layers.size());

  DisplayEngineData& display_engine_data = display_engine_data_map_.at(data.display_id);

  // If the display doesn't support any layers, we cannot composite anything to it,
  // not even an empty scene or GPU fallback image.
  if (display_engine_data.max_layer_count == 0) {
    TRACE_INSTANT("gfx", "scenic_d2d_failed: no hardware layers available", TRACE_SCOPE_THREAD);
    FLATLAND_VERBOSE_LOG << "SetRenderDataOnDisplay() failed: display " << data.display_id.value()
                         << " supports 0 layers.";
    return false;
  }

  // Proactively fallback to GPU composition if the number of layers exceeds the hardware limit.
  if (num_layers > display_engine_data.max_layer_count) {
    TRACE_INSTANT("gfx", "scenic_d2d_failed: too few hardware layers available",
                  TRACE_SCOPE_THREAD);
    FLATLAND_VERBOSE_LOG << "SetRenderDataOnDisplay() falling back to GPU: "
                         << "requested layers (" << num_layers << ") exceeds limit ("
                         << display_engine_data.max_layer_count << ") for display "
                         << data.display_id.value();
    return false;
  }

  if (num_layers == 0) {
    SetDisplayLayers(data.display_id, std::span(&display_engine_data.empty_scene_layer, 1));
    return true;
  }

  // Since we map 1 image to 1 layer, if there are more images than layers available for
  // the given display, then they cannot be directly composited to the display in hardware.
  std::vector<display::LayerId>& layers = display_engine_data.layers;
  if (layers.size() < num_layers) {
    TRACE_INSTANT("gfx", "scenic_d2d_failed: insufficient layers available", TRACE_SCOPE_THREAD);
    FLATLAND_VERBOSE_LOG << "SetRenderDataOnDisplay() failed: insufficient layers available.";
    return false;
  }

  // We only set as many layers as needed for the images we have.
  SetDisplayLayers(data.display_id, std::span{layers.data(), num_layers});

  for (uint32_t i = 0; i < num_layers; i++) {
    const auto& layer = data.layers[i];
    if (std::holds_alternative<ResolvedLayer::ImageContent>(layer.content)) {
      const auto& image = std::get<ResolvedLayer::ImageContent>(layer.content);
      const allocation::GlobalImageId image_id = image.image_id;
      if (image_tiling_type_map_.contains(image_id)) {
        ApplyLayerImage(layers[i], layer, /*wait_id=*/display::kInvalidEventId);
      } else {
        // TODO(https://fxbug.dev/496160334): Previously, the only way this could happen is if the
        // image couldn't be displayed directly by the display driver (e.g. for formats that Vulkan
        // can handle, but not the display driver).
        TRACE_INSTANT("gfx", "scenic_d2d_failed: image not imported for direct-display",
                      TRACE_SCOPE_THREAD);
        FLATLAND_VERBOSE_LOG
            << "SetRenderDataOnDisplay() failed: image not imported for direct-display.";
        return false;
      }
    } else {
      const auto& solid_color = std::get<ResolvedLayer::SolidColorContent>(layer.content);
      const std::array<float, 4> final_color = {
          solid_color.color[0] * layer.multiply_color[0],
          solid_color.color[1] * layer.multiply_color[1],
          solid_color.color[2] * layer.multiply_color[2],
          solid_color.color[3] * layer.multiply_color[3],
      };
      ApplyLayerColor(layers[i], layer.geometry, final_color, layer.blend_mode);
    }
  }

  return true;
}

void DisplayCompositor::ApplyLayerColor(const display::LayerId& layer_id, const SrcToDest& geometry,
                                        const std::array<float, 4>& color,
                                        const types::BlendMode& blend_mode) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_DCHECK(display_coordinator_.is_valid());

  // We have to convert the image_metadata's multiply color, which is an array of normalized
  // floating point values, to an unnormalized array of uint8_ts in the range 0-255.
  const fidl::Array<uint8_t, 8> color_bytes = {
      static_cast<uint8_t>(255 * color[0]),
      static_cast<uint8_t>(255 * color[1]),
      static_cast<uint8_t>(255 * color[2]),
      static_cast<uint8_t>(255 * color[3]),
      0,
      0,
      0,
      0,
  };

  const display::Rectangle display_destination({
      .x = static_cast<int32_t>(geometry.dest.x()),
      .y = static_cast<int32_t>(geometry.dest.y()),
      .width = static_cast<int32_t>(geometry.dest.width()),
      .height = static_cast<int32_t>(geometry.dest.height()),
  });

  display_coordinator_.SetLayerColorConfig(
      layer_id, {.format = fuchsia_images2::PixelFormat::kB8G8R8A8, .bytes = color_bytes},
      display_destination);

// TODO(https://fxbug.dev/42056054): Currently, not all display hardware supports the ability to
// set either the position or the alpha on a color layer, as color layers are not primary
// layers. There exist hardware that require a color layer to be the backmost layer and to be
// the size of the entire display. This means that for the time being, we must rely on GPU
// composition for solid color rects.
//
// There is the option of assigning a 1x1 image with the desired color to a standard image layer,
// as a way of mimicking color layers (and this is what is done in the GPU path as well) --
// however, not all hardware supports images with sizes that differ from the destination size of
// the rect. So implementing that solution on the display path as well is problematic.
#if 0
  const auto [src, dst] = DisplaySrcDstFrames::New(geometry);

  display_coordinator_.SetLayerPrimaryPosition(
      layer_id, geometry.transform, src, dst);

  const fuchsia_hardware_display_types::AlphaMode alpha_mode =
      image.blend_mode.ToDisplayAlphaMode();
  display_coordinator_.SetLayerPrimaryAlpha(layer_id, image.blend_mode, image.multiply_color[3]);
#endif
}

void DisplayCompositor::ApplyLayerImage(const display::LayerId& layer_id,
                                        const ResolvedLayer& layer,
                                        const display::EventId& wait_id) {
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::ApplyLayerImage");
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_DCHECK(display_coordinator_.is_valid());
  FX_DCHECK(std::holds_alternative<ResolvedLayer::ImageContent>(layer.content));

  const auto& image = std::get<ResolvedLayer::ImageContent>(layer.content);

  const auto [src, dst] = DisplaySrcDstFrames::New(layer.geometry);
  FX_DCHECK(src.width() && src.height()) << "Source frame cannot be empty.";
  FX_DCHECK(dst.width() && dst.height()) << "Destination frame cannot be empty.";

  FX_DCHECK(image_tiling_type_map_.contains(image.image_id));
  const auto image_tiling_type = image_tiling_type_map_.at(image.image_id);
  const types::Extent2 image_extent(
      {.width = static_cast<int32_t>(image.width), .height = static_cast<int32_t>(image.height)});
  display_coordinator_.SetLayerPrimaryConfig(layer_id, image_extent, image_tiling_type);

  display_coordinator_.SetLayerPrimaryPosition(layer_id, layer.geometry.transform, src, dst);

  display_coordinator_.SetLayerPrimaryAlpha(layer_id, layer.blend_mode, layer.multiply_color[3]);

  // Set the imported image on the layer.
  display_coordinator_.SetLayerImage(layer_id, display::ImageId(image.image_id), wait_id);
}

zx::result<> DisplayCompositor::ApplyConfig(uint64_t frame_number, uint64_t trace_flow_id) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_DCHECK(display_coordinator_.is_valid());

  display::WireConfigStamp config_stamp = next_config_stamp_;
  next_config_stamp_ = display::WireConfigStamp(next_config_stamp_.value + 1);

  FLATLAND_VERBOSE_LOG << "DisplayCompositor::ApplyConfig() config_stamp=" << config_stamp.value;

  TRACE_DURATION("gfx", "flatland::DisplayCompositor::ApplyConfig");
  fidl::Arena arena;

  TRACE_FLOW_BEGIN("gfx", "Display::CommitConfig", config_stamp.value);
  auto result = display_coordinator_.ApplyConfig(config_stamp);
  if (result.is_error()) {
    return result;
  }

  pending_apply_configs_.push_back({
      .config_stamp = config_stamp,
      .frame_number = frame_number,
      .trace_flow_id = trace_flow_id,
  });
  return fit::ok();
}

bool DisplayCompositor::PerformGpuComposition(
    const uint64_t frame_number, const uint64_t trace_flow_id,
    const zx::time_monotonic presentation_time, std::span<const RenderData> render_data_list,
    std::vector<zx::event> release_fences, std::vector<zx::counter> release_counters,
    std::vector<zx::counter> present_fences, scheduling::FramePresentedCallback callback) {
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::PerformGpuComposition");
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  // Create an event that will be signaled when the final display's content has finished
  // rendering; it will be passed into |release_fence_manager_.OnGpuCompositedFrame()|.  If there
  // are multiple displays which require GPU-composited content, we pass this event to be signaled
  // when the final display's content has finished rendering (thus guaranteeing that all previous
  // content has also finished rendering).
  // TODO(https://fxbug.dev/42157678): we might want to reuse events, instead of creating a new one
  // every frame.
  zx::event render_finished_fence = utils::CreateEvent();

  bool applied_display_mode = false;
  for (size_t i = 0; i < render_data_list.size(); ++i) {
    const bool is_final_display = i == (render_data_list.size() - 1);
    const auto& render_data = render_data_list[i];
    const auto display_engine_data_it = display_engine_data_map_.find(render_data.display_id);
    FX_DCHECK(display_engine_data_it != display_engine_data_map_.end());
    auto& display_engine_data = display_engine_data_it->second;

    // Clear any past CC state here, before applying GPU CC.
    if (cc_state_machine_.GpuRequiresDisplayClearing()) {
      TRACE_DURATION("gfx", "flatland::DisplayCompositor::PerformGpuComposition[cc]");
      display_coordinator_.SetDisplayColorConversion(
          render_data.display_id, kDefaultColorConversionOffsets,
          kDefaultColorConversionCoefficients, kDefaultColorConversionOffsets);
      // TODO(https://fxbug.dev/449801667): investigate whether making this call here can cause
      // problems when GPU composition fails.  This is not a high priority issue, because we
      // generally rely on GPU composition succeeding.
      cc_state_machine_.DisplayCleared();
    }

    if (display_engine_data.vmo_count == 0) {
      FX_LOGS(WARNING) << "No VMOs were created when creating display "
                       << render_data.display_id.value() << ".";
      return false;
    }
    const uint32_t curr_vmo = display_engine_data.curr_vmo;
    display_engine_data.curr_vmo =
        (display_engine_data.curr_vmo + 1) % display_engine_data.vmo_count;
    const auto& render_targets = renderer_->RequiresRenderInProtected(render_data.layers)
                                     ? display_engine_data.protected_render_targets
                                     : display_engine_data.render_targets;
    FX_DCHECK(curr_vmo < render_targets.size()) << curr_vmo << "/" << render_targets.size();
    FX_DCHECK(curr_vmo < display_engine_data.frame_event_datas.size())
        << curr_vmo << "/" << display_engine_data.frame_event_datas.size();
    const auto& render_target = render_targets[curr_vmo];

    // Reset the event data.
    auto& event_data = display_engine_data.frame_event_datas[curr_vmo];
    event_data.wait_event.signal(ZX_EVENT_SIGNALED, 0);

    // Apply the debugging color to the images.
    std::vector<ResolvedLayer> tinted_layers;
    if (config_.tint_gpu_fallback_images) {
      // Unfortunately we copy the list here due to constness.
      tinted_layers.assign(render_data.layers.begin(), render_data.layers.end());
      for (auto& layer : tinted_layers) {
        layer.multiply_color[0] *= kGpuRenderingDebugColor[0];
        layer.multiply_color[1] *= kGpuRenderingDebugColor[1];
        layer.multiply_color[2] *= kGpuRenderingDebugColor[2];
        layer.multiply_color[3] *= kGpuRenderingDebugColor[3];
      }
    }
    const std::span layers = config_.tint_gpu_fallback_images ? tinted_layers : render_data.layers;

    // Prepare semaphores for this render pass.
    std::array<zx::event, 2> render_fences;
    size_t num_render_fences = 1;
    render_fences[0] = std::move(event_data.wait_event);
    if (is_final_display) {
      render_fences[num_render_fences++] = std::move(render_finished_fence);
    }

    Renderer::RenderArgs render_args{
        .release_fences = std::span<zx::event>(render_fences.data(), num_render_fences),
        .apply_color_conversion = cc_state_machine_.GetDataToApply().has_value(),
    };
    if (config_.enable_frame_counter_overlay) {
      render_args.display_frame_number = frame_number;
    }
    // const render_args allows us to retrieve the fences after the render call.
    renderer_->Render(render_target, layers, render_args);

    event_data.wait_event = std::move(render_args.release_fences[0]);
    if (is_final_display) {
      render_finished_fence = std::move(render_args.release_fences[1]);
    }

    if (display_engine_data.layers.empty()) {
      FLATLAND_VERBOSE_LOG << "PerformGpuComposition() failed: no layers available for display "
                           << render_data.display_id.value();
      return false;
    }

    /* const*/ display::LayerId layer_id = display_engine_data.layers[0];
    SetDisplayLayers(render_data.display_id, std::span<display::LayerId>{&layer_id, 1});

    ResolvedLayer gpu_layer = {
        .geometry =
            SrcToDest(types::RectangleF({.x = 0,
                                         .y = 0,
                                         .width = static_cast<float>(render_target.width),
                                         .height = static_cast<float>(render_target.height)}),
                      types::RectangleF({.x = 0,
                                         .y = 0,
                                         .width = static_cast<float>(render_target.width),
                                         .height = static_cast<float>(render_target.height)}),
                      types::RotateFlip::kIdentity()),
        .multiply_color = {1.f, 1.f, 1.f, 1.f},
        .content =
            ResolvedLayer::ImageContent{
                .image_id = render_target.identifier,
                .width = render_target.width,
                .height = render_target.height,
            },
    };

    ApplyLayerImage(layer_id, gpu_layer, event_data.wait_id);

    applied_display_mode =
        applied_display_mode || MaybeSetPendingDisplayMode(render_data.display_id);
  }

  {
    // We expect this to succeed, or else something is very wrong.
    auto result = ApplyConfig(frame_number, trace_flow_id);
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Both display hardware composition and GPU rendering have failed:"
                     << result.status_string();
      return false;
    }
  }

  if (applied_display_mode) {
    // We set one or more display modes, and they passed `CheckConfig()` so we won't need to apply
    // them again.
    ClearAllPendingDisplayModes(render_data_list);
  }

  // See ReleaseFenceManager comments for details.
  FX_DCHECK(render_finished_fence);
  release_fence_manager_.OnGpuCompositedFrame(
      frame_number, std::move(render_finished_fence), std::move(release_fences),
      std::move(release_counters), std::move(present_fences), std::move(callback));
  return true;
}

DisplayCompositor::RenderFrameResult DisplayCompositor::RenderFrame(
    const uint64_t frame_number, const zx::time_monotonic presentation_time,
    std::span<const RenderData> render_data_list, std::vector<zx::event> release_fences,
    std::vector<zx::counter> release_counters, std::vector<zx::counter> present_fences,
    scheduling::FramePresentedCallback callback, RenderFrameTestArgs test_args) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::RenderFrame");
  std::scoped_lock lock(lock_);

  if (last_frame_number_) {
    FX_CHECK(frame_number > *last_frame_number_);
  }
  last_frame_number_ = frame_number;

  uint64_t trace_flow_id = TRACE_NONCE();
  TRACE_FLOW_BEGIN("gfx", "render_frame_to_vsync", trace_flow_id);

  // Determine whether we need to fall back to GPU composition. Avoid calling CheckConfig() if we
  // don't need to, because this requires a round-trip to the display coordinator.
  // Notes:
  //   - failing TryDirectToDisplay() means that the display driver is unable to directly display
  //     this frame's list of client images.
  //   - `enable_frame_counter_overlay` currently requires GPU composition because we use
  //     `escher::DebugFont` to blit the overlay directly into the displayed framebuffer.
  const bool should_try_direct_to_display = config_.enable_direct_to_display &&
                                            !test_args.force_gpu_composition &&
                                            !config_.enable_frame_counter_overlay;
  if (should_try_direct_to_display) {
    if (TryDirectToDisplay(render_data_list, frame_number, trace_flow_id)) {
      for (const auto& data : render_data_list) {
        const int32_t num_render_data = static_cast<int32_t>(data.layers.size());
        const uint64_t display_id = data.display_id.value();
        TRACE_COUNTER("gfx", "Scenic D2D images", display_id, "count", TA_INT32(num_render_data));
        TRACE_COUNTER("gfx", "Scenic GPU images", display_id, "count", TA_INT32(0));
      }

      // CC was successfully applied to the config so we update the state machine.
      cc_state_machine_.SetApplyConfigSucceeded();

      // See ReleaseFenceManager comments for details.
      release_fence_manager_.OnDirectScanoutFrame(frame_number, std::move(release_fences),
                                                  std::move(release_counters),
                                                  std::move(present_fences), std::move(callback));
      return RenderFrameResult::kDirectToDisplay;
    }
  }

  if (PerformGpuComposition(frame_number, trace_flow_id, presentation_time, render_data_list,
                            std::move(release_fences), std::move(release_counters),
                            std::move(present_fences), std::move(callback))) {
    for (const auto& data : render_data_list) {
      const int32_t num_render_data = static_cast<int32_t>(data.layers.size());
      const uint64_t display_id = data.display_id.value();
      TRACE_COUNTER("gfx", "Scenic D2D images", display_id, "count", TA_INT32(0));
      TRACE_COUNTER("gfx", "Scenic GPU images", display_id, "count", TA_INT32(num_render_data));
    }

    return RenderFrameResult::kGpuComposition;
  }

  // Clear counters to indicate that rendering didn't happen.
  for (const auto& data : render_data_list) {
    const int32_t num_render_data = static_cast<int32_t>(data.layers.size());
    const uint64_t display_id = data.display_id.value();
    TRACE_COUNTER("gfx", "Scenic D2D images", display_id, "count", TA_INT32(0));
    TRACE_COUNTER("gfx", "Scenic GPU images", display_id, "count", TA_INT32(0));
  }

  return RenderFrameResult::kFailure;
}

bool DisplayCompositor::TryDirectToDisplay(std::span<const RenderData> render_data_list,
                                           uint64_t frame_number, uint64_t trace_flow_id) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_DCHECK(config_.enable_direct_to_display);
  TRACE_DURATION("gfx", "flatland::DisplayCompositor::TryDirectToDisplay");

  bool applied_display_mode = false;
  for (const auto& data : render_data_list) {
    const display::DisplayId& display_id = data.display_id;
    if (!SetRenderDataOnDisplay(data)) {
      // TODO(https://fxbug.dev/42157429): just because setting the data on one display fails (e.g.
      // due to too many layers), that doesn't mean that all displays need to use GPU-composition.
      // Some day we might want to use GPU-composition for some client images, and direct-scanout
      // for others.
      FLATLAND_VERBOSE_LOG
          << "DisplayCompositor::TryDirectToDisplay() failed SetRenderDataOnDisplay()";
      return false;
    }

    // Check the state machine to see if there's any CC data to apply.
    if (const auto cc_data = cc_state_machine_.GetDataToApply()) {
      display_coordinator_.SetDisplayColorConversion(
          display_id, (*cc_data).preoffsets, (*cc_data).coefficients, (*cc_data).postoffsets);
    }

    applied_display_mode = applied_display_mode || MaybeSetPendingDisplayMode(display_id);
  }

  auto result = ApplyConfig(frame_number, trace_flow_id);
  if (result.is_error()) {
    // No TRACE_INSTANT("gfx", "scenic_d2d_failed:") here: ApplyConfig() has info to generate a
    // more specific event.
    return false;
  }

  if (applied_display_mode) {
    // We set one or more display modes, and they passed `CheckConfig()` so we won't need to apply
    // them again.
    ClearAllPendingDisplayModes(render_data_list);
  }
  return true;
}

void DisplayCompositor::OnVsync(zx::time_monotonic timestamp,
                                display::WireConfigStamp displayed_config_stamp) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "Flatland::DisplayCompositor::OnVsync");

  // We might receive multiple OnVsync() callbacks with the same |displayed_config_stamp| if the
  // scene doesn't change. Early exit for these cases.
  if (last_presented_config_stamp_.has_value() &&
      displayed_config_stamp.value == last_presented_config_stamp_->value) {
    return;
  }

  // Verify that the configuration from Vsync is in the [pending_apply_configs_] queue.
  const auto vsync_frame_it =
      std::find_if(pending_apply_configs_.begin(), pending_apply_configs_.end(),
                   [displayed_config_stamp](const ApplyConfigInfo& info) {
                     return info.config_stamp.value == displayed_config_stamp.value;
                   });

  // This shouldn't be possible, now that the ancient Gfx code has been expunged from Scenic.
  if (vsync_frame_it == pending_apply_configs_.end()) {
    FX_LOGS(ERROR) << "DisplayCompositor::OnVsync() config_stamp=" << displayed_config_stamp.value
                   << "  ... skipping: stamp was not generated by current DisplayCompositor";
    return;
  }

  FLATLAND_VERBOSE_LOG << "DisplayCompositor::OnVsync() config_stamp="
                       << displayed_config_stamp.value << "  timestamp=" << timestamp.get();

  // Handle the presented ApplyConfig() call, as well as the skipped ones.
  auto it = pending_apply_configs_.begin();
  auto end = std::next(vsync_frame_it);
  while (it != end) {
    TRACE_FLOW_END("gfx", "render_frame_to_vsync", it->trace_flow_id);
    release_fence_manager_.OnVsync(it->frame_number, timestamp);
    it = pending_apply_configs_.erase(it);
  }
  last_presented_config_stamp_ = displayed_config_stamp;
}

DisplayCompositor::FrameEventData DisplayCompositor::NewFrameEventData() {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FrameEventData result;
  {  // The DC waits on this to be signaled by the renderer.
    const auto status = zx::event::create(0, &result.wait_event);
    FX_DCHECK(status == ZX_OK);
  }
  result.wait_id = display_coordinator_.ImportEvent(result.wait_event);
  FX_DCHECK(result.wait_id != display::kInvalidEventId);
  return result;
}

fpromise::promise<> DisplayCompositor::AddDisplay(
    display::Display* display, const DisplayInfo info, const uint32_t num_render_targets,
    fuchsia_sysmem2::BufferCollectionInfo* out_collection_info) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_CHECK(display);
  TRACE_DURATION_BEGIN("gfx", "Flatland::DisplayCompositor::AddDisplay");

  FLATLAND_VERBOSE_LOG << "DisplayCompositor::AddDisplay(): display_id="
                       << display->display_id().value() << "  size=" << info.dimensions.x << "x"
                       << info.dimensions.y << "  num_render_targets=" << num_render_targets;

  // Grab the best pixel format that the renderer prefers given the list of available formats on
  // the display.
  FX_DCHECK(!info.formats.empty());
  const auto pixel_format = renderer_->ChoosePreferredRenderTargetFormat(info.formats);

  const fuchsia_math::SizeU size = {{.width = info.dimensions.x, .height = info.dimensions.y}};
  FX_DCHECK(size.width() > 0 && size.height() > 0)
      << "Invalid display size: " << size.width() << "x" << size.height();

  const display::DisplayId& display_id = display->display_id();
  FX_DCHECK(!display_engine_data_map_.contains(display_id))
      << "DisplayCompositor::AddDisplay(): display already exists: " << display_id.value();

  DisplayEngineData& display_engine_data = display_engine_data_map_[display_id];
  display_engine_data.max_layer_count = info.max_layer_count;

  // Used to set mode before the next `ApplyConfig()`.
  display_engine_data.updated_display_mode.emplace(display->mode());

  {
    std::scoped_lock lock(lock_);

    // Prepare the black layer to render an empty scene to the display.
    display_engine_data.empty_scene_layer = display_coordinator_.CreateLayer();
    const display::Rectangle display_destination({.x = 0,
                                                  .y = 0,
                                                  .width = static_cast<int32_t>(size.width()),
                                                  .height = static_cast<int32_t>(size.height())});
    display_coordinator_.SetLayerColorConfig(
        display_engine_data.empty_scene_layer,
        {.format = fuchsia_images2::PixelFormat::kB8G8R8A8, .bytes = {0, 0, 0, 255, 0, 0, 0, 0}},
        display_destination);

    // When we add in a new display, we create a couple of layers for that display upfront to be
    // used when we directly composite render data in hardware via the display coordinator.
    // TODO(https://fxbug.dev/42157936): per-display layer lists are probably a bad idea; this
    // approach doesn't reflect the constraints of the underlying display hardware.
    for (uint32_t i = 0; i < display->max_layer_count(); i++) {
      display_engine_data.layers.push_back(display_coordinator_.CreateLayer());
    }
  }

  // Add vsync callback on display. Note that this will overwrite the existing callback on
  // |display| and other clients won't receive any, i.e. gfx.
  display->AddVsyncCallback(
      [weak_ref = weak_from_this()](zx::time timestamp,
                                    display::WireConfigStamp displayed_config_stamp) {
        if (auto ref = weak_ref.lock())
          ref->OnVsync(timestamp, displayed_config_stamp);
      });

  // Exit early if there are no vmos to create.
  if (num_render_targets == 0) {
    return fpromise::make_ok_promise();
  }

  // If we are creating vmos, we need a non-null buffer collection pointer to return back
  // to the caller.
  fpromise::promise<> render_targets_promise =
      AllocateDisplayRenderTargets(
          /*use_protected_memory=*/false, num_render_targets, size, pixel_format,
          out_collection_info)
          .and_then([this, num_render_targets, &display_engine_data](
                        std::vector<allocation::ImageMetadata>& render_targets) mutable {
            display_engine_data.render_targets = std::move(render_targets);

            {
              std::scoped_lock lock(lock_);
              for (uint32_t i = 0; i < num_render_targets; i++) {
                display_engine_data.frame_event_datas.push_back(NewFrameEventData());
              }
            }
            display_engine_data.vmo_count = num_render_targets;
            display_engine_data.curr_vmo = 0;
          });

  // Create another set of tokens and allocate a protected render target. Protected memory buffer
  // pool is usually limited, so it is better for Scenic to preallocate to avoid being blocked by
  // running out of protected memory.
  if (renderer_->SupportsRenderInProtected()) {
    fpromise::promise<> protected_render_targets_promise =
        AllocateDisplayRenderTargets(
            /*use_protected_memory=*/true, num_render_targets, size, pixel_format)
            .and_then([&display_engine_data](
                          std::vector<allocation::ImageMetadata>& render_targets) mutable {
              display_engine_data.protected_render_targets = std::move(render_targets);
            });
    fpromise::promise<> join_promise =
        fpromise::join_promises(std::move(render_targets_promise),
                                std::move(protected_render_targets_promise))
            .and_then([](std::tuple<fpromise::result<>, fpromise::result<>>& results)
                          -> fpromise::result<> {
              if (auto& [result, protected_result] = results;
                  result.is_error() || protected_result.is_error()) {
                return fpromise::error();
              }
              return fpromise::ok();
            });
    render_targets_promise = std::move(join_promise);
  }

  return render_targets_promise.then([](fpromise::result<>& result) {
    TRACE_DURATION_END("gfx", "Flatland::DisplayCompositor::AddDisplay");
  });
}

void DisplayCompositor::SetColorConversionValues(const fidl::Array<float, 9>& coefficients,
                                                 const fidl::Array<float, 3>& preoffsets,
                                                 const fidl::Array<float, 3>& postoffsets) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  cc_state_machine_.SetData({.coefficients = utils::ReinterpretFidlArrayAsStdArray(coefficients),
                             .preoffsets = utils::ReinterpretFidlArrayAsStdArray(preoffsets),
                             .postoffsets = utils::ReinterpretFidlArrayAsStdArray(postoffsets)});

  renderer_->SetColorConversionValues(coefficients, preoffsets, postoffsets);
}

bool DisplayCompositor::SetMinimumRgb(const uint8_t minimum_rgb) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  std::scoped_lock lock(lock_);
  FX_DCHECK(display_coordinator_.is_valid());

  const auto result = display_coordinator_.raw().sync()->SetMinimumRgb(minimum_rgb);
  if (!result.ok()) {
    FX_LOGS(ERROR) << "SetMinimumRgb transport error: " << result.status_string();
    return false;
  }
  if (result->is_error()) {
    FX_LOGS(ERROR) << "SetMinimumRgb method error: " << zx_status_get_string(result->error_value());
    return false;
  }
  return true;
}

fpromise::promise<std::vector<allocation::ImageMetadata>>
DisplayCompositor::AllocateDisplayRenderTargets(
    const bool use_protected_memory, const uint32_t num_render_targets,
    const fuchsia_math::SizeU& size, const fuchsia_images2::PixelFormat pixel_format,
    fuchsia_sysmem2::BufferCollectionInfo* out_collection_info) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  fidl::Arena arena;
  // Create the buffer collection token to be used for frame buffers.
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  {
    fidl::OneWayStatus result = sysmem_allocator_->AllocateSharedCollection(
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
            .token_request(std::move(server_end))
            .Build());
    FX_DCHECK(result.ok()) << "status: " << result.status_string();
  }

  // Duplicate the token for the display and for the renderer.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> compositor_token{std::move(client_end)};
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> renderer_token;
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> display_token;
  {
    std::array<zx_rights_t, 2> rights_attenuation_masks{ZX_RIGHT_SAME_RIGHTS, ZX_RIGHT_SAME_RIGHTS};
    const auto result =
        fidl::WireCall(compositor_token)
            ->DuplicateSync(
                fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncRequest::Builder(arena)
                    .rights_attenuation_masks(
                        fidl::VectorView<zx_rights_t>::FromExternal(rights_attenuation_masks))
                    .Build());
    FX_DCHECK(result.ok()) << "status: " << result.status_string();
    FX_DCHECK(result->has_tokens());
    FX_DCHECK(result->tokens().size() == 2);

    renderer_token = std::move(result->tokens()[0]);
    display_token = std::move(result->tokens()[1]);

    constexpr size_t kMaxSysmem1DebugNameLength = 64;

    auto set_debug_name = [&arena](
                              fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
                              const char* const name) {
      std::string client_name =
          std::format("AllocateDisplayRenderTargets {} {}", name, fsl::GetCurrentProcessName());
      if (client_name.size() > kMaxSysmem1DebugNameLength) {
        client_name.resize(kMaxSysmem1DebugNameLength);
      }
      // set debug info for renderer_token in case it fails unexpectedly or similar
      auto result = fidl::WireCall(token)->SetDebugClientInfo(
          fuchsia_sysmem2::wire::NodeSetDebugClientInfoRequest::Builder(arena)
              .name(std::move(client_name))
              .id(fsl::GetCurrentProcessKoid())
              .Build());
      FX_DCHECK(result.ok()) << "set_info_status: " << result.status_string();
    };

    set_debug_name(renderer_token, "renderer_token");
    set_debug_name(display_token, "display_token");

    // The compositor_token inherited it's debug info from sysmem_allocator_, so is still set to
    // "scenic flatland::DisplayCompositor" at this point, which is fine; just need to be able to
    // tell which token is potentially failing below - at this point each token (compositor_token,
    // renderer_token, display_token) has distinguishable debug info.
  }

  // Set renderer constraints.
  const auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  return renderer_
      ->ImportBufferCollection(collection_id, sysmem_allocator_, std::move(renderer_token),
                               BufferCollectionUsage::kRenderTarget,
                               std::optional<fuchsia_math::SizeU>(size))
      // TODO(https://fxbug.dev/502763366): Scenic assumes immortality of DisplayCompositor.
      .and_then([this, use_protected_memory, num_render_targets, size, pixel_format,
                 compositor_token = std::move(compositor_token),
                 display_token = std::move(display_token), collection_id,
                 out_collection_info]() mutable {
        {  // Set display constraints.
          std::scoped_lock lock(lock_);
          const bool result = ImportBufferCollectionToDisplayCoordinator(
              collection_id, std::move(display_token),
              fuchsia_hardware_display_types::wire::ImageBufferUsage{
                  .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
              });
          FX_DCHECK(result);
        }

// Set local constraints.
#ifdef CPU_ACCESSIBLE_VMO
        const bool make_cpu_accessible = true;
#else
        const bool make_cpu_accessible = false;
#endif

        fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
        if (make_cpu_accessible && !use_protected_memory) {
          auto [buffer_usage, memory_constraints] = GetUsageAndMemoryConstraintsForCpuWriteOften();
          collection_ptr = CreateBufferCollectionSyncPtrAndSetConstraints(
              sysmem_allocator_, std::move(compositor_token), num_render_targets, size.width(),
              size.height(), std::move(buffer_usage), pixel_format, std::move(memory_constraints));
        } else {
          fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
          fuchsia_sysmem2::BufferCollectionConstraints constraints;
          constraints.min_buffer_count_for_camping(num_render_targets);
          fuchsia_sysmem2::BufferUsage usage;
          usage.none(fuchsia_sysmem2::kNoneUsage);
          constraints.usage(std::move(usage));
          if (use_protected_memory) {
            fuchsia_sysmem2::BufferMemoryConstraints bmc;
            bmc.secure_required(true);
            bmc.inaccessible_domain_supported(true);
            bmc.cpu_domain_supported(false);
            bmc.ram_domain_supported(false);
            constraints.buffer_memory_constraints(std::move(bmc));
          }

          {
            auto [client_end, server_end] =
                fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
            fidl::Arena arena;
            fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
                fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                    .token(std::move(compositor_token))
                    .buffer_collection_request(std::move(server_end))
                    .Build());
            FX_DCHECK(result.ok());
            collection_ptr.Bind(std::move(client_end));
          }

          fuchsia_sysmem2::NodeSetNameRequest set_name_request;
          set_name_request.priority(10u);
          set_name_request.name(use_protected_memory
                                    ? "FlatlandDisplayCompositorProtectedRenderTarget"
                                    : "FlatlandDisplayCompositorRenderTarget");
          auto set_name_result = collection_ptr->SetName(std::move(set_name_request));
          FX_DCHECK(set_name_result.is_ok());

          set_constraints_request.constraints(std::move(constraints));
          const auto status = collection_ptr->SetConstraints(std::move(set_constraints_request));
          FX_DCHECK(status.is_ok()) << "status: " << status.error_value().FormatDescription();
        }

        // Wait for buffers allocated so it can populate its information struct with the vmo data.
        fuchsia_sysmem2::BufferCollectionInfo collection_info;
        {
          auto wait_result = collection_ptr->WaitForAllBuffersAllocated();
          FX_DCHECK(wait_result.is_ok())
              << "wait_result: " << wait_result.error_value().FormatDescription();
          collection_info = std::move(wait_result.value().buffer_collection_info().value());
        }

        {
          const auto status = collection_ptr->Release();
          FX_DCHECK(status.is_ok()) << "status: " << status.error_value().FormatDescription();
        }

        // We know that this collection is supported by display because we collected constraints
        // from display in display::ImportBufferCollection() and waited for successful allocation.
        {
          std::scoped_lock lock(lock_);
          buffer_collection_supports_display_[collection_id] = true;
          buffer_collection_tiling_type_map_[collection_id] =
              BufferCollectionPixelFormatToImageTilingType(collection_info.settings()
                                                               .value()
                                                               .image_format_constraints()
                                                               .value()
                                                               .pixel_format_modifier()
                                                               .value());
        }

        // The collection info is no longer needed, so move it to out_collection_info if provided.
        if (out_collection_info) {
          *out_collection_info = std::move(collection_info);
        }

        std::vector<fpromise::promise<allocation::ImageMetadata>> promises;
        promises.reserve(num_render_targets);
        for (uint32_t i = 0; i < num_render_targets; i++) {
          const allocation::ImageMetadata target = {
              .collection_id = collection_id,
              .identifier = allocation::GenerateUniqueImageId(),
              .vmo_index = i,
              .width = size.width(),
              .height = size.height(),
          };
          auto promise = ImportBufferImage(target, BufferCollectionUsage::kRenderTarget)
                             .and_then([target]() -> fpromise::result<allocation::ImageMetadata> {
                               return fpromise::ok(target);
                             });
          promises.push_back(std::move(promise));
        }
        return fpromise::join_promise_vector(std::move(promises));
      })
      .and_then([](const std::vector<fpromise::result<allocation::ImageMetadata>>& results)
                    -> fpromise::result<std::vector<allocation::ImageMetadata>> {
        std::vector<allocation::ImageMetadata> render_targets;
        render_targets.reserve(results.size());
        for (auto& result : results) {
          if (result.is_ok()) {
            render_targets.push_back(result.value());
          } else {
            FX_LOGS(ERROR) << "Failed to import buffer image";
            return fpromise::error();
          }
        }
        return fpromise::ok(std::move(render_targets));
      });
}

bool DisplayCompositor::ImportBufferCollectionToDisplayCoordinator(
    allocation::GlobalBufferCollectionId identifier,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
    const fuchsia_hardware_display_types::wire::ImageBufferUsage& image_buffer_usage) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  return display::ImportBufferCollection(identifier, display_coordinator_.raw(), std::move(token),
                                         image_buffer_usage);
}

bool DisplayCompositor::MaybeSetPendingDisplayMode(const display::DisplayId& display_id) {
  auto it = display_engine_data_map_.find(display_id);
  if (it == display_engine_data_map_.end()) {
    FX_LOGS(WARNING) << "No display engine data found for display_id=" << display_id.value();
    return false;
  }
  auto& maybe_mode = it->second.updated_display_mode;
  if (!maybe_mode.has_value()) {
    return false;
  }
  display_coordinator_.SetDisplayMode(display_id, types::DisplayMode::From(maybe_mode.value()));
  return true;
}

void DisplayCompositor::ClearAllPendingDisplayModes(std::span<const RenderData> render_data_list) {
  for (auto& render_data : render_data_list) {
    auto it = display_engine_data_map_.find(render_data.display_id);
    if (it == display_engine_data_map_.end()) {
      FX_LOGS(WARNING) << "No display engine data found for display_id="
                       << render_data.display_id.value();
      continue;
    }
    it->second.updated_display_mode.reset();
  }
}

}  // namespace flatland
