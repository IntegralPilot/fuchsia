// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/examples/vkproto/common/physical_device.h"

#include "src/graphics/examples/vkproto/common/swapchain.h"
#include "src/graphics/examples/vkproto/common/utils.h"

#include <vulkan/vulkan.hpp>

namespace {

const char *kMagmaLayer = "VK_LAYER_FUCHSIA_imagepipe_swapchain_fb";

const std::vector<const char *> s_required_physical_device_exts = {
#ifdef __Fuchsia__
    VK_FUCHSIA_EXTERNAL_MEMORY_EXTENSION_NAME,    VK_KHR_EXTERNAL_MEMORY_EXTENSION_NAME,
    VK_FUCHSIA_EXTERNAL_SEMAPHORE_EXTENSION_NAME, VK_KHR_EXTERNAL_SEMAPHORE_EXTENSION_NAME,
    VK_FUCHSIA_BUFFER_COLLECTION_EXTENSION_NAME,
#endif
};

bool ChoosePhysicalDevice(const vk::PhysicalDevice &physical_device_in, const VkSurfaceKHR &surface,
                          const vk::QueueFlags &queue_flags, bool swapchain_enabled,
                          vk::PhysicalDevice *physical_device_out) {
  auto required_physical_device_exts = s_required_physical_device_exts;
  char *default_layer = nullptr;
  if (swapchain_enabled) {
    required_physical_device_exts.push_back(VK_KHR_SWAPCHAIN_EXTENSION_NAME);
    default_layer = const_cast<char *>(kMagmaLayer);
  }
  if (!FindRequiredProperties(s_required_physical_device_exts, vkp::PHYS_DEVICE_EXT_PROP,
                              default_layer, physical_device_in)) {
    return false;
  }

  if (surface) {
    vkp::Swapchain::Info swapchain_info;
    if (!vkp::Swapchain::QuerySwapchainSupport(physical_device_in, surface, &swapchain_info)) {
      return false;
    }
  }

  if (!vkp::FindQueueFamilyIndex(physical_device_in, surface, queue_flags)) {
    RTN_MSG(false, "No matching queue families found.\n");
  }
  *physical_device_out = physical_device_in;
  return true;
}

}  // namespace

namespace vkp {
// This is a default implementation that returns no value. It's weak and can be overridden by
// config_query.cc or other implementations.
__attribute__((weak)) std::optional<uint32_t> GetGpuVendorId() { return {}; }

PhysicalDevice::PhysicalDevice(std::shared_ptr<vk::Instance> instance, const VkSurfaceKHR &surface,
                               std::optional<PhysicalDeviceProperties> properties,
                               const vk::QueueFlags &queue_flags)
    : initialized_(false),
      instance_(instance),
      surface_(surface),
      properties_(properties),
      queue_flags_(queue_flags) {}

bool PhysicalDevice::Init() {
  RTN_IF_MSG(false, initialized_, "PhysicalDevice already initialized.\n");
  RTN_IF_MSG(false, !instance_, "Instance must be initialized.\n");

  auto [r_phys_devices, phys_devices] = instance_->enumeratePhysicalDevices();
  if (vk::Result::eSuccess != r_phys_devices || phys_devices.empty()) {
    RTN_MSG(false, "VK Error: 0x%x - No physical device found.\n",
            static_cast<unsigned int>(r_phys_devices));
  }

  for (const auto &phys_device : phys_devices) {
    vk::PhysicalDeviceProperties p = phys_device.getProperties();
    // Conditionalize physical device selection based on any set properties.
    if (properties_.has_value()) {
      const char *fmt = "\tSkipping phys device: %s mismatch. Value: %d  Reqd: %d\n";
      if (properties_->api_version_.has_value() &&
          properties_->api_version_.value() != p.apiVersion) {
        printf(fmt, "api version", static_cast<int>(p.apiVersion),
               static_cast<int>(properties_->api_version_.value()));
        continue;
      }
      if (properties_->device_id_.has_value() && properties_->device_id_.value() != p.deviceID) {
        printf(fmt, "device ID", static_cast<int>(p.deviceID),
               static_cast<int>(properties_->device_id_.value()));
        continue;
      }
      if (properties_->device_type_.has_value() &&
          properties_->device_type_.value() != p.deviceType) {
        printf(fmt, "device type", static_cast<int>(p.deviceType),
               static_cast<int>(properties_->device_type_.value()));
        continue;
      }
    }
    if (auto vendor_id = GetGpuVendorId(); vendor_id) {
      if (p.vendorID != *vendor_id) {
        continue;
      }
    }
    if (ChoosePhysicalDevice(phys_device, surface_, queue_flags_, swapchain_enabled_,
                             &physical_device_)) {
      LogMemoryProperties(physical_device_);
      initialized_ = true;
      break;
    }
  }

  RTN_IF_MSG(false, !initialized_, "Failed to find a matching physical device.\n");

  return initialized_;
}

void PhysicalDevice::AppendRequiredPhysDeviceExts(std::vector<const char *> *exts,
                                                  bool swapchain_enabled) {
  exts->insert(exts->end(), s_required_physical_device_exts.begin(),
               s_required_physical_device_exts.end());
  if (swapchain_enabled) {
    exts->push_back(VK_KHR_SWAPCHAIN_EXTENSION_NAME);
  }
}

const vk::PhysicalDevice &PhysicalDevice::get() const {
  if (!initialized_) {
    RTN_MSG(physical_device_, "%s", "Request for uninitialized instance.\n");
  }
  return physical_device_;
}

PhysicalDeviceProperties::PhysicalDeviceProperties(uint32_t api_version, uint32_t device_id,
                                                   vk::PhysicalDeviceType device_type)
    : api_version_(api_version), device_id_(device_id), device_type_(device_type) {}

}  // namespace vkp
