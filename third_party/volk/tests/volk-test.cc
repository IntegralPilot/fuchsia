// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <volk.h>

#include <gtest/gtest.h>

TEST(VolkTest, InitializeSuccess) {
  VkResult init_result = volkInitialize();
  EXPECT_EQ(init_result, VK_SUCCESS);
}

TEST(VolkTest, GetVkInstanceVersion) {
  VkResult init_result = volkInitialize();
  ASSERT_EQ(init_result, VK_SUCCESS);

  uint32_t version = volkGetInstanceVersion();
  EXPECT_GE(version, VK_MAKE_VERSION(1, 0, 0));

  printf("Vulkan version %d.%d.%d initialized.\n", VK_VERSION_MAJOR(version),
         VK_VERSION_MINOR(version), VK_VERSION_PATCH(version));
}

namespace {

constexpr VkApplicationInfo kApplicationInfo = {
    .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
    .pNext = nullptr,
    .pApplicationName = "VolkTest",
    .applicationVersion = 0,
    .pEngineName = "VolkTest",
    .engineVersion = 0,
    .apiVersion = VK_API_VERSION_1_0,
};

constexpr VkInstanceCreateInfo kInstanceCreateInfo = {
    .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
    .pNext = nullptr,
    .flags = {},
    .pApplicationInfo = &kApplicationInfo,
    .enabledLayerCount = 0,
    .ppEnabledLayerNames = nullptr,
    .enabledExtensionCount = 0,
    .ppEnabledExtensionNames = nullptr,
};

constexpr float kQueuePriority = 1.0;

constexpr VkDeviceQueueCreateInfo kQueueCreateInfo = {
    .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
    .pNext = nullptr,
    .flags = 0,
    .queueFamilyIndex = 0,
    .queueCount = 1,
    .pQueuePriorities = &kQueuePriority,
};

constexpr VkDeviceCreateInfo kDeviceCreateInfo = {
    .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
    .pNext = nullptr,
    .flags = 0,
    .queueCreateInfoCount = 1,
    .pQueueCreateInfos = &kQueueCreateInfo,
    .enabledLayerCount = 0,
    .ppEnabledLayerNames = nullptr,
    .enabledExtensionCount = 0,
    .ppEnabledExtensionNames = nullptr,
    .pEnabledFeatures = nullptr,
};

}  // namespace

// Ensures that `volkLoadInstance()` loads instance function pointers to
// `VkInstance` correctly.
TEST(VolkTest, LoadInstanceFunctionPointers) {
  VkResult init_result = volkInitialize();
  ASSERT_EQ(init_result, VK_SUCCESS);

  VkInstance instance;
  VkResult create_instance_result =
      vkCreateInstance(&kInstanceCreateInfo, /*pAllocator=*/nullptr, &instance);
  ASSERT_EQ(create_instance_result, VK_SUCCESS);
  auto destroy_instance = fit::defer([&] { vkDestroyInstance(instance, /*pAllocator=*/nullptr); });

  // Load instance-provided global function pointers.
  //
  // Must be called before enumerating physical devices and creating logical
  // devices.
  volkLoadInstance(instance);

  std::vector<VkPhysicalDevice> physical_devices;

  uint32_t physical_device_count = 0;
  VkResult enumerate_physical_devices_result =
      vkEnumeratePhysicalDevices(instance, &physical_device_count, /*pPhysicalDevices=*/nullptr);
  ASSERT_EQ(enumerate_physical_devices_result, VK_SUCCESS);

  physical_devices.resize(physical_device_count);
  enumerate_physical_devices_result =
      vkEnumeratePhysicalDevices(instance, &physical_device_count, physical_devices.data());
  ASSERT_EQ(enumerate_physical_devices_result, VK_SUCCESS);

  ASSERT_FALSE(physical_devices.empty());
  VkPhysicalDeviceProperties physical_device_properties;
  vkGetPhysicalDeviceProperties(physical_devices[0], &physical_device_properties);

  printf("Physical device name: %s\n", physical_device_properties.deviceName);
}

// Ensures that `volkLoadDeviceTable()` loads device function pointers
// to `VolkDeviceTable` correctly.
TEST(VolkTest, LoadDeviceFunctionPointers) {
  VkResult init_result = volkInitialize();
  ASSERT_EQ(init_result, VK_SUCCESS);

  VkInstance instance;
  VkResult create_instance_result =
      vkCreateInstance(&kInstanceCreateInfo, /*pAllocator=*/nullptr, &instance);
  ASSERT_EQ(create_instance_result, VK_SUCCESS);
  auto destroy_instance = fit::defer([&] { vkDestroyInstance(instance, /*pAllocator=*/nullptr); });

  // Load instance-provided global function pointers.
  //
  // Must be called before enumerating physical devices and creating logical
  // devices.
  volkLoadInstance(instance);

  std::vector<VkPhysicalDevice> physical_devices;

  uint32_t physical_device_count = 0;
  VkResult enumerate_physical_devices_result =
      vkEnumeratePhysicalDevices(instance, &physical_device_count, /*pPhysicalDevices=*/nullptr);
  ASSERT_EQ(enumerate_physical_devices_result, VK_SUCCESS);

  physical_devices.resize(physical_device_count);
  enumerate_physical_devices_result =
      vkEnumeratePhysicalDevices(instance, &physical_device_count, physical_devices.data());
  ASSERT_EQ(enumerate_physical_devices_result, VK_SUCCESS);

  ASSERT_FALSE(physical_devices.empty());
  VkPhysicalDeviceProperties physical_device_properties;
  vkGetPhysicalDeviceProperties(physical_devices[0], &physical_device_properties);

  uint32_t queue_family_property_count = 0;
  vkGetPhysicalDeviceQueueFamilyProperties(physical_devices[0], &queue_family_property_count,
                                           /*pQueueFamilyProperties=*/nullptr);
  ASSERT_GT(queue_family_property_count, 0u);

  VkDevice logical_device;
  VkResult create_device_result = vkCreateDevice(physical_devices[0], &kDeviceCreateInfo,
                                                 /*pAllocator=*/nullptr, &logical_device);
  ASSERT_EQ(create_device_result, VK_SUCCESS);

  // Load device-provided function pointers to `table`.
  struct VolkDeviceTable device_table;
  volkLoadDeviceTable(&device_table, logical_device);

  EXPECT_TRUE(device_table.vkCreateImage);
  ASSERT_TRUE(device_table.vkDestroyDevice);

  device_table.vkDestroyDevice(logical_device, /*pAllocator=*/nullptr);
}
