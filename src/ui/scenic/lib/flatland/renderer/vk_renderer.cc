// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"

#include <fuchsia/sysmem/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <array>
#include <memory_resource>

#include "src/lib/fidl/contrib/fpromise/client.h"
#include "src/ui/lib/escher/escher.h"
#include "src/ui/lib/escher/flatland/rectangle_compositor.h"
#include "src/ui/lib/escher/forward_declarations.h"
#include "src/ui/lib/escher/impl/naive_image.h"
#include "src/ui/lib/escher/impl/semaphore_pool.h"
#include "src/ui/lib/escher/impl/vulkan_utils.h"
#include "src/ui/lib/escher/renderer/batch_gpu_uploader.h"
#include "src/ui/lib/escher/renderer/render_funcs.h"
#include "src/ui/lib/escher/renderer/sampler_cache.h"
#include "src/ui/lib/escher/third_party/granite/vk/command_buffer.h"
#include "src/ui/lib/escher/util/fuchsia_utils.h"
#include "src/ui/lib/escher/util/image_utils.h"
#include "src/ui/scenic/lib/flatland/image_formats.h"
#include "src/ui/scenic/lib/utils/shader_warmup.h"

#include <glm/glm.hpp>
#include <glm/gtc/type_ptr.hpp>
#include <vulkan/vulkan.hpp>

namespace flatland {
namespace {

using allocation::BufferCollectionUsage;

// TODO(https://fxbug.dev/42072347): We support two framebuffer formats and warmup for both.
// * RGBA is the only supported format for AFBC on mali. Should be the default in production.
// * BGRA is the only format that allows screen capture and testing on mali. It is also used as
// default on screenshots.
const std::vector<vk::Format> kSupportedRenderTargetImageFormats = {vk::Format::eR8G8B8A8Srgb,
                                                                    vk::Format::eB8G8R8A8Srgb};

const std::vector<vk::Format> kSupportedReadbackImageFormats = {vk::Format::eR8G8B8A8Srgb,
                                                                vk::Format::eB8G8R8A8Srgb};

const std::vector<vk::Format>& GetSupportedImageFormatsForBufferCollectionUsage(
    BufferCollectionUsage usage) {
  switch (usage) {
    case BufferCollectionUsage::kClientImage:
      return SupportedClientImageFormats();
      break;
    case BufferCollectionUsage::kRenderTarget:
      return kSupportedRenderTargetImageFormats;
      break;
    case BufferCollectionUsage::kReadback:
      return kSupportedReadbackImageFormats;
      break;
  }
}

const vk::Filter kDefaultFilter = vk::Filter::eLinear;

// Black color to replace protected content when we aren't in protected mode, i.e. Screenshots.
const glm::vec4 kProtectedReplacementColorInRGBA = glm::vec4(0, 0, 0, 1);

// Returns the corresponding Vulkan image format to use given the provided
// Zircon image format.
vk::Format ConvertToVkFormat(const fuchsia_images2::PixelFormat pixel_format) {
  switch (pixel_format) {
    // These two Zircon formats correspond to the Sysmem BGRA32 format.
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      return vk::Format::eB8G8R8A8Srgb;
    // These two Zircon formats correspond to the Sysmem R8G8B8A8 format.
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      return vk::Format::eR8G8B8A8Srgb;
    case fuchsia_images2::PixelFormat::kNv12:
      return vk::Format::eG8B8R82Plane420Unorm;
    case fuchsia_images2::PixelFormat::kR5G6B5:
      return vk::Format::eR5G6B5UnormPack16;
    default:
      FX_CHECK(false) << "Unsupported Zircon pixel format: " << static_cast<uint32_t>(pixel_format);
      return vk::Format::eUndefined;
  }
}

// Create a default 1x1 texture for solid color renderables which are not associated
// with an image.
escher::TexturePtr CreateWhiteTexture(escher::Escher* escher,
                                      escher::BatchGpuUploader* gpu_uploader) {
  FX_DCHECK(escher);
  uint8_t channels[4];
  channels[0] = channels[1] = channels[2] = channels[3] = 255;
  auto image = escher->NewRgbaImage(gpu_uploader, 1, 1, channels);
  return escher->NewTexture(std::move(image), vk::Filter::eNearest);
}

escher::TexturePtr CreateDepthTexture(escher::Escher* escher,
                                      const escher::ImagePtr& output_image) {
  escher::TexturePtr depth_buffer;
  escher::RenderFuncs::ObtainDepthTexture(
      escher, output_image->use_protected_memory(), output_image->info(),
      escher->device()->caps().GetMatchingDepthStencilFormat().value, depth_buffer);
  return depth_buffer;
}

std::atomic<uint64_t> next_buffer_collection_id = 1;

uint64_t GetNextBufferCollectionId() { return next_buffer_collection_id++; }

std::string GetNextBufferCollectionIdString(const std::string& prefix) {
  // Would use std::ostringstream here, except it bloats binary size by ~50kB, causing CQ to fail.
  return prefix + "-" + std::to_string(GetNextBufferCollectionId());
}

std::string GetImageName(const BufferCollectionUsage usage) {
  switch (usage) {
    case BufferCollectionUsage::kRenderTarget:
      return "FlatlandRenderTargetMemory";
    case BufferCollectionUsage::kReadback:
      return "FlatlandReadbackMemory";
    case BufferCollectionUsage::kClientImage:
      return "FlatlandImageMemory";
    default:
      FX_NOTREACHED();
      return "";
  }
}

vk::ImageUsageFlags GetImageUsageFlags(const BufferCollectionUsage usage) {
  switch (usage) {
    case BufferCollectionUsage::kRenderTarget:
      return escher::RectangleCompositor::kRenderTargetUsageFlags |
             vk::ImageUsageFlagBits::eTransferSrc;
    case BufferCollectionUsage::kReadback:
      return vk::ImageUsageFlagBits::eTransferDst;
    case BufferCollectionUsage::kClientImage:
      return escher::RectangleCompositor::kTextureUsageFlags;
    default:
      FX_NOTREACHED();
      return static_cast<vk::ImageUsageFlags>(0);
  }
}

fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>
CreateBufferCollectionPtrWithEmptyConstraints(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>();
  if (endpoints.is_error()) {
    FX_LOGS(ERROR) << "Could not create end-points: " << endpoints.status_string();
    return {};
  }

  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(token))
          .buffer_collection_request(std::move(endpoints->server))
          .Build());
  if (!result.ok()) {
    FX_LOGS(ERROR) << "Could not bind buffer collection: " << result.status_string();
    return {};
  }

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection{
      std::move(endpoints->client)};
  // Intentionally do not set the constraints field of the request.
  result = buffer_collection->SetConstraints({});
  if (!result.ok()) {
    FX_LOGS(ERROR) << "Cannot set constraints: " << result.status_string();
    return {};
  }

  return buffer_collection;
}

std::vector<vk::ImageFormatConstraintsInfoFUCHSIA> GetVulkanImageFormatConstraints(
    const BufferCollectionUsage usage, const std::optional<fuchsia_math::SizeU> size) {
  std::vector<vk::ImageFormatConstraintsInfoFUCHSIA> constraint_infos;
  for (const auto& format : GetSupportedImageFormatsForBufferCollectionUsage(usage)) {
    vk::ImageCreateInfo create_info =
        escher::RectangleCompositor::GetDefaultImageConstraints(format, GetImageUsageFlags(usage));
    if (size.has_value() && size.value().width() && size.value().height()) {
      create_info.extent = vk::Extent3D{size.value().width(), size.value().height(), 1};
    }

    constraint_infos.push_back(escher::GetDefaultImageFormatConstraintsInfo(create_info));
  }

  return constraint_infos;
}

bool IsValidImage(const allocation::ImageMetadata& metadata) {
  // The metadata can't have an invalid collection id.
  if (metadata.collection_id == allocation::kInvalidId) {
    FX_LOGS(WARNING) << "Image has invalid collection id.";
    return false;
  }

  // The metadata can't have an invalid identifier.
  if (metadata.identifier == allocation::kInvalidImageId) {
    FX_LOGS(WARNING) << "Image has invalid identifier.";
    return false;
  }

  // Check we have valid dimensions.
  if (metadata.width == 0 || metadata.height == 0) {
    FX_LOGS(WARNING) << "Image has invalid dimensions: "
                     << "(" << metadata.width << ", " << metadata.height << ").";
    return false;
  }

  return true;
}

}  // anonymous namespace

VkRenderer::VkRenderer(escher::EscherWeakPtr escher)
    : escher_(std::move(escher)),
      compositor_(escher::RectangleCompositor(escher_)),
      texture_collections_(16, &pool_resource_),
      render_target_collections_(4, &pool_resource_),
      readback_collections_(4, &pool_resource_),
      texture_map_(64, &pool_resource_),
      render_target_map_(8, &pool_resource_),
      depth_target_map_(8, &pool_resource_),
      readback_image_map_(4, &pool_resource_),
      pending_textures_(&pool_resource_),
      pending_render_targets_(&pool_resource_),
      main_dispatcher_(async_get_default_dispatcher()) {
  auto gpu_uploader = escher::BatchGpuUploader::New(escher_, /*frame_trace_number*/ 0);
  FX_DCHECK(gpu_uploader);

  texture_map_[allocation::kInvalidImageId] = CreateWhiteTexture(escher_.get(), gpu_uploader.get());
  gpu_uploader->Submit();

  {
    TRACE_DURATION("gfx", "VkRenderer::Initialize");
    WaitIdle();
  }
}

VkRenderer::~VkRenderer() {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  auto vk_device = escher_->vk_device();
  auto vk_loader = escher_->device()->dispatch_loader();
  for (auto& [_, collection] : texture_collections_) {
    vk_device.destroyBufferCollectionFUCHSIA(collection.vk_collection, nullptr, vk_loader);
  }
  for (auto& [_, collection] : render_target_collections_) {
    vk_device.destroyBufferCollectionFUCHSIA(collection.vk_collection, nullptr, vk_loader);
  }
  for (auto& [_, collection] : readback_collections_) {
    vk_device.destroyBufferCollectionFUCHSIA(collection.vk_collection, nullptr, vk_loader);
  }
}

std::optional<vk::BufferCollectionFUCHSIA>
VkRenderer::SetConstraintsAndCreateVulkanBufferCollection(
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
    const BufferCollectionUsage usage, const std::optional<fuchsia_math::SizeU> size) {
  auto vk_device = escher_->vk_device();
  auto vk_loader = escher_->device()->dispatch_loader();
  FX_DCHECK(vk_device);

  vk::BufferCollectionCreateInfoFUCHSIA bc_create_info;
  bc_create_info.collectionToken = token.TakeChannel().release();
  const vk::BufferCollectionFUCHSIA vk_collection = escher::ESCHER_CHECKED_VK_RESULT(
      vk_device.createBufferCollectionFUCHSIA(bc_create_info, nullptr, vk_loader));

  vk::ImageConstraintsInfoFUCHSIA vk_image_constraints;
  const auto image_format_constraints = GetVulkanImageFormatConstraints(usage, size);
  vk_image_constraints.setFormatConstraints(image_format_constraints)
      .setFlags(escher_->allow_protected_memory()
                    ? vk::ImageConstraintsInfoFlagBitsFUCHSIA::eProtectedOptional
                    : vk::ImageConstraintsInfoFlagsFUCHSIA{})
      .setBufferCollectionConstraints(
          vk::BufferCollectionConstraintsInfoFUCHSIA().setMinBufferCount(1u));

  if (const auto vk_result = vk_device.setBufferCollectionImageConstraintsFUCHSIA(
          vk_collection, vk_image_constraints, vk_loader);
      vk_result != vk::Result::eSuccess) {
    FX_LOGS(ERROR) << "Cannot set vulkan constraints: " << vk::to_string(vk_result)
                   << "; The client may have invalidated the token.";
    vk_device.destroyBufferCollectionFUCHSIA(vk_collection, nullptr, vk_loader);
    return std::nullopt;
  }

  return vk_collection;
}

std::optional<vk::BufferCollectionFUCHSIA> VkRenderer::GetAllocatedVulkanBufferCollection(
    const allocation::GlobalBufferCollectionId collection_id, const BufferCollectionUsage usage) {
  std::scoped_lock lock(lock_);
  // Make sure that the collection that will back this image's memory
  // is actually registered with the renderer.
  std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData>& collections =
      GetBufferCollectionsFor(usage);
  auto collection_itr = collections.find(collection_id);
  if (collection_itr == collections.end()) {
    FX_LOGS(WARNING) << "Collection with id " << collection_id << " does not exist.";
    return std::nullopt;
  }

  auto& [collection, vk_collection, is_allocated] = collection_itr->second;
  // If we've checked the allocation before we don't need to do so again.
  if (is_allocated) {
    return vk_collection;
  }

  // Check to see if the buffers are allocated and return std::nullptr if not.
  auto result = collection->CheckAllBuffersAllocated();
  if (!result.ok()) {
    FX_LOGS(WARNING) << "Collection was not allocated (FIDL error): " << result.status_string();
    return std::nullopt;
  }
  if (result->is_error()) {
    FX_LOGS(WARNING) << "Collection was not allocated (framework error): "
                     << fidl::ToUnderlying(result->error_value());
    return std::nullopt;
  }

  is_allocated = true;
  return vk_collection;
}

escher::Rectangle2D GetNormalizedUvRect(const ResolvedLayer& layer) {
  const SrcToDest& geometry = layer.geometry;
  float w = 1.f;
  float h = 1.f;
  if (std::holds_alternative<ResolvedLayer::ImageContent>(layer.content)) {
    const auto& image = std::get<ResolvedLayer::ImageContent>(layer.content);
    w = static_cast<float>(image.width);
    h = static_cast<float>(image.height);
  }
  FX_DCHECK(w > 0.f && h > 0.f);

  const auto clamp = [](float v, float lo, float hi) { return (v < lo) ? lo : (hi < v) ? hi : v; };

  const bool use_full_image = geometry.src.width() <= 0.f || geometry.src.height() <= 0.f;
  const float u0 = use_full_image ? 0.f : clamp(geometry.src.x(), 0.f, w) / w;
  const float u1 =
      use_full_image ? 1.f : clamp(geometry.src.x() + geometry.src.width(), 0.f, w) / w;
  const float v0 = use_full_image ? 0.f : clamp(geometry.src.y(), 0.f, h) / h;
  const float v1 =
      use_full_image ? 1.f : clamp(geometry.src.y() + geometry.src.height(), 0.f, h) / h;

  std::array<glm::vec2, 4> normalized_uvs;
  switch (geometry.transform.enum_value()) {
    case types::RotateFlip::Enum::kIdentity:
      normalized_uvs = {glm::vec2(u0, v0), glm::vec2(u1, v0), glm::vec2(u1, v1), glm::vec2(u0, v1)};
      break;
    case types::RotateFlip::Enum::kReflectX:
      normalized_uvs = {glm::vec2(u0, v1), glm::vec2(u1, v1), glm::vec2(u1, v0), glm::vec2(u0, v0)};
      break;
    case types::RotateFlip::Enum::kReflectY:
      normalized_uvs = {glm::vec2(u1, v0), glm::vec2(u0, v0), glm::vec2(u0, v1), glm::vec2(u1, v1)};
      break;
    case types::RotateFlip::Enum::kRotateCcw180:
      normalized_uvs = {glm::vec2(u1, v1), glm::vec2(u0, v1), glm::vec2(u0, v0), glm::vec2(u1, v0)};
      break;
    case types::RotateFlip::Enum::kRotateCcw90:
      normalized_uvs = {glm::vec2(u1, v0), glm::vec2(u1, v1), glm::vec2(u0, v1), glm::vec2(u0, v0)};
      break;
    case types::RotateFlip::Enum::kRotateCcw90ReflectX:
      normalized_uvs = {glm::vec2(u0, v0), glm::vec2(u0, v1), glm::vec2(u1, v1), glm::vec2(u1, v0)};
      break;
    case types::RotateFlip::Enum::kRotateCcw90ReflectY:
      normalized_uvs = {glm::vec2(u1, v1), glm::vec2(u1, v0), glm::vec2(u0, v0), glm::vec2(u0, v1)};
      break;
    case types::RotateFlip::Enum::kRotateCcw270:
      normalized_uvs = {glm::vec2(u0, v1), glm::vec2(u0, v0), glm::vec2(u1, v0), glm::vec2(u1, v1)};
      break;
  }

  return {glm::vec2(geometry.dest.x(), geometry.dest.y()),
          glm::vec2(geometry.dest.width(), geometry.dest.height()), normalized_uvs};
}

fpromise::promise<> VkRenderer::ImportBufferCollection(
    GlobalBufferCollectionId collection_id,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token,
    BufferCollectionUsage usage, std::optional<fuchsia_math::SizeU> size) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  FX_DCHECK(collection_id != allocation::kInvalidId);
  FX_DCHECK(buffer_collection_token.is_valid());
  const trace_flow_id_t flow_id = TRACE_NONCE();
  TRACE_DURATION("gfx", "flatland::VkRenderer::ImportBufferCollection[begin]");
  TRACE_FLOW_BEGIN("gfx", "flatland::VkRenderer::ImportBufferCollection", flow_id);

  fidl::Arena arena;
  std::array<zx_rights_t, 1> rights_attenuation_masks{ZX_RIGHT_SAME_RIGHTS};
  fidl::WireClient<fuchsia_sysmem2::BufferCollectionToken> token{std::move(buffer_collection_token),
                                                                 async_get_default_dispatcher()};
  return fidl_fpromise::as_promise<void, void>(
      token->DuplicateSync(
          fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncRequest::Builder(arena)
              .rights_attenuation_masks(
                  fidl::VectorView<zx_rights_t>::FromExternal(rights_attenuation_masks))
              .Build()),
      // TODO(https://fxbug.dev/502763366): Scenic assumes immortality of VkRenderer.
      [this, token = std::move(token), collection_id, &sysmem_allocator, usage, size, flow_id](
          auto& result, auto completer) mutable {
        TRACE_DURATION("gfx", "flatland::VkRenderer::ImportBufferCollection[end]");
        TRACE_FLOW_END("gfx", "flatland::VkRenderer::ImportBufferCollection", flow_id);
        if (!result.ok() || !result->has_tokens()) {
          FX_LOGS(ERROR) << "ImportBufferCollection failed to duplicate token: "
                         << result.status_string();
          completer.complete_error();
          return;
        }
        FX_DCHECK(result->tokens().size() == 1);
        auto vulkan_token = std::move(result->tokens()[0]);

        fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection =
            CreateBufferCollectionPtrWithEmptyConstraints(sysmem_allocator,
                                                          *token.UnbindMaybeGetEndpoint());
        if (!buffer_collection) {
          FX_LOGS(ERROR) << "ImportBufferCollection failed to create buffer collection.";
          completer.complete_error();
          return;
        }

        // Use a name with a priority that's greater than the vulkan implementation, but less
        // than what any client would use.
        fidl::Arena arena;
        fidl::OneWayStatus status = buffer_collection->SetName(
            fuchsia_sysmem2::wire::NodeSetNameRequest::Builder(arena)
                .priority(10u)
                .name(GetNextBufferCollectionIdString(GetImageName(usage)))
                .Build());
        if (!status.ok()) {
          FX_LOGS(ERROR) << "ImportBufferCollection failed to set buffer collection name.";
          completer.complete_error();
          return;
        }

        auto vk_collection =
            SetConstraintsAndCreateVulkanBufferCollection(std::move(vulkan_token), usage, size);
        if (!vk_collection) {
          FX_LOGS(ERROR) << "ImportBufferCollection failed to create vulkan buffer collection.";
          completer.complete_error();
          return;
        }

        // TODO(https://fxbug.dev/42120738): Convert this to a lock-free structure.
        std::scoped_lock lock(lock_);

        std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData>& collections =
            GetBufferCollectionsFor(usage);
        const auto [_, emplace_success] = collections.emplace(
            collection_id, CollectionData{.collection = std::move(buffer_collection),
                                          .vk_collection = *vk_collection});
        if (!emplace_success) {
          FX_LOGS(WARNING) << "ImportBufferCollection failed to store buffer collection, "
                              "because an entry already existed for "
                           << collection_id;
          auto vk_device = escher_->vk_device();
          auto vk_loader = escher_->device()->dispatch_loader();
          vk_device.destroyBufferCollectionFUCHSIA(*vk_collection, nullptr, vk_loader);
          completer.complete_error();
          return;
        }

        completer.complete_ok();
      });
}

void VkRenderer::ReleaseBufferCollection(GlobalBufferCollectionId collection_id,
                                         BufferCollectionUsage usage) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "flatland::VkRenderer::ReleaseBufferCollection");

  // TODO(https://fxbug.dev/42120738): Convert this to a lock-free structure.
  std::scoped_lock lock(lock_);

  std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData>& collections =
      GetBufferCollectionsFor(usage);
  const auto collection_itr = collections.find(collection_id);

  // If the collection is not in the map, then there's nothing to do.
  if (collection_itr == collections.end()) {
    FX_LOGS(WARNING) << "Attempting to release a non-existent buffer collection.";
    return;
  }

  auto vk_device = escher_->vk_device();
  auto vk_loader = escher_->device()->dispatch_loader();
  vk_device.destroyBufferCollectionFUCHSIA(collection_itr->second.vk_collection, nullptr,
                                           vk_loader);

  fidl::OneWayStatus result = collection_itr->second.collection->Release();
  // AttachToken failure causes ZX_ERR_PEER_CLOSED.
  if (!result.ok() && result.status() != ZX_ERR_PEER_CLOSED) {
    FX_LOGS(ERROR) << "Error when closing buffer collection: " << result.status_string();
  }

  collections.erase(collection_itr);
}

bool VkRenderer::ImageIsAlreadyRegisteredForUsage(const allocation::GlobalImageId image_id,
                                                  const BufferCollectionUsage usage) {
  std::scoped_lock lock(lock_);
  switch (usage) {
    case BufferCollectionUsage::kRenderTarget:
      return render_target_map_.find(image_id) != render_target_map_.end();
    case BufferCollectionUsage::kReadback:
      return readback_image_map_.find(image_id) != readback_image_map_.end();
    case BufferCollectionUsage::kClientImage:
      return texture_map_.find(image_id) != texture_map_.end();
    default:
      FX_NOTREACHED();
      return false;
  }
}
bool VkRenderer::ImportRenderTargetImage(const allocation::ImageMetadata& metadata,
                                         const vk::BufferCollectionFUCHSIA vk_collection) {
  bool needs_readback = false;
  {
    std::scoped_lock lock(lock_);
    needs_readback =
        readback_collections_.find(metadata.collection_id) != readback_collections_.end();
  }

  // Image usage flags need to be modified if the client needs to read back from the render target.
  const vk::ImageUsageFlags kRenderTargetReadbackFlags =
      needs_readback ? vk::ImageUsageFlagBits::eTransferSrc : vk::ImageUsageFlags();
  // TODO(https://fxbug.dev/431797024): Scenic decides at startup whether to show the debug overlay
  // or not; it cannot be toggled dynamically at runtime.  If we knew that here, we could decide to
  // omit this flag.
  const vk::ImageUsageFlags kRenderTargetDebugFontFlags = vk::ImageUsageFlagBits::eTransferDst;

  const vk::ImageUsageFlags kRenderTargetFlags =
      escher::RectangleCompositor::kRenderTargetUsageFlags | kRenderTargetReadbackFlags |
      kRenderTargetDebugFontFlags;
  const auto image = ExtractImage(metadata, BufferCollectionUsage::kRenderTarget, vk_collection,
                                  kRenderTargetFlags);
  if (!image) {
    FX_LOGS(ERROR) << "Could not extract render target.";
    return false;
  }

  image->set_swapchain_layout(vk::ImageLayout::eColorAttachmentOptimal);
  auto depth_texture = CreateDepthTexture(escher_.get(), image);

  std::scoped_lock lock(lock_);
  render_target_map_[metadata.identifier] = image;
  depth_target_map_[metadata.identifier] = std::move(depth_texture);
  pending_render_targets_.insert(metadata.identifier);
  return true;
}

bool VkRenderer::ImportReadbackImage(const allocation::ImageMetadata& metadata,
                                     const vk::BufferCollectionFUCHSIA vk_collection) {
  const escher::ImagePtr readback_image =
      ExtractImage(metadata, BufferCollectionUsage::kReadback, vk_collection,
                   vk::ImageUsageFlagBits::eTransferDst);
  if (!readback_image) {
    FX_LOGS(ERROR) << "Could not extract readback image.";
    return false;
  }

  std::scoped_lock lock(lock_);
  readback_image_map_[metadata.identifier] = readback_image;
  return true;
}

bool VkRenderer::ImportClientImage(const allocation::ImageMetadata& metadata,
                                   const vk::BufferCollectionFUCHSIA vk_collection) {
  const auto texture = ExtractTexture(metadata, vk_collection);
  if (!texture) {
    FX_LOGS(ERROR) << "Could not extract client texture image.";
    return false;
  }

  std::scoped_lock lock(lock_);
  texture_map_[metadata.identifier] = texture;
  pending_textures_.insert(metadata.identifier);
  return true;
}

fpromise::promise<> VkRenderer::ImportBufferImage(const allocation::ImageMetadata& metadata,
                                                  const BufferCollectionUsage usage) {
  TRACE_DURATION("gfx", "flatland::VkRenderer::ImportBufferImage");

  if (!IsValidImage(metadata)) {
    return fpromise::make_error_promise();
  }

  vk::BufferCollectionFUCHSIA vk_collection;
  if (const auto collection = GetAllocatedVulkanBufferCollection(metadata.collection_id, usage)) {
    vk_collection = *collection;
  } else {
    return fpromise::make_error_promise();
  }

  if (ImageIsAlreadyRegisteredForUsage(metadata.identifier, usage)) {
    FX_LOGS(WARNING) << "An image with identifier " << metadata.identifier.value()
                     << " has already been registered for usage: " << static_cast<uint32_t>(usage);
    return fpromise::make_error_promise();
  }

  bool import_result = false;
  switch (usage) {
    case BufferCollectionUsage::kRenderTarget: {
      import_result = ImportRenderTargetImage(metadata, vk_collection);
      break;
    }
    case BufferCollectionUsage::kReadback: {
      import_result = ImportReadbackImage(metadata, vk_collection);
      break;
    }
    case BufferCollectionUsage::kClientImage: {
      import_result = ImportClientImage(metadata, vk_collection);
      break;
    }
    default:
      FX_NOTREACHED();
  }
  return import_result ? fpromise::make_ok_promise() : fpromise::make_error_promise();
}

void VkRenderer::ReleaseBufferImage(allocation::GlobalImageId image_id) {
  // Called from main thread or Flatland threads.
  TRACE_DURATION("gfx", "flatland::VkRenderer::ReleaseBufferImage");
  FX_DCHECK(image_id != allocation::kInvalidImageId);

  std::scoped_lock lock(lock_);

  if (texture_map_.find(image_id) != texture_map_.end()) {
    texture_map_.erase(image_id);
    pending_textures_.erase(image_id);
  } else if (render_target_map_.find(image_id) != render_target_map_.end()) {
    render_target_map_.erase(image_id);
    depth_target_map_.erase(image_id);
    readback_image_map_.erase(image_id);
    pending_render_targets_.erase(image_id);
  }
}

escher::ImagePtr VkRenderer::ExtractImage(const allocation::ImageMetadata& metadata,
                                          const BufferCollectionUsage bc_usage,
                                          const vk::BufferCollectionFUCHSIA collection,
                                          const vk::ImageUsageFlags image_usage,
                                          const bool readback) {
  // Called from main thread or Flatland threads.
  TRACE_DURATION("gfx", "VkRenderer::ExtractImage");
  auto vk_device = escher_->vk_device();
  auto vk_loader = escher_->device()->dispatch_loader();

  // Grab the collection Properties from Vulkan.
  // TODO(https://fxbug.dev/42053219): Add unittests to cover the case where sysmem client
  // token gets invalidated when importing images.
  vk::BufferCollectionPropertiesFUCHSIA properties;
  if (const auto properties_results =
          vk_device.getBufferCollectionPropertiesFUCHSIA(collection, vk_loader);
      properties_results.result == vk::Result::eSuccess) {
    properties = properties_results.value;
  } else {
    FX_LOGS(WARNING) << "Could not get buffer collection properties: "
                     << vk::to_string(properties_results.result);
    return nullptr;
  }

  // Check the provided index against actually allocated number of buffers.
  if (properties.bufferCount <= metadata.vmo_index) {
    FX_LOGS(ERROR) << "Specified vmo index is out of bounds: " << metadata.vmo_index;
    return nullptr;
  }

  // Check if allocated buffers are backed by protected memory.
  const bool is_protected =
      (escher_->vk_physical_device()
           .getMemoryProperties()
           .memoryTypes[escher::CountTrailingZeros(properties.memoryTypeBits)]
           .propertyFlags &
       vk::MemoryPropertyFlagBits::eProtected) == vk::MemoryPropertyFlagBits::eProtected;

  // Setup the create info Fuchsia extension.
  vk::BufferCollectionImageCreateInfoFUCHSIA collection_image_info;
  collection_image_info.collection = collection;
  collection_image_info.index = metadata.vmo_index;

  // Setup the create info.
  const auto& kSupportedImageFormats = GetSupportedImageFormatsForBufferCollectionUsage(bc_usage);

  // The same list of formats was provided when specifying constraints in ImportBufferCollection();
  // |createInfoIndex| is the index into the list, in the same order that it was provided.
  FX_DCHECK(properties.createInfoIndex < std::size(kSupportedImageFormats));
  const auto pixel_format = kSupportedImageFormats[properties.createInfoIndex];
  vk::ImageCreateInfo create_info =
      escher::RectangleCompositor::GetDefaultImageConstraints(pixel_format, image_usage);
  create_info.extent = vk::Extent3D{metadata.width, metadata.height, 1};
  create_info.setPNext(&collection_image_info);
  if (is_protected) {
    create_info.flags = vk::ImageCreateFlagBits::eProtected;
  }

  // Create the VK Image, return nullptr if this fails.
  vk::Image image;
  if (const auto image_result = vk_device.createImage(create_info);
      image_result.result == vk::Result::eSuccess) {
    image = image_result.value;
  } else {
    FX_LOGS(ERROR) << "VkCreateImage failed: " << vk::to_string(image_result.result);
    return nullptr;
  }

  // Now we have to allocate VK memory for the image. This memory is going to come from
  // the imported buffer collection's vmo.
  const auto memory_requirements = vk_device.getImageMemoryRequirements(image);
  const uint32_t memory_type_index =
      escher::CountTrailingZeros(memory_requirements.memoryTypeBits & properties.memoryTypeBits);
  const vk::StructureChain<vk::MemoryAllocateInfo, vk::ImportMemoryBufferCollectionFUCHSIA,
                           vk::MemoryDedicatedAllocateInfoKHR>
      alloc_info(vk::MemoryAllocateInfo()
                     .setAllocationSize(memory_requirements.size)
                     .setMemoryTypeIndex(memory_type_index),
                 vk::ImportMemoryBufferCollectionFUCHSIA()
                     .setCollection(collection)
                     .setIndex(metadata.vmo_index),
                 vk::MemoryDedicatedAllocateInfoKHR().setImage(image));
  vk::DeviceMemory memory = nullptr;
  if (const vk::Result err =
          vk_device.allocateMemory(&alloc_info.get<vk::MemoryAllocateInfo>(), nullptr, &memory);
      err != vk::Result::eSuccess) {
    FX_LOGS(ERROR) << "Could not successfully allocate memory: " << vk::to_string(err);
    vk_device.destroyImage(image, nullptr);
    return nullptr;
  }

  // Have escher manager the memory since this is the required format for creating
  // an Escher image. Also we can now check if the total memory size is great enough
  // for the image memory requirements. If it's not big enough, the client likely
  // requested an image size that is larger than the maximum image size allowed by
  // the sysmem collection constraints.
  const auto gpu_mem =
      escher::GpuMem::AdoptVkMemory(vk_device, vk::DeviceMemory(memory), memory_requirements.size,
                                    /*needs_mapped_ptr*/ false);
  if (memory_requirements.size > gpu_mem->size()) {
    FX_LOGS(ERROR) << "Memory requirements for image exceed available memory: "
                   << memory_requirements.size << " " << gpu_mem->size();
    vk_device.destroyImage(image, nullptr);
    return nullptr;
  }

  // Create and return an escher image.
  escher::ImageInfo escher_image_info;
  escher_image_info.format = create_info.format;
  escher_image_info.width = create_info.extent.width;
  escher_image_info.height = create_info.extent.height;
  escher_image_info.usage = create_info.usage;
  escher_image_info.memory_flags = readback ? vk::MemoryPropertyFlagBits::eHostCoherent
                                            : vk::MemoryPropertyFlagBits::eDeviceLocal;
  if (create_info.flags & vk::ImageCreateFlagBits::eProtected) {
    escher_image_info.memory_flags = vk::MemoryPropertyFlagBits::eProtected;
  }
  escher_image_info.is_external = true;
  escher_image_info.color_space = escher::FromSysmemColorSpace(
      static_cast<fuchsia::sysmem::ColorSpaceType>(properties.sysmemColorSpaceIndex.colorSpace));
  return escher::impl::NaiveImage::AdoptVkImage(escher_->resource_recycler(), escher_image_info,
                                                image, std::move(gpu_mem),
                                                create_info.initialLayout);
}

escher::TexturePtr VkRenderer::ExtractTexture(const allocation::ImageMetadata& metadata,
                                              vk::BufferCollectionFUCHSIA collection) {
  // Called from main thread or Flatland threads.
  const auto image = ExtractImage(metadata, BufferCollectionUsage::kClientImage, collection,
                                  escher::RectangleCompositor::kTextureUsageFlags);
  if (!image) {
    FX_LOGS(ERROR) << "Image for texture was nullptr.";
    return nullptr;
  }

  escher::SamplerPtr sampler = escher::image_utils::IsYuvFormat(image->format())
                                   ? escher_->sampler_cache()->ObtainYuvSampler(
                                         image->format(), kDefaultFilter, image->color_space())
                                   : escher_->sampler_cache()->ObtainSampler(kDefaultFilter);
  FX_DCHECK(escher::image_utils::IsYuvFormat(image->format()) ? sampler->is_immutable()
                                                              : !sampler->is_immutable());
  return fxl::MakeRefCounted<escher::Texture>(escher_->resource_recycler(), sampler, image);
}

void VkRenderer::Render(const ImageMetadata& render_target, std::span<const ResolvedLayer> layers,
                        const RenderArgs& render_args) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "VkRenderer::Render");

  // Minimize time that `lock_` is held by making local copies of collections.
  alignas(std::max_align_t) std::array<std::byte, 8192> stack_buffer;
  std::pmr::monotonic_buffer_resource resource(stack_buffer.data(), stack_buffer.size());

  std::pmr::unordered_map<GlobalImageId, escher::TexturePtr> local_texture_map(&resource);
  std::pmr::unordered_map<GlobalImageId, escher::ImagePtr> local_render_target_map(&resource);
  std::pmr::unordered_map<GlobalImageId, escher::TexturePtr> local_depth_target_map(&resource);
  std::pmr::unordered_map<GlobalImageId, escher::ImagePtr> local_readback_image_map(&resource);
  std::pmr::set<GlobalImageId> local_pending_textures(&resource);
  std::pmr::set<GlobalImageId> local_pending_render_targets(&resource);
  {
    TRACE_DURATION("gfx", "LockAndCopyDataStructs");
    std::scoped_lock lock(lock_);
    local_texture_map.reserve(texture_map_.size());
    local_render_target_map.reserve(render_target_map_.size());
    local_depth_target_map.reserve(depth_target_map_.size());
    local_readback_image_map.reserve(readback_image_map_.size());

    local_texture_map.insert(texture_map_.begin(), texture_map_.end());
    local_render_target_map.insert(render_target_map_.begin(), render_target_map_.end());
    local_depth_target_map.insert(depth_target_map_.begin(), depth_target_map_.end());
    local_readback_image_map.insert(readback_image_map_.begin(), readback_image_map_.end());

    // `reserve()` is only necessary for unordered containers (like above), not these ordered sets.
    local_pending_textures.insert(pending_textures_.begin(), pending_textures_.end());
    local_pending_render_targets.insert(pending_render_targets_.begin(),
                                        pending_render_targets_.end());
    pending_textures_.clear();
    pending_render_targets_.clear();
  }

  // If the |render_target| is protected, we should switch to a protected escher::Frame. Otherwise,
  // we should ensure that there is no protected content in |images|.
  FX_DCHECK(local_render_target_map.find(render_target.identifier) !=
            local_render_target_map.end());
  const bool render_in_protected_mode =
      local_render_target_map.at(render_target.identifier)->use_protected_memory();

  // Escher's frame class acts as a command buffer manager that we use to create a
  // command buffer and submit it to the device queue once we are done.
  const auto frame = escher_->NewFrame(
      "flatland::VkRenderer", ++frame_number_, /*enable_gpu_logging=*/false,
      /*requested_type=*/escher::CommandBuffer::Type::kGraphics, render_in_protected_mode);
  auto command_buffer = frame->cmds();
  if (disable_lazy_pipeline_creation_) {
    command_buffer->DisableLazyPipelineCreation();
  }

  // Transition pending images to their correct layout
  // TODO(https://fxbug.dev/42129471): The way we are transitioning image layouts here and in the
  // rest of Scenic is incorrect for "external" images. It just happens to be working by luck on our
  // current hardware.
  for (auto texture_id : local_pending_textures) {
    FX_DCHECK(local_texture_map.find(texture_id) != local_texture_map.end());
    const auto texture = local_texture_map.at(texture_id);
    command_buffer->impl()->TransitionImageLayout(texture->image(), vk::ImageLayout::eUndefined,
                                                  vk::ImageLayout::eShaderReadOnlyOptimal);
  }
  for (auto target_id : local_pending_render_targets) {
    FX_DCHECK(local_render_target_map.find(target_id) != local_render_target_map.end());
    const auto target = local_render_target_map.at(target_id);
    command_buffer->impl()->TransitionImageLayout(
        target, vk::ImageLayout::eUndefined, vk::ImageLayout::eColorAttachmentOptimal,
        VK_QUEUE_FAMILY_FOREIGN_EXT, escher_->device()->vk_main_queue_family());
  }

  TRACE_DURATION_BEGIN("gfx", "VkRenderer::Render[transform_display_list]");
  std::pmr::vector<escher::Rectangle2D> normalized_rects(&resource);
  std::pmr::vector<escher::TexturePtr> textures(&resource);
  std::pmr::vector<escher::RectangleCompositor::ColorData> color_data(&resource);
  normalized_rects.reserve(layers.size());
  textures.reserve(layers.size());
  color_data.reserve(layers.size());
  for (const auto& layer : layers) {
    allocation::GlobalImageId image_id = allocation::kInvalidImageId;
    if (std::holds_alternative<ResolvedLayer::ImageContent>(layer.content)) {
      image_id = std::get<ResolvedLayer::ImageContent>(layer.content).image_id;
    }

    const auto texture_it = local_texture_map.find(image_id);
    if (texture_it == local_texture_map.end()) {
      // TODO(https://fxbug.dev/496160334): the image wasn't found, probably (hopefully) because it
      // was removed by `TrustedFlatland.ReleaseImageImmediately()`, otherwise for unknown reasons.
      // Either way, there's nothing we can do but ignore it.
      FX_LOGS(WARNING) << "VkRenderer::Render missing image: " << image_id;
      continue;
    }
    const escher::TexturePtr& texture_ptr = texture_it->second;

    normalized_rects.push_back(GetNormalizedUvRect(layer));

    // When we are not in protected mode, replace any protected content with black solid color.
    if (!render_in_protected_mode && texture_ptr->image()->use_protected_memory()) {
      textures.emplace_back(local_texture_map.at(allocation::kInvalidImageId));
      color_data.emplace_back(kProtectedReplacementColorInRGBA,
                              escher::RectangleCompositor::Opacity::Opaque);
      continue;
    }

    textures.push_back(texture_ptr);

    glm::vec4 multiply(layer.multiply_color[0], layer.multiply_color[1], layer.multiply_color[2],
                       layer.multiply_color[3]);
    if (std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content)) {
      const auto& solid = std::get<ResolvedLayer::SolidColorContent>(layer.content);
      multiply.r *= solid.color[0];
      multiply.g *= solid.color[1];
      multiply.b *= solid.color[2];
      multiply.a *= solid.color[3];
    }
    escher::RectangleCompositor::Opacity opacity;
    switch (layer.blend_mode.enum_value()) {
      case BlendMode::Enum::kReplace:
        opacity = escher::RectangleCompositor::Opacity::Opaque;
        break;
      case BlendMode::Enum::kPremultipliedAlpha:
        opacity = escher::RectangleCompositor::Opacity::Translucent;
        break;
      case BlendMode::Enum::kStraightAlpha:
        opacity = escher::RectangleCompositor::Opacity::NonPremultipliedTranslucent;
        break;
    }
    color_data.emplace_back(multiply, opacity);
  }
  TRACE_DURATION_END("gfx", "VkRenderer::Render[transform_display_list]");

  // Grab the output image and use it to generate a depth texture. The depth texture needs to
  // be the same width and height as the output image.
  const auto output_image = local_render_target_map.at(render_target.identifier);
  const auto depth_texture = local_depth_target_map.at(render_target.identifier);

  // Transition to eColorAttachmentOptimal for rendering.  Note the src queue family is FOREIGN,
  // since we assume that this image was previously presented to the display controller.
  auto render_image_layout = vk::ImageLayout::eColorAttachmentOptimal;
  command_buffer->impl()->TransitionImageLayout(output_image, vk::ImageLayout::eUndefined,
                                                render_image_layout, VK_QUEUE_FAMILY_FOREIGN_EXT,
                                                escher_->device()->vk_main_queue_family());

  // Now the compositor can finally draw.
  compositor_.DrawBatch(command_buffer, normalized_rects, textures, color_data, output_image,
                        depth_texture, render_args.apply_color_conversion);

  if (render_args.display_frame_number.has_value()) {
    // Prepare string and positioning for frame counter overlay.
    const uint64_t frame_number = render_args.display_frame_number.value();

    constexpr int32_t kGlyphScale = 4;
    const auto frame_number_string = std::to_string(frame_number);
    const int32_t x_offset = (static_cast<int32_t>(output_image->width()) -
                              (static_cast<int32_t>(frame_number_string.length()) * kGlyphScale *
                               static_cast<int32_t>(escher::DebugFont::kGlyphWidth))) /
                             2;

    // Transition the output image layout so that we can blit into it.
    command_buffer->impl()->TransitionImageLayout(
        output_image, render_image_layout, vk::ImageLayout::eTransferDstOptimal,
        escher_->device()->vk_main_queue_family(), escher_->device()->vk_main_queue_family());
    render_image_layout = vk::ImageLayout::eTransferDstOptimal;

    GetDebugFont()->Blit(command_buffer, frame_number_string, output_image, {x_offset, 40}, 4);
  }

  const auto readback_image_it = local_readback_image_map.find(render_target.identifier);
  // Copy to the readback image if there is a readback image.
  if (readback_image_it != local_readback_image_map.end()) {
    BlitRenderTarget(command_buffer, output_image, &render_image_layout, readback_image_it->second,
                     render_target);
  }

  // Having drawn, we transition to eGeneral on the FOREIGN target queue, so that we can present the
  // the image to the display controller.
  command_buffer->impl()->TransitionImageLayout(
      output_image, render_image_layout, vk::ImageLayout::eGeneral,
      escher_->device()->vk_main_queue_family(), VK_QUEUE_FAMILY_FOREIGN_EXT);

  // Create vk::semaphores from the zx::events.
  std::vector<escher::SemaphorePtr> semaphores;
  for (auto& fence_original : render_args.release_fences) {
    // Since the original fences are passed in by const reference, we
    // duplicate them here so that the duped fences can be moved into
    // the create info struct of the semaphore.
    zx::event fence_copy;
    {
      const auto status = fence_original.duplicate(ZX_RIGHT_SAME_RIGHTS, &fence_copy);
      FX_DCHECK(status == ZX_OK);
    }

    const auto sema = escher_->semaphore_pool()->AllocateAndImport(std::move(fence_copy));

    // Create a flow event that ends in the magma system driver.
    if (TRACE_ENABLED()) {
      zx_info_handle_basic_t koid_info;
      fence_original.get_info(ZX_INFO_HANDLE_BASIC, &koid_info, sizeof(koid_info), nullptr,
                              nullptr);
      TRACE_FLOW_BEGIN("gfx", "semaphore", koid_info.koid);
    }

    // TODO(https://fxbug.dev/42174813): Semaphore lifetime should be guaranteed by Escher. This
    // wait is a workaround for the issue where we destroy semaphores before they are signalled.
    if (escher_->device()->caps().is_virtual_gpu) {
      TRACE_DURATION("gfx", "VkRenderer::Render[WaitOnce]");
      auto wait = std::make_shared<async::WaitOnce>(fence_original.get(), ZX_EVENT_SIGNALED,
                                                    ZX_WAIT_ASYNC_TIMESTAMP);
      zx_status_t wait_status =
          wait->Begin(async_get_default_dispatcher(),
                      [copy_ref = wait, sema](async_dispatcher_t*, async::WaitOnce*, zx_status_t,
                                              const zx_packet_signal_t*) {
                        // Let these fall out of scope.
                      });
      FX_DCHECK(wait_status == ZX_OK);
    }

    semaphores.emplace_back(sema);
  }

  // Submit the commands and wait for them to finish.
  frame->EndFrame(semaphores, nullptr, /*skip_escher_cleanup=*/true);
}

void VkRenderer::SetColorConversionValues(const fidl::Array<float, 9>& coefficients,
                                          const fidl::Array<float, 3>& preoffsets,
                                          const fidl::Array<float, 3>& postoffsets) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  // Coefficients are ordered like this:
  // | c0 c1 c2 0 |
  // | c3 c4 c5 0 |
  // | c6 c7 c8 0 |
  // |  0  0  0 1 |
  //
  // Note: GLM uses column-major memory layout.
  // clang-format off
  const float values[16] = {coefficients[0], coefficients[3], coefficients[6], 0,
                            coefficients[1], coefficients[4], coefficients[7], 0,
                            coefficients[2], coefficients[5], coefficients[8], 0,
                                          0,               0,               0, 1};
  // clang-format on
  const glm::mat4 glm_matrix = glm::make_mat4(values);
  const glm::vec4 glm_preoffsets(preoffsets[0], preoffsets[1], preoffsets[2], 0.0);
  const glm::vec4 glm_postoffsets(postoffsets[0], postoffsets[1], postoffsets[2], 0.0);
  compositor_.SetColorConversionParams({glm_matrix, glm_preoffsets, glm_postoffsets});
}

fuchsia_images2::PixelFormat VkRenderer::ChoosePreferredRenderTargetFormat(
    const std::vector<fuchsia_images2::PixelFormat>& available_formats) const {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  for (auto preferred_format : kSupportedRenderTargetImageFormats) {
    for (fuchsia_images2::PixelFormat format : available_formats) {
      const vk::Format vk_format = ConvertToVkFormat(format);
      if (vk_format == preferred_format) {
        return format;
      }
    }
  }
  FX_DCHECK(false) << "Preferred format is not available.";
  return fuchsia_images2::PixelFormat::kInvalid;
}

bool VkRenderer::SupportsRenderInProtected() const {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  return escher_->allow_protected_memory();
}

bool VkRenderer::RequiresRenderInProtected(std::span<const ResolvedLayer> layers) const {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  std::scoped_lock lock(lock_);

  for (const auto& layer : layers) {
    if (std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content)) {
      continue;
    }
    const auto& image = std::get<ResolvedLayer::ImageContent>(layer.content);
    auto it = texture_map_.find(image.image_id);
    if (it != texture_map_.end()) {
      if (it->second->image()->use_protected_memory()) {
        return true;
      }
    } else {
      // TODO(https://fxbug.dev/496160334): the image wasn't found, probably (hopefully) because it
      // was removed by `TrustedFlatland.ReleaseImageImmediately()`, otherwise for unknown reasons.
      // Either way, it doesn't require protected render so we can safely ignore it.
      FX_LOGS(WARNING) << "VkRenderer::RequiresRenderInProtected missing image: " << image.image_id;
    }
  }
  return false;
}

bool VkRenderer::WaitIdle() {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());

  return escher_->vk_device().waitIdle() == vk::Result::eSuccess;
}

void VkRenderer::WarmPipelineCache() {
  TRACE_DURATION("gfx", "VkRenderer::WarmPipelineCache");

  for (auto output_format : kSupportedRenderTargetImageFormats) {
    auto depth_format = escher_->device()->caps().GetMatchingDepthStencilFormat().value;

    auto immutable_samplers = utils::ImmutableSamplersForShaderWarmup(escher_, kDefaultFilter);

    // Depending on the memory types provided by the Vulkan implementation, separate versions of the
    // render-passes (and therefore pipelines) may be required for protected/non-protected memory.
    // Or not; if not, then the second call will simply use the ones that are already cached.
    compositor_.WarmPipelineCache(output_format, vk::ImageLayout::eColorAttachmentOptimal,
                                  depth_format, immutable_samplers,
                                  /* use_protected_memory= */ true);
    compositor_.WarmPipelineCache(output_format, vk::ImageLayout::eColorAttachmentOptimal,
                                  depth_format, immutable_samplers,
                                  /* use_protected_memory= */ false);
  }
}

void VkRenderer::BlitRenderTarget(escher::CommandBuffer* command_buffer,
                                  const escher::ImagePtr source_image,
                                  vk::ImageLayout* source_image_layout,
                                  const escher::ImagePtr dest_image,
                                  const ImageMetadata& metadata) {
  FX_DCHECK(main_dispatcher_ == async_get_default_dispatcher());
  TRACE_DURATION("gfx", "VkRenderer::BlitRenderTarget");

  command_buffer->TransitionImageLayout(source_image, *source_image_layout,
                                        vk::ImageLayout::eTransferSrcOptimal);
  *source_image_layout = vk::ImageLayout::eTransferSrcOptimal;
  command_buffer->TransitionImageLayout(dest_image, vk::ImageLayout::eUndefined,
                                        vk::ImageLayout::eTransferDstOptimal);
  command_buffer->Blit(
      source_image, vk::Offset2D(0, 0), vk::Extent2D(metadata.width, metadata.height), dest_image,
      vk::Offset2D(0, 0), vk::Extent2D(metadata.width, metadata.height), kDefaultFilter);
}

std::pmr::unordered_map<GlobalBufferCollectionId, VkRenderer::CollectionData>&
VkRenderer::GetBufferCollectionsFor(const BufferCollectionUsage usage) {
  // Called from main thread or Flatland threads.
  switch (usage) {
    case BufferCollectionUsage::kRenderTarget:
      return render_target_collections_;
    case BufferCollectionUsage::kReadback:
      return readback_collections_;
    case BufferCollectionUsage::kClientImage:
      return texture_collections_;
    default:
      FX_NOTREACHED();
  }
}

escher::DebugFont* VkRenderer::GetDebugFont() {
  if (!debug_font_) {
    auto gpu_uploader = escher::BatchGpuUploader::New(escher_, /*frame_trace_number*/ 0);
    FX_DCHECK(gpu_uploader);

    debug_font_ = escher::DebugFont::New(gpu_uploader.get(), escher_->image_cache());
    gpu_uploader->Submit();
  }
  return debug_font_.get();
}

}  // namespace flatland
