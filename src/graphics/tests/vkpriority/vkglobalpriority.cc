// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <chrono>
#include <future>
#include <thread>
#include <vector>

#include <gtest/gtest.h>
#include <vulkan/vulkan.h>

#define PRINT_STDERR(format, ...) \
  fprintf(stderr, "%s:%d " format "\n", __FILE__, __LINE__, ##__VA_ARGS__)

namespace {

struct Device {
  VkDevice device;
  VkQueue queue;
  VkCommandPool command_pool;
  VkCommandBuffer command_buffer;
};

class VkGlobalPriorityTest {
 public:
  VkGlobalPriorityTest() = default;

  bool Initialize();
  bool Exec();

 private:
  bool InitVulkan();
  bool InitCommandPool();
  bool InitCommandBuffer(Device* device);

  bool is_initialized_ = false;
  VkPhysicalDevice vk_physical_device_;

  Device high_prio_device_;
  Device low_prio_device_;
};

bool VkGlobalPriorityTest::Initialize() {
  if (is_initialized_)
    return false;

  if (!InitVulkan()) {
    PRINT_STDERR("failed to initialize Vulkan");
    return false;
  }

  if (!InitCommandPool()) {
    PRINT_STDERR("InitCommandPool failed");
    return false;
  }

  if (!InitCommandBuffer(&low_prio_device_)) {
    PRINT_STDERR("InitImage failed");
    return false;
  }

  if (!InitCommandBuffer(&high_prio_device_)) {
    PRINT_STDERR("InitImage failed");
    return false;
  }

  is_initialized_ = true;

  return true;
}

bool VkGlobalPriorityTest::InitVulkan() {
  VkInstanceCreateInfo create_info{
      VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,  // VkStructureType             sType;
      nullptr,                                 // const void*                 pNext;
      0,                                       // VkInstanceCreateFlags       flags;
      nullptr,                                 // const VkApplicationInfo*    pApplicationInfo;
      0,                                       // uint32_t                    enabledLayerCount;
      nullptr,                                 // const char* const*          ppEnabledLayerNames;
      0,                                       // uint32_t                    enabledExtensionCount;
      nullptr,  // const char* const*          ppEnabledExtensionNames;
  };
  VkAllocationCallbacks* allocation_callbacks = nullptr;
  VkInstance instance;
  VkResult result;

  if ((result = vkCreateInstance(&create_info, allocation_callbacks, &instance)) != VK_SUCCESS) {
    PRINT_STDERR("vkCreateInstance failed %d", result);
    return false;
  }

  uint32_t physical_device_count;
  if ((result = vkEnumeratePhysicalDevices(instance, &physical_device_count, nullptr)) !=
      VK_SUCCESS) {
    PRINT_STDERR("vkEnumeratePhysicalDevices failed %d", result);
    return false;
  }

  if (physical_device_count < 1) {
    PRINT_STDERR("unexpected physical_device_count %d", physical_device_count);
    return false;
  }

  std::vector<VkPhysicalDevice> physical_devices(physical_device_count);
  if ((result = vkEnumeratePhysicalDevices(instance, &physical_device_count,
                                           physical_devices.data())) != VK_SUCCESS) {
    PRINT_STDERR("vkEnumeratePhysicalDevices failed %d", result);
    return false;
  }

  VkPhysicalDeviceProperties properties;
  vkGetPhysicalDeviceProperties(physical_devices[0], &properties);

  uint32_t queue_family_count;
  vkGetPhysicalDeviceQueueFamilyProperties(physical_devices[0], &queue_family_count, nullptr);

  if (queue_family_count < 1) {
    PRINT_STDERR("invalid queue_family_count %d", queue_family_count);
    return false;
  }

  std::vector<VkQueueFamilyProperties> queue_family_properties(queue_family_count);
  vkGetPhysicalDeviceQueueFamilyProperties(physical_devices[0], &queue_family_count,
                                           queue_family_properties.data());

  int32_t queue_family_index = -1;
  for (uint32_t i = 0; i < queue_family_count; i++) {
    if (queue_family_properties[i].queueFlags & VK_QUEUE_COMPUTE_BIT) {
      queue_family_index = i;
      break;
    }
  }

  if (queue_family_index < 0) {
    PRINT_STDERR("couldn't find an appropriate queue");
    return false;
  }

  if (queue_family_properties[queue_family_index].queueCount < 2) {
    PRINT_STDERR("Need 2 queues to use priorities");
    return false;
  }

  VkQueueGlobalPriority priorities[2] = {VK_QUEUE_GLOBAL_PRIORITY_MEDIUM_EXT,
                                         VK_QUEUE_GLOBAL_PRIORITY_LOW_EXT};
  Device* devices[2] = {&high_prio_device_, &low_prio_device_};

  for (int i = 0; i < 2; i++) {
    VkDeviceQueueGlobalPriorityCreateInfoEXT global_create_info{
        .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_GLOBAL_PRIORITY_CREATE_INFO_EXT,
        .pNext = nullptr,
        .globalPriority = priorities[i],
    };

    float queue_priorities[1] = {1.0};
    VkDeviceQueueCreateInfo queue_create_info = {
        .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
        .pNext = &global_create_info,
        .flags = 0,
        .queueFamilyIndex = 0,
        .queueCount = 1,
        .pQueuePriorities = queue_priorities};

    std::vector<const char*> enabled_extension_names{VK_EXT_GLOBAL_PRIORITY_EXTENSION_NAME};

    VkDeviceCreateInfo createInfo = {
        .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
        .pNext = nullptr,
        .flags = 0,
        .queueCreateInfoCount = 1,
        .pQueueCreateInfos = &queue_create_info,
        .enabledLayerCount = 0,
        .ppEnabledLayerNames = nullptr,
        .enabledExtensionCount = static_cast<uint32_t>(enabled_extension_names.size()),
        .ppEnabledExtensionNames = enabled_extension_names.data(),
        .pEnabledFeatures = nullptr};
    VkDevice vkdevice;

    if ((result = vkCreateDevice(physical_devices[0], &createInfo,
                                 nullptr /* allocationcallbacks */, &vkdevice)) != VK_SUCCESS) {
      PRINT_STDERR("vkCreateDevice failed: %d", result);
      return false;
    }

    devices[i]->device = vkdevice;
    vkGetDeviceQueue(vkdevice, queue_family_index, 0, &devices[i]->queue);
  }
  vk_physical_device_ = physical_devices[0];

  return true;
}

bool VkGlobalPriorityTest::InitCommandPool() {
  VkCommandPoolCreateInfo command_pool_create_info = {
      .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
      .pNext = nullptr,
      .flags = 0,
      .queueFamilyIndex = 0,
  };
  VkResult result;
  if ((result = vkCreateCommandPool(high_prio_device_.device, &command_pool_create_info, nullptr,
                                    &high_prio_device_.command_pool)) != VK_SUCCESS) {
    PRINT_STDERR("vkCreateCommandPool failed: %d", result);
    return false;
  }
  if ((result = vkCreateCommandPool(low_prio_device_.device, &command_pool_create_info, nullptr,
                                    &low_prio_device_.command_pool)) != VK_SUCCESS) {
    PRINT_STDERR("vkCreateCommandPool failed: %d", result);
    return false;
  }
  return true;
}

bool VkGlobalPriorityTest::InitCommandBuffer(Device* device) {
  VkCommandBufferAllocateInfo command_buffer_create_info = {
      .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
      .pNext = nullptr,
      .commandPool = device->command_pool,
      .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
      .commandBufferCount = 1};
  VkResult result;
  if ((result = vkAllocateCommandBuffers(device->device, &command_buffer_create_info,
                                         &device->command_buffer)) != VK_SUCCESS) {
    PRINT_STDERR("vkAllocateCommandBuffers failed: %d", result);
    return false;
  }

  VkCommandBufferBeginInfo begin_info = {
      .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
      .pNext = nullptr,
      .flags = 0,
      .pInheritanceInfo = nullptr,  // ignored for primary buffers
  };
  if ((result = vkBeginCommandBuffer(device->command_buffer, &begin_info)) != VK_SUCCESS) {
    PRINT_STDERR("vkBeginCommandBuffer failed: %d", result);
    return false;
  }

  VkShaderModule compute_shader_module_;
  VkShaderModuleCreateInfo sh_info = {};
  sh_info.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;

#include "priority.comp.h"
  sh_info.codeSize = sizeof(priority_comp);
  sh_info.pCode = priority_comp;
  if ((result = vkCreateShaderModule(device->device, &sh_info, NULL, &compute_shader_module_)) !=
      VK_SUCCESS) {
    PRINT_STDERR("vkCreateShaderModule failed: %d", result);
    return false;
  }

  VkPipelineLayout layout;

  VkPipelineLayoutCreateInfo pipeline_create_info = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
      .pNext = nullptr,
      .flags = 0,
      .setLayoutCount = 0,
      .pSetLayouts = nullptr,
      .pushConstantRangeCount = 0,
      .pPushConstantRanges = nullptr};

  if ((result = vkCreatePipelineLayout(device->device, &pipeline_create_info, nullptr, &layout)) !=
      VK_SUCCESS) {
    PRINT_STDERR("vkCreatePipelineLayout failed: %d", result);
    return false;
  }

  VkPipeline compute_pipeline;

  VkComputePipelineCreateInfo pipeline_info = {
      .sType = VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO,
      .pNext = nullptr,
      .flags = 0,
      .stage = {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                .pNext = nullptr,
                .flags = 0,
                .stage = VK_SHADER_STAGE_COMPUTE_BIT,
                .module = compute_shader_module_,
                .pName = "main",
                .pSpecializationInfo = nullptr},
      .layout = layout,
      .basePipelineHandle = VK_NULL_HANDLE,
      .basePipelineIndex = 0};

  if ((result = vkCreateComputePipelines(device->device, VK_NULL_HANDLE, 1, &pipeline_info, nullptr,
                                         &compute_pipeline)) != VK_SUCCESS) {
    PRINT_STDERR("vkCreateComputePipelines failed: %d", result);
    return false;
  }

  vkCmdBindPipeline(device->command_buffer, VK_PIPELINE_BIND_POINT_COMPUTE, compute_pipeline);
  vkCmdDispatch(device->command_buffer, 1000, 1000, 10);

  if ((result = vkEndCommandBuffer(device->command_buffer)) != VK_SUCCESS) {
    PRINT_STDERR("vkEndCommandBuffer failed: %d", result);
    return false;
  }

  return true;
}

bool VkGlobalPriorityTest::Exec() {
  VkResult result;
  result = vkQueueWaitIdle(low_prio_device_.queue);
  if (result != VK_SUCCESS) {
    PRINT_STDERR("vkQueueWaitIdle failed with result %d", result);
    return false;
  }
  result = vkQueueWaitIdle(high_prio_device_.queue);
  if (result != VK_SUCCESS) {
    PRINT_STDERR("vkQueueWaitIdle failed with result %d", result);
    return false;
  }

  // Submit multiple times so we have multiple commands to schedule.
  constexpr int kCommitNum = 10;
  VkSubmitInfo submit_info[kCommitNum];
  for (int i = 0; i < kCommitNum; i++) {
    submit_info[i] = {
        .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
        .pNext = nullptr,
        .waitSemaphoreCount = 0,
        .pWaitSemaphores = nullptr,
        .pWaitDstStageMask = nullptr,
        .commandBufferCount = 1,
        .pCommandBuffers = &low_prio_device_.command_buffer,
        .signalSemaphoreCount = 0,
        .pSignalSemaphores = nullptr,
    };
  }

  auto low_prio_start_time = std::chrono::steady_clock::now();
  if ((result = vkQueueSubmit(low_prio_device_.queue, kCommitNum, submit_info, VK_NULL_HANDLE)) !=
      VK_SUCCESS) {
    PRINT_STDERR("vkQueueSubmit failed: %d", result);
    return false;
  }

  std::chrono::steady_clock::time_point low_prio_end_time;
  auto low_priority_future = std::async(std::launch::async, [this, &low_prio_end_time]() {
    VkResult result;

    if ((result = vkQueueWaitIdle(low_prio_device_.queue)) != VK_SUCCESS) {
      PRINT_STDERR("vkQueueWaitIdle failed: %d", result);
      return false;
    }
    low_prio_end_time = std::chrono::steady_clock::now();
    return true;
  });
  VkSubmitInfo high_prio_submit_info[kCommitNum];
  for (int i = 0; i < kCommitNum; i++) {
    high_prio_submit_info[i] = {
        .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
        .pNext = nullptr,
        .waitSemaphoreCount = 0,
        .pWaitSemaphores = nullptr,
        .pWaitDstStageMask = nullptr,
        .commandBufferCount = 1,
        .pCommandBuffers = &high_prio_device_.command_buffer,
        .signalSemaphoreCount = 0,
        .pSignalSemaphores = nullptr,
    };
  }

  auto high_prio_start_time = std::chrono::steady_clock::now();
  if ((result = vkQueueSubmit(high_prio_device_.queue, kCommitNum, high_prio_submit_info,
                              VK_NULL_HANDLE)) != VK_SUCCESS) {
    PRINT_STDERR("vkQueueSubmit failed: %d", result);
    return false;
  }

  std::chrono::steady_clock::time_point high_prio_end_time;
  auto high_priority_future = std::async(std::launch::async, [this, &high_prio_end_time]() {
    VkResult result;
    if ((result = vkQueueWaitIdle(high_prio_device_.queue)) != VK_SUCCESS) {
      PRINT_STDERR("vkQueueWaitIdle failed: %d", result);
      return false;
    }
    high_prio_end_time = std::chrono::steady_clock::now();
    return true;
  });

  high_priority_future.wait();
  low_priority_future.wait();
  if (!high_priority_future.get() || !low_priority_future.get()) {
    PRINT_STDERR("Queue wait failed");
    return false;
  }
  auto high_prio_duration = high_prio_end_time - high_prio_start_time;
  printf("high priority vkQueueWaitIdle finished duration: %lld\n",
         std::chrono::duration_cast<std::chrono::milliseconds>(high_prio_duration).count());
  auto low_prio_duration = low_prio_end_time - low_prio_start_time;
  printf("low priority vkQueueWaitIdle finished duration: %lld\n",
         std::chrono::duration_cast<std::chrono::milliseconds>(low_prio_duration).count());

  // Depends on the precise scheduling, so may sometimes fail.
  EXPECT_LE(high_prio_duration, low_prio_duration);

  return true;
}

// This test creates two devices with different global priorities and schedules the same amount
// of work on each. The higher priority device should finish faster even though it is scheduled
// second.
// TODO(fxbug.dev/402461734): Enable on GPUs that support global priority
TEST(Vulkan, DISABLED_GlobalPriorityTest) {
  VkGlobalPriorityTest test = {};
  ASSERT_TRUE(test.Initialize());
  ASSERT_TRUE(test.Exec());
}

}  // namespace
