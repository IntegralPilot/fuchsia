// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/graphics/examples/vkproto/common/command_buffers.h"
#include "src/graphics/examples/vkproto/common/debug_utils_messenger.h"
#include "src/graphics/examples/vkproto/common/graphics_pipeline.h"
#include "src/graphics/examples/vkproto/common/image_view.h"
#include "src/graphics/examples/vkproto/common/instance.h"
#include "src/graphics/examples/vkproto/common/physical_device.h"
#include "src/graphics/examples/vkproto/common/render_pass.h"
#ifdef __Fuchsia__
#include "src/graphics/examples/vkproto/fuchsia/fuchsia_surface.h"
#else
#include "src/graphics/examples/vkproto/glfw/glfw_surface.h"
#endif
#include "src/graphics/examples/vkproto/common/swapchain.h"
#include "src/graphics/examples/vkproto/common/utils.h"

#include <vulkan/vulkan.hpp>

#if USE_GLFW
#define GLFW_INCLUDE_VULKAN
#include <GLFW/glfw3.h>
#endif

static bool DrawFrame(const vkp::Device& vkp_device, const vkp::Swapchain& swap_chain,
                      const vkp::CommandBuffers& command_buffers,
                      const std::vector<vk::UniqueFence>& fences);

#if USE_GLFW
void glfwErrorCallback(int error, const char* description) {
  fprintf(stderr, "glfwErrorCallback: %d : %s\n", error, description);
}

static void glfwFramebufferResizeCallback(GLFWwindow* window, int width, int height) {
  // TODO(rosasco): Add ability to recreate swapchain here. Define new struct with all
  //                required dependencies to rebuild the swapchain called SwapchainDeps.
  //                Call glfwSetWindowUserPointer(window, swapchain_deps) in main below, then:
  //                auto swapchain_deps =
  //                    reinterpret_cast<SwapchainDeps*>(glfwGetWindowUserPointer(window));
  fprintf(stderr, "ERROR: Window resize not implemented.\n");
  exit(1);
}
#endif

bool TestCommon() {
  // INSTANCE
  const bool kEnableValidation = true;
  vkp::Instance vkp_instance(kEnableValidation);
  RTN_IF_MSG(false, !vkp_instance.Init(), "Instance Initialization Failed.\n");
  std::shared_ptr<vk::Instance> instance = vkp_instance.shared();

  // DEBUG UTILS MESSENGER
  vkp::DebugUtilsMessenger vkp_debug_messenger(instance);
  RTN_IF_MSG(false, !vkp_debug_messenger.Init(), "Debug messenger initialization failed");

#if USE_GLFW
  GLFWwindow* window = nullptr;
  glfwInit();
  glfwSetErrorCallback(glfwErrorCallback);
  RTN_IF_MSG(false, !glfwVulkanSupported(), "glfwVulkanSupported has returned false.\n");
  glfwWindowHint(GLFW_CLIENT_API, GLFW_NO_API);
  window = glfwCreateWindow(1024, 768, "VkProto", nullptr, nullptr);
  RTN_IF_MSG(false, !window, "glfwCreateWindow failed.\n");
  glfwSetFramebufferSizeCallback(window, glfwFramebufferResizeCallback);
#endif

  // SURFACE
  std::shared_ptr<vkp::Surface> vkp_surface;
#if USE_GLFW
  vkp_surface = std::make_shared<vkp::GlfwSurface>(instance, window);
#else
  vkp_surface = std::make_shared<vkp::FuchsiaSurface>(instance);
#endif
  RTN_IF_MSG(false, !vkp_surface->Init(), "Surface initialization failed\n");

  VkSurfaceKHR surface = vkp_surface ? vkp_surface->get() : nullptr;

  // PHYSICAL DEVICE
  vkp::PhysicalDevice vkp_physical_device(instance, surface);
  RTN_IF_MSG(false, !vkp_physical_device.Init(), "Physical device initialization failed\n");
  const vk::PhysicalDevice& physical_device = vkp_physical_device.get();

  // LOGICAL DEVICE
  auto vkp_device = vkp::Device(physical_device, surface);
  RTN_IF_MSG(false, !vkp_device.Init(), "Logical device initialization failed\n");
  std::shared_ptr<vk::Device> device = vkp_device.shared();

  vk::Format image_format;
  vk::Extent2D extent;
  std::shared_ptr<vkp::Swapchain> vkp_swap_chain;

  // The number of image views added controls the number of framebuffers,
  // command buffers, fences and signaling semaphores created subsequently.
  std::vector<vk::ImageView> image_views;
  // SWAP CHAIN
  vkp_swap_chain = std::make_shared<vkp::Swapchain>(physical_device, device, surface);
  RTN_IF_MSG(false, !vkp_swap_chain->Init(), "Swap chain initialization failed\n");

  image_format = vkp_swap_chain->image_format();
  extent = vkp_swap_chain->extent();
  const auto& swap_chain_image_views = vkp_swap_chain->image_views();
  for (auto& view : swap_chain_image_views) {
    image_views.emplace_back(*view);
  }

  // RENDER PASS
  auto vkp_render_pass =
      std::make_shared<vkp::RenderPass>(device, image_format, false /* offscreen */);
  RTN_IF_MSG(false, !vkp_render_pass->Init(), "Render pass initialization failed\n");

  // GRAPHICS PIPELINE
  auto vkp_pipeline = std::make_unique<vkp::GraphicsPipeline>(device, extent, vkp_render_pass);
  RTN_IF_MSG(false, !vkp_pipeline->Init(), "Graphics pipeline initialization failed\n");

  // FRAMEBUFFER
  auto vkp_framebuffers =
      std::make_unique<vkp::Framebuffers>(device, extent, vkp_render_pass->get(), image_views);
  RTN_IF_MSG(false, !vkp_framebuffers->Init(), "Framebuffer Initialization Failed.\n");

  // COMMAND POOL
  auto vkp_command_pool =
      std::make_shared<vkp::CommandPool>(device, vkp_device.queue_family_index());
  RTN_IF_MSG(false, !vkp_command_pool->Init(), "Command Pool Initialization Failed.\n");

  // COMMAND BUFFER
  auto vkp_command_buffers = std::make_unique<vkp::CommandBuffers>(
      vkp_device.shared(), vkp_command_pool, vkp_framebuffers->framebuffers(), vkp_pipeline->get(),
      vkp_render_pass->get(), extent);
  RTN_IF_MSG(false, !vkp_command_buffers->Init(), "Command buffer initialization.\n");

  // There is a 1/1/1 mapping between
  // swapchain image view / command buffer / fence.
  std::vector<vk::UniqueFence> fences;
  const vk::FenceCreateInfo fence_info(vk::FenceCreateFlagBits::eSignaled);
  for (size_t i = 0; i < image_views.size(); i++) {
    auto [r_fence, fence] = device->createFenceUnique(fence_info);
    RTN_IF_VKH_ERR(false, r_fence, "Onscreen submission fence.\n");
    fences.emplace_back(std::move(fence));
  }

#if USE_GLFW
  while (!glfwWindowShouldClose(window)) {
    glfwPollEvents();
    DrawFrame(vkp_device, *vkp_swap_chain, *vkp_command_buffers, fences);
  }
#else
  DrawFrame(vkp_device, *vkp_swap_chain, *vkp_command_buffers, fences);
  sleep(30);
#endif
  RTN_IF_VKH_ERR(false, device->waitIdle(), "waitIdle\n");

#if USE_GLFW
  glfwDestroyWindow(window);
  glfwTerminate();
#endif
  return true;
}

bool DrawFrame(const vkp::Device& vkp_device, const vkp::Swapchain& vkp_swap_chain,
               const vkp::CommandBuffers& vkp_command_buffers,
               const std::vector<vk::UniqueFence>& fences) {
  // Compact variables for readability derived from |current_frame|.
  const vk::Device& device = vkp_device.get();

  auto [r_image_available_semaphore, image_available_semaphore] =
      device.createSemaphore(vk::SemaphoreCreateInfo{});
  RTN_IF_VKH_ERR(false, r_image_available_semaphore, "Image available semaphore.\n");

  auto [r_render_finished_semaphore, render_finished_semaphore] =
      device.createSemaphore(vk::SemaphoreCreateInfo{});
  RTN_IF_VKH_ERR(false, r_render_finished_semaphore, "Render finished semaphore.\n");

  // Obtain next swap chain image in which to draw.
  // The timeout makes this a blocking call if no swapchain images, and
  // therefore command buffers, are available so there is no need to wait for a
  // submission fence before calling acquireNextImageKHR().
  auto [r_acquire, swapchain_image_index] =
      device.acquireNextImageKHR(vkp_swap_chain.get(), std::numeric_limits<uint64_t>::max(),
                                 image_available_semaphore, nullptr);
  RTN_IF_VKH_ERR(false, r_acquire, "Acquire swapchain image.\n");

  // Define stage that |image_available_semaphore| is waiting on.
  const vk::PipelineStageFlags image_available_wait_stage =
      vk::PipelineStageFlagBits::eColorAttachmentOutput;

  vk::CommandBuffer command_buffer =
      vkp_command_buffers.command_buffers()[swapchain_image_index].get();

  vk::SubmitInfo submit_info;
  submit_info.waitSemaphoreCount = 1;
  submit_info.pWaitSemaphores = &image_available_semaphore;
  submit_info.pWaitDstStageMask = &image_available_wait_stage;
  submit_info.commandBufferCount = 1;
  submit_info.pCommandBuffers = &command_buffer;
  submit_info.signalSemaphoreCount = 1;
  submit_info.pSignalSemaphores = &render_finished_semaphore;

  // No guarantee that we're done with the command buffer we're about to use,
  // so wait on its fence.
  const vk::Fence& fence = fences[swapchain_image_index].get();
  RTN_IF_VKH_ERR(false,
                 device.waitForFences(1, &fence, VK_TRUE, std::numeric_limits<uint64_t>::max()),
                 "waitForFences\n");
  RTN_IF_VKH_ERR(false, device.resetFences(1, &fence), "resetFences failed\n");

  RTN_IF_VKH_ERR(false, vkp_device.queue().submit(1, &submit_info, fence),
                 "Failed to onscreen submit command buffer.\n");

  vk::PresentInfoKHR present_info;
  present_info.waitSemaphoreCount = 1;
  present_info.pWaitSemaphores = &render_finished_semaphore;
  present_info.swapchainCount = 1;
  present_info.setPSwapchains(&(vkp_swap_chain.get()));
  present_info.pImageIndices = &swapchain_image_index;

  RTN_IF_VKH_ERR(false, vkp_device.queue().presentKHR(&present_info), "presentKHR failed\n");
  RTN_IF_VKH_ERR(false, vkp_device.queue().waitIdle(), "queue waitIdle failed\n");

  device.destroySemaphore(render_finished_semaphore);
  device.destroySemaphore(image_available_semaphore);

  return true;
}

TEST(VkProto, Onscreen) { ASSERT_TRUE(TestCommon()); }
