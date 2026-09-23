// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/vulkan/swapchain/image_pipe_surface_display.h"

#include <dirent.h>
#include <errno.h>
#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <vk_dispatch_table_helper.h>
#include <zircon/rights.h>
#include <zircon/status.h>

#include <deque>

#include <fbl/unique_fd.h>
#include <vulkan/vk_layer.h>
#include <vulkan/vulkan_fuchsia.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/vulkan/swapchain/display_coordinator_listener.h"
#include "src/lib/vulkan/swapchain/vulkan_utils.h"

namespace image_pipe_swapchain {

namespace {

const char* const kTag = "ImagePipeSurfaceDisplay";

using DisplayCoordinator = fuchsia_hardware_display::Coordinator;
using OneWayResult = fit::result<fidl::OneWayStatus>;

zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> ConnectToDisplayProvider() {
  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher;
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> watch_result =
      watcher.GetNextInstance(/*stop_at_idle=*/true);
  if (watch_result.status_value() == ZX_ERR_STOP) {
    fprintf(stderr, "fuchsia.hardware.display.Service is not available");
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  return watch_result;
}

}  // namespace

ImagePipeSurfaceDisplay::ImagePipeSurfaceDisplay()
    : client_loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
      listener_loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

ImagePipeSurfaceDisplay::~ImagePipeSurfaceDisplay() { listener_loop_.Shutdown(); }

bool ImagePipeSurfaceDisplay::Init() {
  {
    zx::result client_end = component::Connect<fuchsia_sysmem2::Allocator>();
    if (!client_end.is_ok()) {
      fprintf(stderr, "%s: Couldn't conect to fuchsia.sysmem2.Allocator: %s\n", kTag,
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
    if (!result.is_ok()) {
      fprintf(stderr, "%s: Couldn't set debug client info on fuchsia.sysmem2.Allocator: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      ConnectToDisplayProvider();
  if (provider_result.is_error()) {
    fprintf(stderr, "%s: Failed to create provider channel %d (%s)\n", kTag,
            provider_result.error_value(), provider_result.status_string());
  }
  fidl::ClientEnd<fuchsia_hardware_display::Provider> provider = std::move(provider_result).value();

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  fidl::Arena arena;
  auto open_coordinator_request =
      fuchsia_hardware_display::wire::ProviderOpenCoordinatorRequest::Builder(arena)
          .coordinator(std::move(coordinator_server))
          .coordinator_listener(std::move(listener_client))
          .priority(fuchsia_hardware_display::wire::ClientPriority{
              .value = fuchsia_hardware_display::wire::kTestUtilityClientPriorityValue})
          .Build();
  fidl::WireResult open_coordinator_result =
      fidl::WireCall(provider)->OpenCoordinator(open_coordinator_request);
  if (!open_coordinator_result.ok()) {
    fprintf(stderr, "%s: Failed to call display.Provider handle %d (%s)\n", kTag,
            open_coordinator_result.status(), open_coordinator_result.FormatDescription().c_str());
    return false;
  }
  if (open_coordinator_result.value().is_error()) {
    fprintf(stderr, "%s: Failed to open display.Coordinator %d (%s)\n", kTag,
            open_coordinator_result.value().error_value(),
            zx_status_get_string(open_coordinator_result.value().error_value()));
    return false;
  }

  display_coordinator_.Bind(std::move(coordinator_client), client_loop_.dispatcher(), this);
  display_coordinator_listener_ = std::make_unique<DisplayCoordinatorListener>(
      std::move(listener_server),
      fit::bind_member(this, &ImagePipeSurfaceDisplay::ControllerOnDisplaysChanged),
      fit::bind_member(this, &ImagePipeSurfaceDisplay::ControllerOnVsync),
      /* on_client_ownership_change= */ nullptr, *listener_loop_.dispatcher());

  while (!have_display_) {
    listener_loop_.Run(zx::time::infinite(), true);
    if (display_connection_exited_)
      return false;
  }
  listener_loop_.StartThread("ImagePipeSurfaceDisplay-coordinator-listener", nullptr);

  return true;
}

bool ImagePipeSurfaceDisplay::WaitForAsyncMessage() {
  got_message_response_ = false;
  while (!got_message_response_ && !display_connection_exited_) {
    client_loop_.Run(zx::time::infinite(), true);
  }
  return !display_connection_exited_;
}

void ImagePipeSurfaceDisplay::ControllerOnDisplaysChanged(
    std::vector<fuchsia_hardware_display::Info> infos,
    std::vector<fuchsia_hardware_display_types::DisplayId>) {
  if (infos.size() == 0)
    return;

  const fuchsia_hardware_display::Info& info = infos[0];

  width_ = info.modes()[0].active_area().width();
  height_ = info.modes()[0].active_area().height();
  display_id_ = info.id();
  max_layer_count_ = info.max_layer_count();
  std::deque<VkSurfaceFormatKHR> formats;

  for (fuchsia_images2::PixelFormat pixel_format : info.pixel_format()) {
    switch (pixel_format) {
      case fuchsia_images2::PixelFormat::kB8G8R8A8:
        formats.push_back({VK_FORMAT_B8G8R8A8_UNORM, VK_COLORSPACE_SRGB_NONLINEAR_KHR});
        formats.push_back({VK_FORMAT_B8G8R8A8_SRGB, VK_COLORSPACE_SRGB_NONLINEAR_KHR});
        break;
      case fuchsia_images2::PixelFormat::kR8G8B8A8:
        // Push front to prefer R8G8B8A8 formats.
        formats.push_front({VK_FORMAT_R8G8B8A8_SRGB, VK_COLORSPACE_SRGB_NONLINEAR_KHR});
        formats.push_front({VK_FORMAT_R8G8B8A8_UNORM, VK_COLORSPACE_SRGB_NONLINEAR_KHR});
        break;
      default:
        // Ignore unknown formats.
        break;
    }
  }
  if (formats.empty()) {
    fprintf(stderr, "OnDisplaysChanged: No pixel format available. Cannot use this display.\n");
    return;
  }
  supported_image_properties_ =
      SupportedImageProperties{.formats = {formats.begin(), formats.end()}};
  have_display_ = true;
}

void ImagePipeSurfaceDisplay::ControllerOnVsync(
    fuchsia_hardware_display_types::DisplayId, zx::time timestamp,
    fuchsia_hardware_display::ConfigStamp displayed_config_stamp,
    fuchsia_hardware_display::VsyncAckCookie cookie) {
  // Minimize the time spent holding the mutex by gathering fences to signal, but not immediately
  // signaling them.
  std::vector<zx::event> events_to_signal;
  {
    std::scoped_lock lock(mutex_);

    while (!pending_release_fences_.empty() &&
           displayed_config_stamp.value() > pending_release_fences_.front().config_stamp.value()) {
      events_to_signal.push_back(std::move(pending_release_fences_.front().release_fence));
      pending_release_fences_.pop();
    }
  }

  // Signal the events accumulated above.
  for (auto& evt : events_to_signal) {
    evt.signal(0, ZX_EVENT_SIGNALED);
  }

  // Non-zero cookies must be acknowledged immediately, others need not be acknowledged.
  // See `coordinator.fidl`.
  if (cookie.value() != 0) {
    OneWayResult result = display_coordinator_->AcknowledgeVsync(cookie.value());
    if (result.is_error()) {
      fprintf(stderr, "%s: AcknowledgeVsync failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
    }
  }
}

bool ImagePipeSurfaceDisplay::CreateImage(VkDevice device, VkLayerDispatchTable* pDisp,
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

  VkResult result;

  auto [local_token_client_end, local_token_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::SyncClient local_token(std::move(local_token_client_end));
  {
    fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest local_allocate_request;
    local_allocate_request.token_request(std::move(local_token_server_end));

    OneWayResult result =
        sysmem_allocator_->AllocateSharedCollection(std::move(local_allocate_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: AllocateSharedCollection failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  auto [vulkan_token_client, vulkan_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  {
    fuchsia_sysmem2::BufferCollectionTokenDuplicateRequest vulkan_duplicate_request;
    vulkan_duplicate_request.token_request(std::move(vulkan_token_server));
    vulkan_duplicate_request.rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);

    OneWayResult result = local_token->Duplicate(std::move(vulkan_duplicate_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: Duplicate failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  auto [display_token_client, display_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  {
    fuchsia_sysmem2::BufferCollectionTokenDuplicateRequest display_duplicate_request;
    display_duplicate_request.token_request(std::move(display_token_server));
    display_duplicate_request.rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);

    OneWayResult result = local_token->Duplicate(std::move(display_duplicate_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: Duplicate failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  {
    fit::result result = local_token->Sync();
    if (result.is_error()) {
      fprintf(stderr, "%s: Sync failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  const fuchsia_hardware_display::BufferCollectionId kBufferCollectionId(1);

  zx_status_t status = ZX_OK;
  display_coordinator_
      ->ImportBufferCollection({kBufferCollectionId, std::move(display_token_client)})
      .ThenExactlyOnce([this,
                        &status](fidl::Result<DisplayCoordinator::ImportBufferCollection>& result) {
        if (result.is_error()) {
          auto& err = result.error_value();
          status = err.is_framework_error() ? err.framework_error().status() : err.domain_error();

          fprintf(stderr, "%s: ImportBufferCollection failed: %s\n", kTag,
                  result.error_value().FormatDescription().c_str());
        }
        got_message_response_ = true;
      });
  if (!WaitForAsyncMessage()) {
    fprintf(stderr, "%s: Display Disconnected\n", kTag);
    return false;
  }
  if (status != ZX_OK) {
    return false;
  }

  const fuchsia_hardware_display_types::ImageBufferUsage image_buffer_usage{{
      .tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear,
  }};
  const fuchsia_hardware_display_types::ImageMetadata image_metadata({
      .dimensions = fuchsia_math::SizeU({.width = extent.width, .height = extent.height}),
      .tiling_type = image_buffer_usage.tiling_type(),
  });

  display_coordinator_->SetBufferCollectionConstraints({kBufferCollectionId, image_buffer_usage})
      .ThenExactlyOnce(
          [this,
           &status](fidl::Result<DisplayCoordinator::SetBufferCollectionConstraints>& result) {
            if (result.is_error()) {
              auto& err = result.error_value();
              status =
                  err.is_framework_error() ? err.framework_error().status() : err.domain_error();
              fprintf(stderr, "%s: SetBufferCollectionConstraints failed: %s\n", kTag,
                      result.error_value().FormatDescription().c_str());
            }
            got_message_response_ = true;
          });

  if (!WaitForAsyncMessage()) {
    fprintf(stderr, "%s: Display Disconnected\n", kTag);
    return false;
  }
  if (status != ZX_OK) {
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
      .queueFamilyIndexCount = 0,      // not used since not sharing
      .pQueueFamilyIndices = nullptr,  // not used since not sharing
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

  VkBufferCollectionCreateInfoFUCHSIA import_info = {
      .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_CREATE_INFO_FUCHSIA,
      .pNext = nullptr,
      .collectionToken = vulkan_token_client.TakeChannel().release(),
  };
  VkBufferCollectionFUCHSIA collection;
  result = pDisp->CreateBufferCollectionFUCHSIA(device, &import_info, pAllocator, &collection);
  if (result != VK_SUCCESS) {
    fprintf(stderr, "%s: Failed to import buffer collection: %d\n", kTag, result);
    return false;
  }

  result = pDisp->SetBufferCollectionImageConstraintsFUCHSIA(device, collection,
                                                             &image_constraints_info);

  if (result != VK_SUCCESS) {
    fprintf(stderr, "%s: Failed to import buffer collection: %d\n", kTag, result);
    return false;
  }

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
  // 1000 should override the generic Magma name.
  {
    constexpr uint32_t kNamePriority = 1000u;
    const char* kImageName = "ImagePipeSurfaceDisplay";
    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.name(kImageName);
    set_name_request.priority(kNamePriority);
    OneWayResult result = sysmem_collection->SetName(std::move(set_name_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: SetName failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    constraints.min_buffer_count(image_count);
    // Used because every constraints need to have a usage.
    constraints.usage(
        std::move(fuchsia_sysmem2::BufferUsage().display(fuchsia_sysmem2::kDisplayUsageLayer)));

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest constraints_request;
    constraints_request.constraints(std::move(constraints));

    OneWayResult result = sysmem_collection->SetConstraints(std::move(constraints_request));
    if (result.is_error()) {
      fprintf(stderr, "%s: SetConstraints failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  auto wait_for_all_buffers_allocated_result = sysmem_collection->WaitForAllBuffersAllocated();
  if (wait_for_all_buffers_allocated_result.is_error()) {
    fprintf(stderr, "%s: WaitForBuffersAllocated failed: %s\n", kTag,
            wait_for_all_buffers_allocated_result.error_value().FormatDescription().c_str());
    return false;
  }

  {
    OneWayResult result = sysmem_collection->Release();
    if (result.is_error()) {
      fprintf(stderr, "%s: Release failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  ZX_ASSERT(wait_for_all_buffers_allocated_result.value().buffer_collection_info().has_value());
  auto& buffer_collection_info =
      wait_for_all_buffers_allocated_result.value().buffer_collection_info().value();
  ZX_ASSERT(buffer_collection_info.buffers().has_value());
  if (buffer_collection_info.buffers()->size() != image_count) {
    fprintf(stderr, "%s: incorrect image count %lu allocated vs. %d requested\n", kTag,
            buffer_collection_info.buffers()->size(), image_count);
    return false;
  }

  for (uint32_t i = 0; i < image_count; ++i) {
    VkExternalMemoryImageCreateInfo external_image_create_info = {
        .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
        .pNext = nullptr,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_ZIRCON_VMO_BIT_FUCHSIA,
    };
    VkBufferCollectionImageCreateInfoFUCHSIA image_format_fuchsia = {
        .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_IMAGE_CREATE_INFO_FUCHSIA,
        .pNext = &external_image_create_info,
        .collection = collection,
        .index = i};
    image_create_info.pNext = &image_format_fuchsia;

    VkImage image;
    result = pDisp->CreateImage(device, &image_create_info, pAllocator, &image);
    if (result != VK_SUCCESS) {
      fprintf(stderr, "%s: vkCreateImage failed: %d\n", kTag, result);
      return false;
    }

    VkMemoryRequirements memory_requirements;
    pDisp->GetImageMemoryRequirements(device, image, &memory_requirements);

    VkBufferCollectionPropertiesFUCHSIA properties = {
        .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_PROPERTIES_FUCHSIA};
    result = pDisp->GetBufferCollectionPropertiesFUCHSIA(device, collection, &properties);
    if (result != VK_SUCCESS) {
      fprintf(stderr, "%s: GetBufferCollectionPropertiesFUCHSIA failed: %d\n", kTag, status);
      return false;
    }

    // Find lowest usable index.
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
    result = pDisp->AllocateMemory(device, &alloc_info, pAllocator, &memory);
    if (result != VK_SUCCESS) {
      fprintf(stderr, "%s: vkAllocateMemory failed: %d\n", kTag, result);
      return result;
    }
    result = pDisp->BindImageMemory(device, image, memory, 0);
    if (result != VK_SUCCESS) {
      fprintf(stderr, "%s: vkBindImageMemory failed: %d\n", kTag, result);
      return result;
    }

    uint32_t image_id = next_image_id();
    {
      const fuchsia_hardware_display::ImageId fidl_image_id(image_id);
      display_coordinator_->ImportImage({image_metadata, kBufferCollectionId, i, fidl_image_id})
          .ThenExactlyOnce([this, &status](fidl::Result<DisplayCoordinator::ImportImage>& result) {
            if (result.is_error()) {
              auto& err = result.error_value();
              status =
                  err.is_framework_error() ? err.framework_error().status() : err.domain_error();

              fprintf(stderr, "%s: ImportVmoImage failed: %s\n", kTag,
                      result.error_value().FormatDescription().c_str());
            }
            got_message_response_ = true;
          });
    }
    if (!WaitForAsyncMessage()) {
      return false;
    }
    if (status != ZX_OK) {
      return false;
    }

    ImageInfo info = {.image = image, .memory = memory, .image_id = image_id};

    image_info_out->push_back(info);

    image_ids.insert(image_id);
  }

  pDisp->DestroyBufferCollectionFUCHSIA(device, collection, pAllocator);

  {
    OneWayResult result = display_coordinator_->ReleaseBufferCollection({kBufferCollectionId});
    if (result.is_error()) {
      fprintf(stderr, "%s: ReleaseBufferCollection failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  // Assert that `CreateImage()` hasn't already been called.  If it has, the current implementation
  // won't properly clean up resources from previous `CreateImage()` calls.
  if (layer_id_.value() != fuchsia_hardware_display_types::kInvalidDispId) {
    fprintf(stderr,
            "%s: CreateImage() stomping existing layer_id_; see https://fxbug.dev/374201213\n",
            kTag);
  }

  const fuchsia_hardware_display::LayerId layer_id(next_layer_id_);
  ++next_layer_id_;
  display_coordinator_->CreateLayer({layer_id})
      .ThenExactlyOnce([this, layer_id,
                        &status](fidl::Result<DisplayCoordinator::CreateLayer>& result) {
        if (result.is_ok()) {
          layer_id_ = layer_id;
          status = ZX_OK;
        } else {
          layer_id_ =
              fuchsia_hardware_display::LayerId{fuchsia_hardware_display_types::kInvalidDispId};
          auto& err = result.error_value();
          status = err.is_framework_error() ? err.framework_error().status() : err.domain_error();

          fprintf(stderr, "%s: CreateLayer failed: %s\n", kTag,
                  result.error_value().FormatDescription().c_str());
        }
        got_message_response_ = true;
      });
  if (!WaitForAsyncMessage()) {
    return false;
  }
  if (status != ZX_OK) {
    return false;
  }

  std::vector<fuchsia_hardware_display::LayerId> layers = {layer_id_};
  if (layers.size() > max_layer_count_) {
    fprintf(stderr, "%s: Hardware layer limit (%u) is insufficient (requested %zu)\n", kTag,
            max_layer_count_, layers.size());
    return false;
  }

  {
    OneWayResult result = display_coordinator_->SetDisplayLayers({display_id_, std::move(layers)});
    if (result.is_error()) {
      fprintf(stderr, "%s: SetDisplayLayers failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  {
    OneWayResult result = display_coordinator_->SetLayerPrimaryConfig({layer_id_, image_metadata});
    if (result.is_error()) {
      fprintf(stderr, "%s: SetLayerPrimaryConfig failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return false;
    }
  }

  return true;
}

bool ImagePipeSurfaceDisplay::GetSize(uint32_t* width_out, uint32_t* height_out) {
  *width_out = width_;
  *height_out = height_;
  return true;
}

void ImagePipeSurfaceDisplay::RemoveImage(uint32_t image_id) {
  auto iter = image_ids.find(image_id);
  if (iter != image_ids.end()) {
    image_ids.erase(iter);
  }
}

void ImagePipeSurfaceDisplay::PresentImage(
    bool immediate, uint32_t image_id, std::vector<std::unique_ptr<PlatformEvent>> acquire_fences,
    std::vector<std::unique_ptr<PlatformEvent>> release_fences, VkQueue queue) {
  ZX_ASSERT(!immediate);
  ZX_ASSERT(acquire_fences.size() <= 1);
  ZX_ASSERT(release_fences.size() <= 1);

  auto iter = image_ids.find(image_id);
  if (iter == image_ids.end()) {
    fprintf(stderr, "%s::PresentImage: can't find image_id %u\n", kTag, image_id);
    return;
  }

  fuchsia_hardware_display::EventId wait_event_id = {
      fuchsia_hardware_display_types::kInvalidDispId};
  if (acquire_fences.size()) {
    zx::event event = static_cast<FuchsiaEvent*>(acquire_fences[0].get())->Take();

    zx_info_handle_basic_t info;
    zx_status_t status =
        event.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    if (status != ZX_OK) {
      fprintf(stderr, "%s: failed to get event id: %d\n", kTag, status);
      return;
    }
    wait_event_id.value(info.koid);
    OneWayResult result = display_coordinator_->ImportEvent({std::move(event), wait_event_id});
    if (result.is_error()) {
      fprintf(stderr, "%s: ImportEvent failed for acquire fence: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      return;
    }
  }

  // image_id is also used in DisplayController interface.
  const fuchsia_hardware_display::ImageId fidl_image_id = {image_id};
  {
    OneWayResult result = display_coordinator_->SetLayerImage2(
        {{.layer_id = layer_id_, .image_id = fidl_image_id, .wait_event_id = wait_event_id}});

    if (result.is_error()) {
      fprintf(stderr, "%s: SetLayerImage2 failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      // Note: don't return on failure, because we want to release fences afterward.
    }
  }

  const fuchsia_hardware_display::ConfigStamp new_config_stamp = NextConfigStamp();
  {
    std::scoped_lock lock(mutex_);

    // Apply the config while the mutex is locked.  This avoids a race condition where the vsync for
    // this config could be received before we enqueue the pending release fences below.
    fuchsia_hardware_display::CoordinatorCommitConfigRequest request;
    request.stamp(new_config_stamp);
    OneWayResult result = display_coordinator_->CommitConfig(std::move(request));
    if (result.is_error()) {
      fprintf(stderr, "%s: ApplyConfig failed: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
      // Note: don't return on failure, because we want to release fences afterward.
    }

    // If there are unsignaled release fences, we want them to be released when the just-applied
    // config is latched.
    if (!release_fences.empty()) {
      // We only handle 1 fence, so if more are provided then we wouldn't behave as expected.
      zx::event release_fence = static_cast<FuchsiaEvent*>(release_fences[0].get())->Take();
      ZX_ASSERT(release_fences.size() == 1);
      pending_release_fences_.push(
          {.config_stamp = new_config_stamp, .release_fence = std::move(release_fence)});
    }
  }

  if (wait_event_id.value() != fuchsia_hardware_display_types::kInvalidDispId) {
    OneWayResult result = display_coordinator_->ReleaseEvent(wait_event_id);
    if (result.is_error()) {
      fprintf(stderr, "%s: ReleaseEvent failed for wait event: %s\n", kTag,
              result.error_value().FormatDescription().c_str());
    }
  }
}

SupportedImageProperties& ImagePipeSurfaceDisplay::GetSupportedImageProperties() {
  return supported_image_properties_;
}

void ImagePipeSurfaceDisplay::on_fidl_error(fidl::UnbindInfo error) {
  display_connection_exited_ = true;
}

}  // namespace image_pipe_swapchain
