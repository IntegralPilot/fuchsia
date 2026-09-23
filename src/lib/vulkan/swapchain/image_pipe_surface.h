// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_VULKAN_SWAPCHAIN_IMAGE_PIPE_SURFACE_H_
#define SRC_LIB_VULKAN_SWAPCHAIN_IMAGE_PIPE_SURFACE_H_

#include <unordered_map>
#include <vector>

#include <vulkan/vk_layer.h>
#include <vulkan/vulkan.h>

#include "platform_event.h"

struct VkLayerDispatchTable_;
using VkLayerDispatchTable = struct VkLayerDispatchTable_;

struct VkLayerInstanceDispatchTable_;
using VkLayerInstanceDispatchTable = struct VkLayerInstanceDispatchTable_;

struct LayerData {
  VkInstance instance = VK_NULL_HANDLE;
  uint32_t instance_version = VK_API_VERSION_1_0;
  std::unique_ptr<VkLayerDispatchTable> device_dispatch_table;
  std::unique_ptr<VkLayerInstanceDispatchTable> instance_dispatch_table;
  std::unordered_map<VkDebugUtilsMessengerEXT, VkDebugUtilsMessengerCreateInfoEXT> debug_callbacks;
  PFN_vkSetDeviceLoaderData fpSetDeviceLoaderData = nullptr;
};

struct DeviceData {
  bool protected_memory_supported = false;
};

namespace image_pipe_swapchain {

struct SupportedImageProperties {
  std::vector<VkSurfaceFormatKHR> formats;
};

// An abstract surface that must implement CreateImage, RemoveImage, and
// PresentImage.
class ImagePipeSurface {
 public:
  struct ImageInfo {
    VkImage image{};
    VkDeviceMemory memory{};
    uint32_t image_id{};
  };

  ImagePipeSurface() {}

  virtual ~ImagePipeSurface() = default;

  VkFlags SupportedUsage() {
    return VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT |
           VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_SAMPLED_BIT |
           VK_IMAGE_USAGE_STORAGE_BIT;
  }

  static VkCompositeAlphaFlagsKHR SupportedAlphaFlags() {
    return VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR | VK_COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR;
  }

  virtual bool Init() { return true; }
  virtual bool CanPresentPendingImage() { return true; }

  virtual bool GetSize(uint32_t* width_out, uint32_t* height_out) { return false; }

  virtual bool IsLost() { return false; }

  // Vulkan defines immediate as presentation that doesn't wait for the vblank period to update
  // the current image; so, switching buffers can result in tearing.
  // Importantly, with immediate update, the previous image should also become immediately
  // available for reuse, making this mode useful for measuring maximum render rate.
  //
  // Flatland and display drivers don't support unsynchronized updates, so we approximate
  // it by ignoring fences.  This allows the client to reuse an image before its release
  // semaphores are signaled, so the client doesn't wait to acquire a swapchain buffer.
  // We also send a fake acquire fence to remove any associated delays.
  //
  // The difference is in the amount of tearing:
  // - spec: less tearing, just the transition from one complete image to another
  // - implementation: more tearing, the visible image may be incomplete
  //
  virtual bool SupportsImmediatePresentMode() { return false; }

  // Adds an image resource to image pipe.
  virtual bool CreateImage(VkDevice device, VkLayerDispatchTable* pDisp, VkFormat format,
                           VkImageUsageFlags usage, VkSwapchainCreateFlagsKHR swapchain_flags,
                           VkExtent2D extent, uint32_t image_count,
                           VkCompositeAlphaFlagBitsKHR alpha_flags,
                           const VkAllocationCallbacks* pAllocator,
                           std::vector<ImageInfo>* image_info_out) = 0;
  // Removes an image resource from the pipe.
  virtual void RemoveImage(uint32_t image_id) = 0;
  // Enqueues the specified image for presentation.
  virtual void PresentImage(bool immediate, uint32_t image_id,
                            std::vector<std::unique_ptr<PlatformEvent>> acquire_fences,
                            std::vector<std::unique_ptr<PlatformEvent>> release_fences,
                            VkQueue queue) = 0;

#if defined(VK_USE_PLATFORM_FUCHSIA)
  virtual bool OnCreateSurface(VkInstance instance, VkLayerInstanceDispatchTable* dispatch_table,
                               const VkImagePipeSurfaceCreateInfoFUCHSIA* pCreateInfo,
                               const VkAllocationCallbacks* pAllocator) {
    return true;
  }
#elif defined(VK_USE_PLATFORM_WAYLAND_KHR)
  virtual bool OnCreateSurface(VkInstance instance, VkLayerInstanceDispatchTable* dispatch_table,
                               const VkWaylandSurfaceCreateInfoKHR* pCreateInfo,
                               const VkAllocationCallbacks* pAllocator) {
    return true;
  }
#endif

  virtual void OnDestroySurface(VkInstance instance, VkLayerInstanceDispatchTable* dispatch_table,
                                const VkAllocationCallbacks* pAllocator) {}

  virtual bool OnCreateSwapchain(VkDevice device, LayerData* device_layer_data,
                                 const VkSwapchainCreateInfoKHR* pCreateInfo,
                                 const VkAllocationCallbacks* pAllocator) {
    return true;
  }

  virtual void OnDestroySwapchain(VkDevice device, const VkAllocationCallbacks* pAllocator) {}

  virtual SupportedImageProperties& GetSupportedImageProperties() = 0;

  virtual VkResult GetPresentModes(VkPhysicalDevice physicalDevice,
                                   VkLayerInstanceDispatchTable* dispatch_table, uint32_t* pCount,
                                   VkPresentModeKHR* pPresentModes) {
    std::vector<VkPresentModeKHR> present_modes{VK_PRESENT_MODE_FIFO_KHR};

    if (SupportsImmediatePresentMode()) {
      present_modes.push_back(VK_PRESENT_MODE_IMMEDIATE_KHR);
    }

    if (!pPresentModes) {
      *pCount = static_cast<uint32_t>(present_modes.size());
      return VK_SUCCESS;
    }

    VkResult result = VK_SUCCESS;
    if (*pCount < present_modes.size()) {
      result = VK_INCOMPLETE;
    } else {
      *pCount = static_cast<uint32_t>(present_modes.size());
    }

    memcpy(pPresentModes, present_modes.data(), (*pCount) * sizeof(VkPresentModeKHR));
    return result;
  }

 protected:
  uint32_t next_image_id() {
    if (++next_image_id_ == 0) {
      ++next_image_id_;
    }
    return next_image_id_;
  }

 private:
  uint32_t next_image_id_ = UINT32_MAX - 1;  // Exercise rollover
};

}  // namespace image_pipe_swapchain

#endif  // SRC_LIB_VULKAN_SWAPCHAIN_IMAGE_PIPE_SURFACE_H_
