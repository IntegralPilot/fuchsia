// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/vk/vulkan_device_queues.h"

#include <lib/syslog/cpp/macros.h>

#include <iterator>
#include <set>

#include "src/ui/lib/escher/impl/vulkan_utils.h"

#include "vulkan/vulkan_structs.hpp"

namespace escher {

VulkanDeviceQueues::Caps::Caps(vk::PhysicalDevice device) {
  {
    vk::PhysicalDeviceProperties props = device.getProperties();
    max_image_width = props.limits.maxImageDimension2D;
    max_image_height = props.limits.maxImageDimension2D;
    device_api_version = props.apiVersion;
    msaa_sample_counts =
        impl::GetSupportedColorSampleCounts(props.limits.sampledImageColorSampleCounts);
    is_virtual_gpu = props.deviceType == vk::PhysicalDeviceType::eVirtualGpu;
  }

  {
    auto formats = impl::GetSupportedDepthFormats(device, {
                                                              vk::Format::eD16Unorm,
                                                              vk::Format::eX8D24UnormPack32,
                                                              vk::Format::eD32Sfloat,
                                                              vk::Format::eS8Uint,
                                                              vk::Format::eD16UnormS8Uint,
                                                              vk::Format::eD24UnormS8Uint,
                                                              vk::Format::eD32SfloatS8Uint,
                                                          });
    depth_stencil_formats.insert(formats.begin(), formats.end());
  }
}

vk::ResultValue<vk::Format> VulkanDeviceQueues::Caps::GetMatchingDepthStencilFormat(
    const std::vector<vk::Format>& formats) const {
  for (auto fmt : formats) {
    if (depth_stencil_formats.find(fmt) != depth_stencil_formats.end()) {
      return {vk::Result::eSuccess, fmt};
    }
  }
  return {vk::Result::eErrorFeatureNotPresent, vk::Format::eUndefined};
}

vk::ResultValue<size_t> VulkanDeviceQueues::Caps::GetMatchingSampleCount(
    const std::vector<size_t>& counts) const {
  for (auto count : counts) {
    if (msaa_sample_counts.find(count) != msaa_sample_counts.end()) {
      return {vk::Result::eSuccess, count};
    }
  }
  return {vk::Result::eErrorFeatureNotPresent, 0};
}

std::set<vk::Format> VulkanDeviceQueues::Caps::GetAllMatchingDepthStencilFormats(
    const std::set<vk::Format>& formats) const {
  std::set<vk::Format> result;
  std::set_intersection(formats.begin(), formats.end(), depth_stencil_formats.begin(),
                        depth_stencil_formats.end(), std::inserter(result, result.begin()));
  return result;
}

std::set<size_t> VulkanDeviceQueues::Caps::GetAllMatchingSampleCounts(
    const std::set<size_t>& counts) const {
  std::set<size_t> result;
  std::set_intersection(counts.begin(), counts.end(), msaa_sample_counts.begin(),
                        msaa_sample_counts.end(), std::inserter(result, result.begin()));
  return result;
}

namespace {

// Helper for PopulateProcAddrs().
template <typename FuncT>
static FuncT GetDeviceProcAddr(vk::Device device, const char* func_name) {
  FuncT func = reinterpret_cast<FuncT>(device.getProcAddr(func_name));
  FX_CHECK(func) << "failed to find function address for: " << func_name;
  return func;
}

// Helper for VulkanDeviceQueues constructor.
VulkanDeviceQueues::ProcAddrs PopulateProcAddrs(vk::Device device,
                                                const VulkanDeviceQueues::Params& params) {
#define GET_DEVICE_PROC_ADDR(XXX) result.XXX = GetDeviceProcAddr<PFN_vk##XXX>(device, "vk" #XXX)

  VulkanDeviceQueues::ProcAddrs result;
  if (params.required_extension_names.find(VK_KHR_SWAPCHAIN_EXTENSION_NAME) !=
          params.required_extension_names.end() ||
      params.desired_extension_names.find(VK_KHR_SWAPCHAIN_EXTENSION_NAME) !=
          params.desired_extension_names.end()) {
    GET_DEVICE_PROC_ADDR(CreateSwapchainKHR);
    GET_DEVICE_PROC_ADDR(DestroySwapchainKHR);
    GET_DEVICE_PROC_ADDR(GetSwapchainImagesKHR);
    GET_DEVICE_PROC_ADDR(AcquireNextImageKHR);
    GET_DEVICE_PROC_ADDR(QueuePresentKHR);
  }
  return result;

#undef GET_DEVICE_PROC_ADDR
}

// Return value for FindSuitablePhysicalDeviceAndQueueFamilies(). Valid if and
// only if device is non-null.  Otherwise, no suitable device was found.
struct SuitablePhysicalDeviceAndQueueFamilies {
  vk::PhysicalDevice physical_device;
  uint32_t main_queue_family = VK_QUEUE_FAMILY_IGNORED;
  uint32_t transfer_queue_family = VK_QUEUE_FAMILY_IGNORED;
  std::set<vk::QueueGlobalPriorityEXT> main_queue_family_priorities;
};

// Helper for GatherExtensions().  There are some extension names which we disallow; return false
// if we try to use one.
bool CheckExtensionAgainstDenyList(const std::string& name) {
  // These two extensions are deprecated after being promoted to VK_KHR_global_priority.
  // Elsewhere, if the KHR extension is requested, we take additional actions, but we don't
  // if the EXT extensions are requested.  So to avoid confusion, we simply disallow the EXT
  // extensions.
  if (name == VK_EXT_GLOBAL_PRIORITY_EXTENSION_NAME ||
      name == VK_EXT_GLOBAL_PRIORITY_QUERY_EXTENSION_NAME) {
    FX_LOGS(ERROR) << "VK_EXT_global_priority/VK_EXT_global_priority_query are DISALLOWED.  "
                   << "Use VK_KHR_global_priority instead.";
    return false;
  }

  return true;
}

// Helper for GatherExtensions().
bool ValidateExtension(vk::PhysicalDevice device, const std::string& name,
                       const std::vector<vk::ExtensionProperties>& base_extensions,
                       const std::set<std::string>& required_layer_names) {
  if (!CheckExtensionAgainstDenyList(name)) {
    return false;
  }

  auto found = std::find_if(base_extensions.begin(), base_extensions.end(),
                            [&name](const vk::ExtensionProperties& extension) {
                              return !strncmp(extension.extensionName, name.c_str(),
                                              VK_MAX_EXTENSION_NAME_SIZE);
                            });
  if (found != base_extensions.end())
    return true;

  // Didn't find the extension in the base list of extensions.  Perhaps it is
  // implemented in a layer.
  for (auto& layer_name : required_layer_names) {
    auto layer_extensions =
        ESCHER_CHECKED_VK_RESULT(device.enumerateDeviceExtensionProperties(layer_name));
    auto found = std::find_if(layer_extensions.begin(), layer_extensions.end(),
                              [&name](vk::ExtensionProperties& extension) {
                                return !strncmp(extension.extensionName, name.c_str(),
                                                VK_MAX_EXTENSION_NAME_SIZE);
                              });
    if (found != layer_extensions.end())
      return true;
  }

  return false;
}

// Helper struct returned by `GatherExtensions()`.
struct Extensions {
  // Same device passed as an arg to `GatherExtensions()`.
  vk::PhysicalDevice device;
  // The requested extensions that are available on `device`.  This is the union of both required
  // and desired (optional) extensions.  The device may support additional extensions, but the
  // ones returned here are filtered to only include requested extensions.
  std::set<std::string> available;
  // Optional extensions which were requested, but are not supported by `device`.
  std::set<std::string> missing_optional;
};

// If `device` supports all of the required extensions and layers, return an `Extensions` struct
// (see the comments on the fields of that struct).  Returns nothing if there are ANY required
// extensions/layers which are not supported by `device`.
std::optional<Extensions> GatherExtensions(vk::PhysicalDevice device,
                                           const std::set<std::string>& required_extension_names,
                                           const std::set<std::string>& desired_extension_names,
                                           const std::set<std::string>& required_layer_names) {
  auto extensions = ESCHER_CHECKED_VK_RESULT(device.enumerateDeviceExtensionProperties());

  std::set<std::string> available_extension_names;
  std::set<std::string> missing_required_extension_names;
  std::set<std::string> missing_optional_extension_names;

  // Classify all required extensions as either available or missing.
  for (auto& name : required_extension_names) {
    if (ValidateExtension(device, name, extensions, required_layer_names)) {
      available_extension_names.insert(name);
    } else {
      missing_required_extension_names.insert(name);
    }
  }

  // If any required extensions were not available, log the missing ones for easy debugging, then
  // return nothing.
  if (!missing_required_extension_names.empty()) {
    FX_LOGS(INFO) << "Vulkan physical device is missing required extensions:";
    for (auto& name : missing_required_extension_names) {
      FX_LOGS(INFO) << "           - " << name;
    }
    FX_LOGS(INFO) << "... skipping.";
    return {};
  }

  // All required extensions were found.  Now check which desired/optional ones are available.
  for (auto& name : desired_extension_names) {
    if (ValidateExtension(device, name, extensions, required_layer_names)) {
      available_extension_names.insert(name);
    } else {
      missing_optional_extension_names.insert(name);
    }
  }

  return {{device, available_extension_names, missing_optional_extension_names}};
}

SuitablePhysicalDeviceAndQueueFamilies FindSuitablePhysicalDeviceAndQueueFamilies(
    const VulkanInstancePtr& instance, const VulkanDeviceQueues::Params& params) {
  auto physical_devices =
      ESCHER_CHECKED_VK_RESULT(instance->vk_instance().enumeratePhysicalDevices());

  // A suitable main queue needs to support graphics and compute.
  const auto kMainQueueFlags = vk::QueueFlagBits::eGraphics | vk::QueueFlagBits::eCompute;

  // A specialized transfer queue will only support transfer; see comment below,
  // where these flags are used.
  const auto kTransferQueueFlags =
      vk::QueueFlagBits::eTransfer | vk::QueueFlagBits::eGraphics | vk::QueueFlagBits::eCompute;

  // Look for a physical device that has both all required extensions and all desired extensions.
  // If one is found, we look no further.  If any required extensions are missing, skip that device.
  // If only optional extensions are missing, remember this device; we will use it if we fail to
  // find one that has all required/desired extensions.
  //
  // Note the weakness of this algorithm: maybe the first device is only missing one optional
  // extension, but a subsequent device doesn't support any optional extensions.  In such a case,
  // the first device might be a better choice but will not be chosen (because we only remember the
  // most recent device which supports all required extensions).  It doesn't make sense to be
  // smarter here because:
  //   - we usually have only one physical device anyway
  //   - when choosing between two devices, there are other possible metrics than the number of
  //     optional extensions that each supports.  For example, one device might have twice the VRAM
  //     or compute core, yet support less extensions.
  std::optional<Extensions> extensions;
  {
    for (auto& physical_device : physical_devices) {
      auto maybe_extensions =
          GatherExtensions(physical_device, params.required_extension_names,
                           params.desired_extension_names, instance->params().layer_names);

      if (maybe_extensions) {
        if (maybe_extensions->missing_optional.empty()) {
          // Hooray!  We found a device which supports all required and optional extensions.
          extensions = std::move(maybe_extensions);
          break;
        }
        // We'll take the first device that has required extensions.
        if (!extensions) {
          extensions = std::move(maybe_extensions);
        }
      }
    }
    if (!extensions) {
      // No devices had all of the required extensions.
      return {vk::PhysicalDevice(), VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED};
    }
    if (!extensions->missing_optional.empty()) {
      FX_LOGS(INFO)
          << "Found Vulkan physical device with all required extensions, but missing optional extensions:";
      for (auto& name : extensions->missing_optional) {
        FX_LOGS(INFO) << "           - " << name;
      }
    }
  }

  // At this point we've chosen the physical device we're going to use.
  {
    auto& physical_device = extensions->device;
    auto& extension_names = extensions->available;

    // Find the main queue family.  If none is found, continue on to the next
    // physical device.
    const bool filter_queues_for_present =
        params.surface &&
        !(params.flags & VulkanDeviceQueues::Params::kDisableQueueFilteringForPresent);
    uint32_t queue_family_count;
    physical_device.getQueueFamilyProperties2(&queue_family_count, nullptr);

    // We know how many queue families there are.  For each one, query their generic properties,
    // and also extra properties relating to global priorities (but only if the global priorities
    // extension is available).  The latter is achieved by adding an additional struct into the
    // former's pNext chain.
    std::vector<vk::QueueFamilyProperties2> properties(queue_family_count);
    std::vector<vk::QueueFamilyGlobalPriorityPropertiesKHR> global_priority_properties(
        queue_family_count);
    const bool supports_queue_global_priorities =
        std::find(extension_names.begin(), extension_names.end(),
                  VK_KHR_GLOBAL_PRIORITY_EXTENSION_NAME) != extension_names.end();
    if (supports_queue_global_priorities) {
      // For each queue family that we are querying, add a separate pNext struct to query the
      // global priority properties for that same queue family.
      for (size_t i = 0; i < queue_family_count; ++i) {
        properties[i].pNext = &global_priority_properties[i];
      }
    }
    physical_device.getQueueFamilyProperties2(&queue_family_count, properties.data());

    for (uint32_t i = 0; i < queue_family_count; ++i) {
      auto& props = properties[i].queueFamilyProperties;
      if (kMainQueueFlags == (props.queueFlags & kMainQueueFlags)) {
        if (filter_queues_for_present) {
          // TODO: it is possible that there is no queue family that supports
          // both graphics/compute and present.  In this case, we would need a
          // separate present queue.  For now, just look for a single queue that
          // meets all of our needs.
          auto get_surface_support_result =
              physical_device.getSurfaceSupportKHR(i, params.surface, instance->proc_addrs());
          FX_CHECK(get_surface_support_result.result == vk::Result::eSuccess);
          VkBool32 supports_present = get_surface_support_result.value;
          if (supports_present != VK_TRUE) {
            FX_LOGS(INFO) << "Queue supports graphics/compute, but not presentation";
            continue;
          }
        }

        // At this point, we have already succeeded.  Now, try to find the optimal transfer queue
        // family (if we can't, we'll just use the main queue for transfer operations).
        SuitablePhysicalDeviceAndQueueFamilies result;
        result.physical_device = physical_device;
        result.main_queue_family = i;
        result.transfer_queue_family = i;
        for (uint32_t j = 0; j < queue_family_count; ++j) {
          if ((props.queueFlags & kTransferQueueFlags) == vk::QueueFlagBits::eTransfer) {
            // We have found a transfer-only queue.  This is the fastest way to
            // upload data to the GPU.
            result.transfer_queue_family = j;
            break;
          }
        }

        // Remember the available priorities for the main queue family.
        for (uint32_t k = 0; k < global_priority_properties[i].priorityCount; ++k) {
          result.main_queue_family_priorities.insert(global_priority_properties[i].priorities[k]);
        }

        return result;
      }
    }
  }
  return {vk::PhysicalDevice(), VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED};
}

}  // namespace

fxl::RefPtr<VulkanDeviceQueues> VulkanDeviceQueues::New(VulkanInstancePtr instance, Params params) {
  // Escher requires the memory_requirements_2 extension for the
  // vma_gpu_allocator to function.
  params.required_extension_names.insert(VK_KHR_GET_MEMORY_REQUIREMENTS_2_EXTENSION_NAME);

  // Required for use of image barrier queue family index VK_QUEUE_FAMILY_FOREIGN_EXT
  params.required_extension_names.insert(VK_EXT_QUEUE_FAMILY_FOREIGN_EXTENSION_NAME);

  // If the params contain a surface, then ensure that the swapchain extension
  // is supported so that we can render to that surface.
  if (params.surface) {
    params.required_extension_names.insert(VK_KHR_SWAPCHAIN_EXTENSION_NAME);
  }

#if defined(OS_FUCHSIA)
  // If we're running on Fuchsia, make sure we have our semaphore extensions.
  params.required_extension_names.insert(VK_FUCHSIA_EXTERNAL_SEMAPHORE_EXTENSION_NAME);
  params.required_extension_names.insert(VK_KHR_EXTERNAL_SEMAPHORE_EXTENSION_NAME);
#endif

  vk::PhysicalDevice physical_device;
  uint32_t main_queue_family = VK_QUEUE_FAMILY_IGNORED;
  uint32_t transfer_queue_family = VK_QUEUE_FAMILY_IGNORED;
  SuitablePhysicalDeviceAndQueueFamilies chosen_device_and_queues =
      FindSuitablePhysicalDeviceAndQueueFamilies(instance, params);
  FX_CHECK(chosen_device_and_queues.physical_device)
      << "Unable to find a suitable physical device.";
  FX_CHECK(chosen_device_and_queues.main_queue_family != VK_QUEUE_FAMILY_IGNORED);
  FX_CHECK(chosen_device_and_queues.transfer_queue_family != VK_QUEUE_FAMILY_IGNORED);
  physical_device = chosen_device_and_queues.physical_device;
  main_queue_family = chosen_device_and_queues.main_queue_family;
  transfer_queue_family = chosen_device_and_queues.transfer_queue_family;

  {
    auto props = physical_device.getProperties();
    FX_LOGS(INFO) << "Using physical device: " << props.deviceName
                  << "  (apiVersion: " << VK_VERSION_MAJOR(props.apiVersion) << "."
                  << VK_VERSION_MINOR(props.apiVersion) << "." << VK_VERSION_PATCH(props.apiVersion)
                  << " driverVersion: " << VK_VERSION_MAJOR(props.driverVersion) << "."
                  << VK_VERSION_MINOR(props.driverVersion) << "."
                  << VK_VERSION_PATCH(props.driverVersion) << ")";
  }

  // Partially populate device capabilities from the physical device.
  // Other stuff (e.g. which extensions are supported) will be added below.
  VulkanDeviceQueues::Caps caps(physical_device);

  auto physical_device_features = vk::PhysicalDeviceFeatures2();
  // We first use these structs to retrieve features from the physical device, and later
  // repurpose them below by adding them to the pNext chain of `VkDeviceCreateInfo`.
  auto physical_device_memory_features = vk::PhysicalDeviceProtectedMemoryFeatures();
  auto sampler_ycbcr_conversion_features = vk::PhysicalDeviceSamplerYcbcrConversionFeatures();
  physical_device_features.setPNext(&physical_device_memory_features);
  physical_device_memory_features.setPNext(&sampler_ycbcr_conversion_features);

  physical_device.getFeatures2(&physical_device_features);

  // Get the maximum supported Vulkan API version (minimum of device version and instance version).
  uint32_t max_api_version = std::min(caps.device_api_version, instance->api_version());

  // Protected memory is only supported with Vulkan API version 1.1.
  if (!physical_device_memory_features.protectedMemory || max_api_version < VK_API_VERSION_1_1) {
    FX_LOGS(INFO) << "Protected memory is not supported.";
    caps.allow_protected_memory = false;
  } else {
    caps.allow_protected_memory = params.flags & VulkanDeviceQueues::Params::kAllowProtectedMemory;
    if (caps.allow_protected_memory) {
      FX_LOGS(INFO) << "Protected memory is enabled.";
    } else {
      FX_LOGS(INFO) << "Protected memory is supported but not enabled.";
    }
  }

  // Prepare to create the Device and Queues.
  vk::DeviceQueueCreateInfo queue_info[2];
  const float kQueuePriority = 0;
  queue_info[0] = vk::DeviceQueueCreateInfo();
  queue_info[0].queueFamilyIndex = main_queue_family;
  queue_info[0].flags = caps.allow_protected_memory ? vk::DeviceQueueCreateFlagBits::eProtected
                                                    : vk::DeviceQueueCreateFlags();
  queue_info[0].queueCount = 1;
  queue_info[0].pQueuePriorities = &kQueuePriority;
  queue_info[1] = vk::DeviceQueueCreateInfo();
  queue_info[1].queueFamilyIndex = transfer_queue_family;
  queue_info[1].queueCount = 1;
  queue_info[1].pQueuePriorities = &kQueuePriority;

  // Prepare the list of extension names that will be used to create the device.
  {
    // These extensions were already validated by FindSuitablePhysicalDeviceAndQueueFamilies();
    caps.extensions = params.required_extension_names;

    // Request as many of the desired (but optional) extensions as possible.
    auto extensions =
        ESCHER_CHECKED_VK_RESULT(physical_device.enumerateDeviceExtensionProperties());

    std::set<std::string> available_desired_extensions;
    for (auto& name : params.desired_extension_names) {
      if (ValidateExtension(physical_device, name, extensions, instance->params().layer_names)) {
        caps.extensions.insert(name);
      }
    }
  }
  std::vector<const char*> extension_names;
  for (auto& extension : caps.extensions) {
    extension_names.push_back(extension.c_str());
  }

  caps.allow_ycbcr =
      sampler_ycbcr_conversion_features.samplerYcbcrConversion &&
      caps.extensions.find(VK_KHR_SAMPLER_YCBCR_CONVERSION_EXTENSION_NAME) != caps.extensions.end();

  // Specify the required physical device features, and verify that they are all
  // supported.
  // TODO(https://fxbug.dev/42151436): instead of hard-coding the required features here, provide a
  // mechanism for Escher clients to specify additional required features.
  vk::PhysicalDeviceFeatures supported_device_features;
  physical_device.getFeatures(&supported_device_features);
  bool device_has_all_required_features = true;

#define ADD_DESIRED_FEATURE(X)                                              \
  if (supported_device_features.X) {                                        \
    caps.enabled_features.X = true;                                         \
  } else {                                                                  \
    FX_LOGS(INFO) << "Desired Vulkan Device feature not supported: " << #X; \
  }

#define ADD_REQUIRED_FEATURE(X)                                               \
  caps.enabled_features.X = true;                                             \
  if (!supported_device_features.X) {                                         \
    FX_LOGS(ERROR) << "Required Vulkan Device feature not supported: " << #X; \
    device_has_all_required_features = false;                                 \
  }

  // TODO(https://fxbug.dev/42081100): We would like to make 'shaderClipDistance' a requirement on
  // all Scenic platforms.  For now, treat it as a DESIRED_FEATURE.
  ADD_DESIRED_FEATURE(shaderClipDistance);
  ADD_DESIRED_FEATURE(fillModeNonSolid);

#undef ADD_DESIRED_FEATURE
#undef ADD_REQUIRED_FEATURE

  if (!device_has_all_required_features) {
    return fxl::RefPtr<VulkanDeviceQueues>();
  }

// Insert NEXT as the first struct in BASE's "pNext chain".  In other words, if this is called
// repeatedly, then the structs in the chain are in the reverse of the order they are added.
#define INSERT_PNEXT(BASE, NEXT)                    /* NEVER_REPLACE_NEXT */ \
  {                                                 /* NEVER_REPLACE_NEXT */ \
    (NEXT).pNext = const_cast<void*>((BASE).pNext); /* NEVER_REPLACE_NEXT */ \
    (BASE).pNext = &(NEXT);                         /* NEVER_REPLACE_NEXT */ \
  }

  // Almost ready to create the device; start populating the VkDeviceCreateInfo.
  vk::DeviceCreateInfo device_info;
  device_info.queueCreateInfoCount = 2;
  device_info.pQueueCreateInfos = queue_info;
  device_info.enabledExtensionCount = static_cast<uint32_t>(extension_names.size());
  device_info.ppEnabledExtensionNames = extension_names.data();
  device_info.pEnabledFeatures = &caps.enabled_features;
  if (caps.allow_protected_memory) {
    INSERT_PNEXT(device_info, physical_device_memory_features);
  }
  if (caps.allow_ycbcr) {
    INSERT_PNEXT(device_info, sampler_ycbcr_conversion_features);
  }

  // It's possible that the main queue and transfer queue are in the same
  // queue family.  Adjust the device-creation parameters to account for this.
  uint32_t main_queue_index = 0;
  uint32_t transfer_queue_index = 0;
  if (main_queue_family == transfer_queue_family) {
#if 0
    // TODO: it may be worthwhile to create multiple queues in the same family.
    // However, we would need to look at VkQueueFamilyProperties.queueCount to
    // make sure that we can create multiple queues for that family.  For now,
    // it is easier to share a single queue when the main/transfer queues are in
    // the same family.
    queue_info[0].queueCount = 2;
    device_info.queueCreateInfoCount = 1;
    transfer_queue_index = 1;
#else
    device_info.queueCreateInfoCount = 1;
#endif
  }

  // Try to assign a higher priority to the created queue.  This will only occur if the caller
  // requested the VK_KHR_global_priority extension.
  vk::DeviceQueueGlobalPriorityCreateInfoKHR global_priority_create_info;
  if (chosen_device_and_queues.main_queue_family_priorities.find(
          vk::QueueGlobalPriorityEXT::eRealtime) !=
      chosen_device_and_queues.main_queue_family_priorities.end()) {
    global_priority_create_info.globalPriority = vk::QueueGlobalPriorityEXT::eRealtime;
    INSERT_PNEXT(queue_info[0], global_priority_create_info);
    FX_LOGS(INFO) << "Using main queue with Realtime global priority";
  } else if (chosen_device_and_queues.main_queue_family_priorities.find(
                 vk::QueueGlobalPriorityEXT::eHigh) !=
             chosen_device_and_queues.main_queue_family_priorities.end()) {
    global_priority_create_info.globalPriority = vk::QueueGlobalPriorityEXT::eHigh;
    INSERT_PNEXT(queue_info[0], global_priority_create_info);
    FX_LOGS(INFO) << "Using main queue with High global priority";
  }

#undef INSERT_PNEXT

  // Create the device.
  auto result = physical_device.createDevice(device_info);
  if (result.result != vk::Result::eSuccess) {
    FX_LOGS(WARNING) << "Could not create Vulkan Device: " << vk::to_string(result.result) << ".";
    return fxl::RefPtr<VulkanDeviceQueues>();
  }
  vk::Device device = result.value;

  // Obtain the queues that we requested to be created with the device.
  vk::Queue main_queue;
  if (caps.allow_protected_memory) {
    auto main_queue_info2 = vk::DeviceQueueInfo2()
                                .setFlags(vk::DeviceQueueCreateFlagBits::eProtected)
                                .setQueueFamilyIndex(main_queue_family)
                                .setQueueIndex(main_queue_index);
    main_queue = device.getQueue2(main_queue_info2);
  } else {
    main_queue = device.getQueue(main_queue_family, main_queue_index);
  }

  vk::Queue transfer_queue;
  if (main_queue_family == transfer_queue_family && main_queue_index == transfer_queue_index) {
    transfer_queue = main_queue;
  } else {
    transfer_queue = device.getQueue(transfer_queue_family, transfer_queue_index);
  }

  return fxl::AdoptRef(new VulkanDeviceQueues(
      device, physical_device, main_queue, main_queue_family, transfer_queue, transfer_queue_family,
      std::move(instance), std::move(params), std::move(caps)));
}

VulkanDeviceQueues::VulkanDeviceQueues(vk::Device device, vk::PhysicalDevice physical_device,
                                       vk::Queue main_queue, uint32_t main_queue_family,
                                       vk::Queue transfer_queue, uint32_t transfer_queue_family,
                                       VulkanInstancePtr instance, Params params, Caps caps)
    : device_(device),
      physical_device_(physical_device),
      main_queue_(main_queue),
      main_queue_family_(main_queue_family),
      transfer_queue_(transfer_queue),
      transfer_queue_family_(transfer_queue_family),
      instance_(std::move(instance)),
      params_(std::move(params)),
      caps_(std::move(caps)),
      proc_addrs_(PopulateProcAddrs(device_, params_)) {
  dispatch_loader_.init(instance_->vk_instance(), device_);
}

VulkanDeviceQueues::~VulkanDeviceQueues() { device_.destroy(); }

VulkanContext VulkanDeviceQueues::GetVulkanContext() const {
  return escher::VulkanContext(instance_->vk_instance(), vk_physical_device(), vk_device(),
                               dispatch_loader(), vk_main_queue(), vk_main_queue_family(),
                               vk_transfer_queue(), vk_transfer_queue_family());
}

}  // namespace escher
