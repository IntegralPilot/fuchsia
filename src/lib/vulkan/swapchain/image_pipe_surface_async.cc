// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "image_pipe_surface_async.h"

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>
#include <vk_dispatch_table_helper.h>

#include <vulkan/vulkan_core.h>

// May need to fall back to `buffer_collection_token` instead of `buffer_collection_token2`
// in `Flatland.RegisterBufferCollectionArgs()`.
#include <zircon/availability.h>

#if !FUCHSIA_API_LEVEL_AT_LEAST(25)
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#endif

#include <cinttypes>
#include <string>

#include <vulkan/vk_layer.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/vulkan/swapchain/vulkan_utils.h"

static fidl::ClientEnd<fuchsia_ui_composition::Allocator> allocator_endpoint_for_test;
static fidl::ClientEnd<fuchsia_ui_composition::Flatland> flatland_endpoint_for_test;

extern "C" {
__attribute__((visibility("default"))) bool imagepipe_initialize_service_channel(
    zx::channel allocator_endpoint, zx::channel flatland_endpoint) {
  allocator_endpoint_for_test =
      fidl::ClientEnd<fuchsia_ui_composition::Allocator>(std::move(allocator_endpoint));
  flatland_endpoint_for_test =
      fidl::ClientEnd<fuchsia_ui_composition::Flatland>(std::move(flatland_endpoint));
  return true;
}
}

namespace image_pipe_swapchain {

namespace {

const char* const kTag = "ImagePipeSurfaceAsync";
const fuchsia_ui_composition::TransformId kRootTransform = {1};

const std::string DEBUG_NAME =
    fsl::GetCurrentProcessName() + "-" + std::to_string(fsl::GetCurrentProcessKoid());

const std::string PER_APP_PRESENT_TRACING_NAME = "Flatland::PerAppPresent[" + DEBUG_NAME + "]";

using OneWayResult = fit::result<fidl::OneWayStatus>;

}  // namespace

ImagePipeSurfaceAsync::~ImagePipeSurfaceAsync() {
  async::PostTask(loop_.dispatcher(), [this] {
    // flatland_ and flatland_allocator_ are thread hostile so they must be turned down on the
    // thread that they are used on.
    flatland_connection_.reset();
    if (fit::result result = flatland_allocator_.UnbindMaybeGetEndpoint(); result.is_error()) {
      fprintf(stderr, "%s: Couldn't unbind to fuchsia.ui.composition.Allocator: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
    }
    loop_.Quit();
  });

  loop_.JoinThreads();
}

bool ImagePipeSurfaceAsync::Init() {
  {
    zx::result client_end = component::Connect<fuchsia_sysmem2::Allocator>();
    if (!client_end.is_ok()) {
      fprintf(stderr, "%s: Couldn't connect to fuchsia.sysmem2.Allocator: %s\n", kTag,
              client_end.status_string());
      return false;
    }
    sysmem_allocator_.Bind(std::move(*client_end));
  }

  {
    OneWayResult result = sysmem_allocator_->SetDebugClientInfo(
        std::move(fuchsia_sysmem2::AllocatorSetDebugClientInfoRequest()
                      .name(fsl::GetCurrentProcessName())
                      .id(fsl::GetCurrentProcessKoid())));
    if (result.is_error()) {
      // Fatal because this is a one-way method, therefore subsequent calls would fail too.
      fprintf(stderr, "%s: Couldn't initialize fuchsia.sysmem2.Allocator: %s\n", kTag,
              result.error_value().status_string());
      return false;
    }
  }

  async::PostTask(loop_.dispatcher(), [this] {
    auto error_catcher = fit::defer([this] {
      std::lock_guard<std::mutex> lock(mutex_);
      OnErrorLocked();
    });

    if (!view_creation_token_.value().is_valid()) {
      fprintf(stderr, "%s: ViewCreationToken is invalid.\n", kTag);
      return;
    }

    if (allocator_endpoint_for_test.is_valid()) {
      flatland_allocator_.Bind(std::move(allocator_endpoint_for_test), loop_.dispatcher());
    } else {
      zx::result allocator_client_end = component::Connect<fuchsia_ui_composition::Allocator>();
      if (allocator_client_end.is_error()) {
        fprintf(stderr, "%s: Couldn't connect to fuchsia.ui.composition.Allocator: %s\n", kTag,
                allocator_client_end.status_string());
        return;
      }

      flatland_allocator_.Bind(std::move(allocator_client_end.value()), loop_.dispatcher());
    }

    fidl::ClientEnd<fuchsia_ui_composition::Flatland> flatland_client_end;
    if (flatland_endpoint_for_test.is_valid()) {
      flatland_client_end = std::move(flatland_endpoint_for_test);
    } else {
      zx::result result = component::Connect<fuchsia_ui_composition::Flatland>();
      if (result.is_error()) {
        fprintf(stderr, "%s: Couldn't connect to fuchsia.ui.composition.Flatland: %s\n", kTag,
                result.status_string());
        return;
      }
      flatland_client_end = std::move(result.value());
    }

    flatland_connection_ = simple_present::FlatlandConnection::Create(
        loop_.dispatcher(), std::move(flatland_client_end), kTag);

    flatland_connection_->SetErrorCallback([this]() {
      std::lock_guard<std::mutex> lock(mutex_);
      OnErrorLocked();
    });

    // This Flatland doesn't need input or any hit regions, so CreateView() is used instead of
    // CreateView2().
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::Endpoints<fuchsia_ui_composition::ParentViewportWatcher>::Create();
    OneWayResult result = FlatlandClient()->CreateView(
        {std::move(view_creation_token_), std::move(parent_viewport_watcher_server_end)});
    result = FlatlandClient()->CreateTransform(kRootTransform);
    result = FlatlandClient()->SetRootTransform(kRootTransform);
    result = FlatlandClient()->SetDebugName(DEBUG_NAME);
    if (result.is_error()) {
      fprintf(stderr, "%s: Couldn't set up Flatland view and root transform: %s\n", kTag,
              result.error_value().status_string());
      return;
    }

    error_catcher.cancel();
  });

  return true;
}

bool ImagePipeSurfaceAsync::CreateImage(VkDevice device, VkLayerDispatchTable* pDisp,
                                        VkFormat format, VkImageUsageFlags usage,
                                        VkSwapchainCreateFlagsKHR swapchain_flags,
                                        VkExtent2D extent, uint32_t image_count,
                                        VkCompositeAlphaFlagBitsKHR alpha_flags,
                                        const VkAllocationCallbacks* pAllocator,
                                        std::vector<ImageInfo>* image_info_out) {
  // To create BufferCollection, the image must have a valid format.
  if (format == VK_FORMAT_UNDEFINED) {
    fprintf(stderr, "%s: Invalid format: %d\n", kTag, format);
    return false;
  }

  // Allocate token for BufferCollection.
  auto [local_token_client_end, local_token_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::SyncClient local_token{std::move(local_token_client_end)};
  {
    fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest local_allocate_request;
    local_allocate_request.token_request(std::move(local_token_server_end));

    OneWayResult result =
        sysmem_allocator_->AllocateSharedCollection(std::move(local_allocate_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: AllocateSharedCollection failed: %s\n", kTag,
              result.error_value().status_string());
      return false;
    }
  }

  // Duplicate tokens to pass around.
  fuchsia_sysmem2::BufferCollectionTokenDuplicateSyncRequest token_duplicate_request;
  token_duplicate_request.rights_attenuation_masks(
      std::vector<zx_rights_t>{ZX_RIGHT_SAME_RIGHTS, ZX_RIGHT_SAME_RIGHTS});
  auto token_duplicate_result = local_token->DuplicateSync(std::move(token_duplicate_request));
  if (token_duplicate_result.is_error()) {
    fprintf(stderr, "%s: DuplicateSync failed: %s\n", kTag,
            token_duplicate_result.error_value().status_string());
    return false;
  }

  auto scenic_token = std::move(token_duplicate_result->tokens()->at(0));
  auto vulkan_token = std::move(token_duplicate_result->tokens()->at(1));

  fuchsia_ui_composition::BufferCollectionExportToken export_token;
  fuchsia_ui_composition::BufferCollectionImportToken import_token;
  zx_status_t status = zx::eventpair::create(0, &export_token.value(), &import_token.value());
  if (status != ZX_OK) {
    fprintf(stderr, "%s: Eventpair create failed: %d\n", kTag, status);
    return false;
  }

  async::PostTask(loop_.dispatcher(), [this, scenic_token = std::move(scenic_token),
                                       export_token = std::move(export_token)]() mutable {
    // Pass |scenic_token| to Scenic to collect constraints.
    if (flatland_allocator_.is_valid()) {
      fuchsia_ui_composition::RegisterBufferCollectionArgs args{};
      args.export_token(std::move(export_token));
#if FUCHSIA_API_LEVEL_AT_LEAST(25)
      args.buffer_collection_token2(std::move(scenic_token));
#else
      args.buffer_collection_token(fidl::ClientEnd<fuchsia_sysmem::BufferCollectionToken>(
          std::move(scenic_token).TakeChannel()));
#endif
      args.usage(fuchsia_ui_composition::RegisterBufferCollectionUsage::kDefault);
      flatland_allocator_->RegisterBufferCollection(std::move(args))
          .ThenExactlyOnce(
              [this](fidl::Result<fuchsia_ui_composition::Allocator::RegisterBufferCollection>&
                         result) {
                if (result.is_error()) {
                  fprintf(stderr, "%s: Flatland Allocator registration failed.\n", kTag);
                  std::lock_guard<std::mutex> lock(mutex_);
                  OnErrorLocked();
                }
              });
    }
  });

  // Set swapchain constraints |vulkan_token|.
  VkBufferCollectionCreateInfoFUCHSIA import_info = {
      .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_CREATE_INFO_FUCHSIA,
      .pNext = nullptr,
      .collectionToken = vulkan_token.TakeChannel().release(),
  };
  VkBufferCollectionFUCHSIA collection;
  VkResult vk_result =
      pDisp->CreateBufferCollectionFUCHSIA(device, &import_info, pAllocator, &collection);
  if (vk_result != VK_SUCCESS) {
    fprintf(stderr, "Failed to import buffer collection: %d\n", vk_result);
    return false;
  }
  uint32_t image_flags = 0;
  if (swapchain_flags & VK_SWAPCHAIN_CREATE_MUTABLE_FORMAT_BIT_KHR)
    image_flags |= VK_IMAGE_CREATE_MUTABLE_FORMAT_BIT;
  if (swapchain_flags & VK_SWAPCHAIN_CREATE_PROTECTED_BIT_KHR)
    image_flags |= VK_IMAGE_CREATE_PROTECTED_BIT;
  VkImageCreateInfo image_create_info = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
      .pNext = nullptr,
      .flags = image_flags,
      .imageType = VK_IMAGE_TYPE_2D,
      .format = format,
      .extent = VkExtent3D{extent.width, extent.height, 1},
      .mipLevels = 1,
      .arrayLayers = 1,
      .samples = VK_SAMPLE_COUNT_1_BIT,
      .tiling = VK_IMAGE_TILING_OPTIMAL,
      .usage = usage,
      .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
      .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
  };
  const VkSysmemColorSpaceFUCHSIA kSrgbColorSpace = {
      .sType = VK_STRUCTURE_TYPE_SYSMEM_COLOR_SPACE_FUCHSIA,
      .pNext = nullptr,
      .colorSpace = static_cast<uint32_t>(fuchsia_images2::ColorSpace::kSrgb)};
  const VkSysmemColorSpaceFUCHSIA kYuvColorSpace = {
      .sType = VK_STRUCTURE_TYPE_SYSMEM_COLOR_SPACE_FUCHSIA,
      .pNext = nullptr,
      .colorSpace = static_cast<uint32_t>(fuchsia_images2::ColorSpace::kRec709)};

  VkImageFormatConstraintsInfoFUCHSIA format_info = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_FORMAT_CONSTRAINTS_INFO_FUCHSIA,
      .pNext = nullptr,
      .imageCreateInfo = image_create_info,
      .requiredFormatFeatures = GetFormatFeatureFlagsFromUsage(usage),
      .sysmemPixelFormat = 0u,
      .colorSpaceCount = 1,
      .pColorSpaces = IsYuvFormat(format) ? &kYuvColorSpace : &kSrgbColorSpace,
  };
  VkImageConstraintsInfoFUCHSIA image_constraints_info = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_CONSTRAINTS_INFO_FUCHSIA,
      .pNext = nullptr,
      .formatConstraintsCount = 1,
      .pFormatConstraints = &format_info,
      .bufferCollectionConstraints =
          VkBufferCollectionConstraintsInfoFUCHSIA{
              .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_CONSTRAINTS_INFO_FUCHSIA,
              .pNext = nullptr,
              .minBufferCount = 1,
              .maxBufferCount = 0,
              .minBufferCountForCamping = 0,
              .minBufferCountForDedicatedSlack = 0,
              .minBufferCountForSharedSlack = 0,
          },
      .flags = 0u,
  };

  vk_result = pDisp->SetBufferCollectionImageConstraintsFUCHSIA(device, collection,
                                                                &image_constraints_info);
  if (vk_result != VK_SUCCESS) {
    fprintf(stderr, "Failed to set buffer collection constraints: %d\n", vk_result);
    return false;
  }

  // Set |image_count| constraints on the |local_token|.
  auto [sysmem_collection_client, sysmem_collection_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::SyncClient sysmem_collection(std::move(sysmem_collection_client));
  {
    fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_shared_collection_request;
    bind_shared_collection_request.token({local_token.TakeClientEnd()});
    bind_shared_collection_request.buffer_collection_request(std::move(sysmem_collection_server));

    OneWayResult result =
        sysmem_allocator_->BindSharedCollection(std::move(bind_shared_collection_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: BindSharedCollection failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }
  {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    constraints.min_buffer_count(image_count);
    constraints.usage(std::move(
        fuchsia_sysmem2::BufferUsage().vulkan(fuchsia_sysmem2::kVulkanImageUsageSampled)));

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest constraints_request;
    constraints_request.constraints(std::move(constraints));

    OneWayResult result = sysmem_collection->SetConstraints(std::move(constraints_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: SetConstraints failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  // Wait for buffer to be allocated.
  auto wait_for_all_buffers_allocated_result = sysmem_collection->WaitForAllBuffersAllocated();
  if (wait_for_all_buffers_allocated_result.is_error()) {
    fprintf(stderr, "%s: WaitForBuffersAllocated failed: %s\n", kTag,
            wait_for_all_buffers_allocated_result.error_value().FormatDescription().c_str());
    return false;
  }

  ZX_ASSERT(wait_for_all_buffers_allocated_result.value().buffer_collection_info().has_value());
  auto& buffer_collection_info =
      wait_for_all_buffers_allocated_result.value().buffer_collection_info().value();
  ZX_ASSERT(buffer_collection_info.buffers().has_value());
  if (buffer_collection_info.buffers()->size() != image_count) {
    fprintf(stderr, "%s: incorrect image count %" PRIu64 " allocated vs. %d requested\n", kTag,
            buffer_collection_info.buffers()->size(), image_count);
    return false;
  }

  // Insert width and height information while adding images because it wasn't passed in
  // AddBufferCollection().
  fuchsia_images2::ImageFormat image_format = {};
  image_format.size(fuchsia_math::SizeU{extent.width, extent.height});

  for (uint32_t i = 0; i < image_count; ++i) {
    // Create Vk image.
    VkBufferCollectionImageCreateInfoFUCHSIA image_format_fuchsia = {
        .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_IMAGE_CREATE_INFO_FUCHSIA,
        .pNext = nullptr,
        .collection = collection,
        .index = i};
    image_create_info.pNext = &image_format_fuchsia;
    VkImage image;
    vk_result = pDisp->CreateImage(device, &image_create_info, pAllocator, &image);
    if (vk_result != VK_SUCCESS) {
      fprintf(stderr, "%s: vkCreateImage failed: %d\n", kTag, vk_result);
      return false;
    }

    // Extract memory handles from BufferCollection.
    VkMemoryRequirements memory_requirements;
    pDisp->GetImageMemoryRequirements(device, image, &memory_requirements);
    VkBufferCollectionPropertiesFUCHSIA properties = {
        .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_PROPERTIES_FUCHSIA};
    vk_result = pDisp->GetBufferCollectionPropertiesFUCHSIA(device, collection, &properties);
    if (vk_result != VK_SUCCESS) {
      fprintf(stderr, "%s: GetBufferCollectionPropertiesFUCHSIA failed: %d\n", kTag, status);
      return false;
    }
    uint32_t memory_type_index =
        __builtin_ctz(memory_requirements.memoryTypeBits & properties.memoryTypeBits);
    VkMemoryDedicatedAllocateInfoKHR dedicated_info = {
        .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO_KHR,
        .image = image,
    };
    VkImportMemoryBufferCollectionFUCHSIA import_info = {
        .sType = VK_STRUCTURE_TYPE_IMPORT_MEMORY_BUFFER_COLLECTION_FUCHSIA,
        .pNext = &dedicated_info,
        .collection = collection,
        .index = i,
    };
    VkMemoryAllocateInfo alloc_info{
        .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
        .pNext = &import_info,
        .allocationSize = memory_requirements.size,
        .memoryTypeIndex = memory_type_index,
    };
    VkDeviceMemory memory;
    vk_result = pDisp->AllocateMemory(device, &alloc_info, pAllocator, &memory);
    if (vk_result != VK_SUCCESS) {
      fprintf(stderr, "%s: vkAllocateMemory failed: %d\n", kTag, vk_result);
      return false;
    }
    vk_result = pDisp->BindImageMemory(device, image, memory, 0);
    if (vk_result != VK_SUCCESS) {
      fprintf(stderr, "%s: vkBindImageMemory failed: %d\n", kTag, vk_result);
      return false;
    }

    ImageInfo info = {
        .image = image,
        .memory = memory,
        .image_id = next_image_id(),
    };
    image_info_out->push_back(info);

    fuchsia_ui_composition::BufferCollectionImportToken import_token_dup;
    zx_status_t status =
        import_token.value().duplicate(ZX_RIGHT_SAME_RIGHTS, &import_token_dup.value());
    if (status != ZX_OK) {
      fprintf(stderr, "%s: Duplicate failed: %d\n", kTag, status);
      return false;
    }
    async::PostTask(loop_.dispatcher(), [this, info, import_token_dup = std::move(import_token_dup),
                                         i, extent, alpha_flags]() mutable {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!channel_closed_) {
        OneWayResult result = fit::ok();

        fuchsia_ui_composition::ImageProperties image_properties;
        image_properties.size(fuchsia_math::SizeU{extent.width, extent.height});
        result = FlatlandClient()->CreateImage(
            {{info.image_id}, std::move(import_token_dup), i, std::move(image_properties)});

        result = FlatlandClient()->SetImageDestinationSize(
            {{info.image_id}, {extent.width, extent.height}});

        assert(alpha_flags == VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR ||
               alpha_flags == VK_COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR);
        auto blend_mode = alpha_flags == VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR
                              ? fuchsia_ui_composition::BlendMode::kSrc
                              : fuchsia_ui_composition::BlendMode::kSrcOver;

        result = FlatlandClient()->SetImageBlendingFunction({{info.image_id}, blend_mode});

        if (result.is_error()) {
          fprintf(stderr, "%s: Failed to create image and configure size/blending: %s\n", kTag,
                  result.error_value().FormatDescription().c_str());
          OnErrorLocked();
          return;
        }
      }
    });
  }

  pDisp->DestroyBufferCollectionFUCHSIA(device, collection, pAllocator);
  OneWayResult result = sysmem_collection->Release();
  if (result.is_error()) {
    fprintf(stderr, "%s: Release failed: %s\n", kTag,
            result.error_value().FormatDescription().c_str());
    return false;
  }

  return true;
}

bool ImagePipeSurfaceAsync::IsLost() {
  std::lock_guard<std::mutex> lock(mutex_);
  return channel_closed_;
}

void ImagePipeSurfaceAsync::RemoveImage(uint32_t image_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  for (auto iter = queue_.begin(); iter != queue_.end();) {
    if (iter->image_id == image_id) {
      iter = queue_.erase(iter);
    } else {
      iter++;
    }
  }

  async::PostTask(loop_.dispatcher(), [this, image_id]() {
    std::lock_guard<std::mutex> lock(mutex_);
    if (!channel_closed_) {
      OneWayResult result = FlatlandClient()->ReleaseImage({image_id});
      if (result.is_error()) {
        fprintf(stderr, "%s: ReleaseImage failed: %s\n", kTag,
                result.error_value().FormatDescription().c_str());
        OnErrorLocked();
      }
    }
  });
}

void ImagePipeSurfaceAsync::PresentImage(bool immediate, uint32_t image_id,
                                         std::vector<std::unique_ptr<PlatformEvent>> acquire_fences,
                                         std::vector<std::unique_ptr<PlatformEvent>> release_fences,
                                         VkQueue queue) {
  std::lock_guard<std::mutex> lock(mutex_);
  TRACE_FLOW_BEGIN("gfx", "image_pipe_swapchain_to_present", image_id);

  std::vector<std::unique_ptr<FenceSignaler>> release_fence_signalers;
  release_fence_signalers.reserve(release_fences.size());

  for (auto& fence : release_fences) {
    zx::event event = static_cast<FuchsiaEvent*>(fence.get())->Take();
    release_fence_signalers.push_back(std::make_unique<FenceSignaler>(std::move(event)));
  }

  if (channel_closed_)
    return;

  std::vector<zx::event> acquire_events;
  acquire_events.reserve(acquire_fences.size());
  for (auto& fence : acquire_fences) {
    zx::event event = static_cast<FuchsiaEvent*>(fence.get())->Take();
    acquire_events.push_back(std::move(event));
  }

  queue_.push_back({image_id, std::move(acquire_events), std::move(release_fence_signalers)});

  async::PostTask(loop_.dispatcher(), [this, immediate]() {
    std::lock_guard<std::mutex> lock(mutex_);
    PresentNextImageLocked(immediate);
  });
}

SupportedImageProperties& ImagePipeSurfaceAsync::GetSupportedImageProperties() {
  return supported_image_properties_;
}

void ImagePipeSurfaceAsync::PresentNextImageLocked(bool immediate) {
  if (queue_.empty())
    return;
  TRACE_DURATION("gfx", "ImagePipeSurfaceAsync::PresentNextImageLocked");

  auto& present = queue_.front();
  TRACE_FLOW_END("gfx", "image_pipe_swapchain_to_present", present.image_id);

  TRACE_FLOW_BEGIN("gfx", PER_APP_PRESENT_TRACING_NAME.c_str(), present.image_id);
  if (!channel_closed_) {
    std::vector<zx::event> release_events;
    release_events.reserve(present.release_fences.size());
    for (auto& signaler : present.release_fences) {
      zx::event event;
      signaler->event().duplicate(ZX_RIGHT_SAME_RIGHTS, &event);
      release_events.push_back(std::move(event));
    }

    // In Flatland, release fences apply to the content of the previous present. Keeping track of
    // the previous frame's release fences and swapping ensure we set the correct ones. When the
    // current frame's OnFramePresented callback is called, it is safe to stop tracking the
    // previous frame's |release_fences|.
    previous_present_release_fence_signalers_.swap(present.release_fences);

    bool squashable = immediate;

    // To guarantee FIFO mode, we can't have Scenic drop any of our frames.
    // We accomplish that by setting unsquashable flag.
    fuchsia_ui_composition::PresentArgs present_args;
    // Use 0 as the requested presentation time to present as soon as possible.
    present_args.requested_presentation_time(0)
        .acquire_fences(std::move(present.acquire_fences))
        .release_fences(std::move(release_events))
        .unsquashable(!squashable);

    OneWayResult result = FlatlandClient()->SetContent({kRootTransform, {present.image_id}});
    if (result.is_error()) {
      fprintf(stderr, "%s: SetContent failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
    }
    flatland_connection_->Present(
        std::move(present_args),
        // Called on the async loop.
        [this, immediate,
         release_fences = std::move(present.release_fences)](zx_time_t actual_presentation_time) {
          std::lock_guard<std::mutex> lock(mutex_);
          for (auto& fence : release_fences) {
            fence->reset();
          }
          PresentNextImageLocked(immediate);
        });
  }

  queue_.erase(queue_.begin());
}

void ImagePipeSurfaceAsync::OnErrorLocked() {
  channel_closed_ = true;
  queue_.clear();
  flatland_connection_.reset();
  previous_present_release_fence_signalers_.clear();
}

}  // namespace image_pipe_swapchain
