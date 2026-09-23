// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.placeholders/cpp/wire.h>
#include <lib/zx/channel.h>

#include <cstdio>
#include <cstdlib>
#include <string>

#include <vulkan/vk_icd.h>
#include <vulkan/vulkan.h>

// This file contains a fake Vulkan ICD that implements everything up to and including
// vkCreateInstance.

VKAPI_ATTR __attribute__((visibility("default"))) VkResult VKAPI_CALL
vk_icdNegotiateLoaderICDInterfaceVersion(uint32_t* pVersion) {
  fprintf(stderr, "Got icd Negotiate loader ICD interface version\n");
  *pVersion = 3;

  return VK_SUCCESS;
}

namespace {

bool open_in_namespace_callback_initialized = false;
struct Instance {
  Instance() : loader_magic(ICD_LOADER_MAGIC) {}

  // Instance is a dispatchable object, and the loader uses the first 8 bytes as a pointer. See
  // https://github.com/KhronosGroup/Vulkan-Loader/blob/master/loader/LoaderAndLayerInterface.md#icd-dispatchable-object-creation
  uintptr_t loader_magic;
};

VKAPI_ATTR VkResult VKAPI_CALL vkCreateInstance(const VkInstanceCreateInfo* pCreateInfo,
                                                const VkAllocationCallbacks* pAllocator,
                                                VkInstance* pInstance) {
  // Check that the open in namespace proc was set and was valid.
  if (!open_in_namespace_callback_initialized)
    return VK_ERROR_INITIALIZATION_FAILED;
  *pInstance = reinterpret_cast<VkInstance>(new Instance);
  return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL vkEnumerateInstanceExtensionProperties(
    const char* pLayerName, uint32_t* pPropertyCount, VkExtensionProperties* pProperties) {
  *pPropertyCount = 0;
  return VK_SUCCESS;
}

VKAPI_ATTR void VKAPI_CALL vkDestroyInstance(VkInstance instance,
                                             const VkAllocationCallbacks* pAllocator) {
  delete reinterpret_cast<Instance*>(instance);
}

VKAPI_ATTR VkResult VKAPI_CALL vkEnumerateInstanceVersion(uint32_t* pApiVersion) {
  *pApiVersion = VK_API_VERSION_1_0;
  return VK_SUCCESS;
}

VKAPI_ATTR void VKAPI_CALL vkGetPhysicalDeviceFeatures(VkPhysicalDevice physicalDevice,
                                                       VkPhysicalDeviceFeatures* pFeatures) {}

VKAPI_ATTR void VKAPI_CALL vkGetPhysicalDeviceFormatProperties(
    VkPhysicalDevice physicalDevice, VkFormat format, VkFormatProperties* pFormatProperties) {}

VKAPI_ATTR VkResult VKAPI_CALL vkGetPhysicalDeviceImageFormatProperties(
    VkPhysicalDevice physicalDevice, VkFormat format, VkImageType type, VkImageTiling tiling,
    VkImageUsageFlags usage, VkImageCreateFlags flags,
    VkImageFormatProperties* pImageFormatProperties) {
  return VK_SUCCESS;
}

VKAPI_ATTR void VKAPI_CALL vkGetPhysicalDeviceProperties(VkPhysicalDevice physicalDevice,
                                                         VkPhysicalDeviceProperties* pProperties) {}

VKAPI_ATTR void VKAPI_CALL vkGetPhysicalDeviceQueueFamilyProperties(
    VkPhysicalDevice physicalDevice, uint32_t* pQueueFamilyPropertyCount,
    VkQueueFamilyProperties* pQueueFamilyProperties) {}

VKAPI_ATTR void VKAPI_CALL vkGetPhysicalDeviceMemoryProperties(
    VkPhysicalDevice physicalDevice, VkPhysicalDeviceMemoryProperties* pMemoryProperties) {}

VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL vkGetDeviceProcAddr(VkDevice device, const char* pName) {
  return nullptr;
}

VKAPI_ATTR VkResult VKAPI_CALL vkCreateDevice(VkPhysicalDevice physicalDevice,
                                              const VkDeviceCreateInfo* pCreateInfo,
                                              const VkAllocationCallbacks* pAllocator,
                                              VkDevice* pDevice) {
  return VK_ERROR_INITIALIZATION_FAILED;
}

// Only the bare minimum Vulkan 1.0 core entrypoints are implemented. These are just enough for
// vkCreateInstance to succeed.
#define DEF_FUNCTION(name) {#name, reinterpret_cast<PFN_vkVoidFunction>(name)}
struct FunctionTable {
  std::string name;
  PFN_vkVoidFunction function;
} functions[] = {
    DEF_FUNCTION(vkCreateInstance),
    DEF_FUNCTION(vkDestroyInstance),
    DEF_FUNCTION(vkEnumerateInstanceVersion),
    DEF_FUNCTION(vkEnumerateInstanceExtensionProperties),
    DEF_FUNCTION(vkEnumeratePhysicalDevices),
    DEF_FUNCTION(vkGetPhysicalDeviceFeatures),
    DEF_FUNCTION(vkGetPhysicalDeviceFormatProperties),
    DEF_FUNCTION(vkGetPhysicalDeviceImageFormatProperties),
    DEF_FUNCTION(vkGetPhysicalDeviceProperties),
    DEF_FUNCTION(vkGetPhysicalDeviceQueueFamilyProperties),
    DEF_FUNCTION(vkGetPhysicalDeviceMemoryProperties),
    DEF_FUNCTION(vkGetDeviceProcAddr),
    DEF_FUNCTION(vkCreateDevice),
    DEF_FUNCTION(vkEnumerateDeviceExtensionProperties),
    DEF_FUNCTION(vkGetPhysicalDeviceSparseImageFormatProperties),
};

#undef DEF_FUNCTION

}  // namespace

VKAPI_ATTR __attribute__((visibility("default"))) PFN_vkVoidFunction VKAPI_CALL
vk_icdGetInstanceProcAddr(VkInstance instance, const char* pName) {
  for (auto& function : functions) {
    if (function.name == pName)
      return function.function;
  }
  return nullptr;
}

VKAPI_ATTR __attribute__((visibility("default"))) PFN_vkVoidFunction VKAPI_CALL
vk_icdGetPhysicalDeviceProcAddr(VkInstance instance, const char* pName) {
  return nullptr;
}

extern "C" {
using PFN_vkOpenInNamespaceAddr = VkResult(VKAPI_PTR*)(const char*, uint32_t);
using PFN_vkInitializeOpenInNamespaceCallbackAddr = void(VKAPI_PTR*)(PFN_vkOpenInNamespaceAddr);

using PFN_vkSetFuchsiaApiLevel = void(VKAPI_PTR*)(uint32_t);

VKAPI_ATTR void VKAPI_CALL vk_icdInitializeOpenInNamespaceCallback(PFN_vkOpenInNamespaceAddr addr);
}

VKAPI_ATTR __attribute__((visibility("default"))) void VKAPI_CALL
vk_icdInitializeOpenInNamespaceCallback(PFN_vkOpenInNamespaceAddr open_in_namespace_addr) {
  static_assert(std::is_same_v<decltype(&vk_icdInitializeOpenInNamespaceCallback),
                               PFN_vkInitializeOpenInNamespaceCallbackAddr>,
                "Function type must match callback");
  auto [client, server] = fidl::Endpoints<test_placeholders::Echo>::Create();

  // A hermetic ICD shouldn't try to access to anything from the loader service.
  if (!getenv("HERMETIC_ICD")) {
    // ConnectToDeviceFs in the fake Vulkan loader should provide an echo service at "echo".
    VkResult result =
        open_in_namespace_addr("/loader-gpu-devices/echo", server.TakeChannel().release());
    if (result != VK_SUCCESS) {
      fprintf(stderr, "Opening /loader-gpu-devices/echo failed with error %d\n", result);
      return;
    }
    // Make sure the service is being served from the DeviceFs.
    constexpr std::string_view kExpected = "Hello, world!";
    auto response = fidl::WireCall(client)->EchoString(fidl::StringView::FromExternal(kExpected));
    if (!response.ok()) {
      fprintf(stderr, "Failed to call method on service from DeviceFs: %s\n",
              response.status_string());
      return;
    }
    if (response->response.get() != kExpected) {
      fprintf(stderr, "Service from DeviceFs returned incorrect response!\n");
      return;
    }
  }

  open_in_namespace_callback_initialized = true;
}
