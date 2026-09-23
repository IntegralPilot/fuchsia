// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/impl/semaphore_pool.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/event.h>

#include <gtest/gtest.h>

#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/lib/escher/test/common/test_with_vk_validation_layer.h"
#include "src/ui/lib/escher/vk/vulkan_context.h"
#include "src/ui/lib/escher/vk/vulkan_device_queues.h"

namespace escher {
namespace {

using SemaphorePoolTest = escher::test::TestWithVkValidationLayer;

// Helper to get a fresh Zircon event.
zx::event CreateEvent() {
  zx::event event;
  zx_status_t status = zx::event::create(0, &event);
  FX_DCHECK(status == ZX_OK);
  return event;
}

// Verify that allocating from a completely empty pool returns a new, valid,
// unimported C++ Semaphore object.
VK_TEST_F(SemaphorePoolTest, AllocateFromEmptyPool) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();

  SemaphorePool pool(device, dispatch_loader);

  auto sem = pool.Allocate();
  ASSERT_TRUE(sem);
  EXPECT_NE(sem->vk_semaphore(), vk::Semaphore());
  EXPECT_FALSE(sem->is_imported());
}

// Verify that an unimported semaphore is routed to the cleaned_free_list_
// upon retirement and successfully reused by subsequent Allocate() calls.
VK_TEST_F(SemaphorePoolTest, RecycleAndReuseCleanedSemaphore) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();

  SemaphorePool pool(device, dispatch_loader);

  auto sem1 = pool.Allocate();
  const auto raw_vk_sem = sem1->vk_semaphore();
  EXPECT_FALSE(sem1->is_imported());

  // Release semaphore. When its ref-count hits 0, it should return to the pool.
  sem1 = nullptr;

  // Allocate again. The pool should reuse the exact same Vulkan semaphore.
  auto sem2 = pool.Allocate();
  EXPECT_EQ(sem2->vk_semaphore(), raw_vk_sem);
  EXPECT_FALSE(sem2->is_imported());
}

// Verify that an imported semaphore is routed to the uncleaned_free_list_ upon
// retirement and successfully reused by subsequent AllocateAndImport() calls.
VK_TEST_F(SemaphorePoolTest, RecycleAndReuseUncleanedSemaphore) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();

  SemaphorePool pool(device, dispatch_loader);

  auto sem1 = pool.AllocateAndImport(CreateEvent());
  const auto raw_vk_sem = sem1->vk_semaphore();
  EXPECT_TRUE(sem1->is_imported());

  // Release semaphore. It should return to the pool's uncleaned list.
  sem1 = nullptr;

  // Allocate and import again. The pool should reuse the exact same Vulkan semaphore.
  auto sem2 = pool.AllocateAndImport(CreateEvent());
  EXPECT_EQ(sem2->vk_semaphore(), raw_vk_sem);
  EXPECT_TRUE(sem2->is_imported());
}

// Verify the decoupled fallback where Allocate() pulls from the uncleaned list,
// performs a lazy clean, resets/preserves the flags, and returns.
VK_TEST_F(SemaphorePoolTest, AllocatePullsFromUncleanedWithLazyStomp) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();

  SemaphorePool pool(device, dispatch_loader);

  // Put a semaphore into the uncleaned list by allocating/importing and releasing.
  auto sem1 = pool.AllocateAndImport(CreateEvent());
  const auto raw_vk_sem = sem1->vk_semaphore();
  sem1 = nullptr;

  // Call Allocate() (with cleaned list empty). It should fallback to uncleaned,
  // perform a lazy clean, and return it.
  auto sem2 = pool.Allocate();
  EXPECT_EQ(sem2->vk_semaphore(), raw_vk_sem);
  EXPECT_FALSE(sem2->is_imported());
}

// Verify the decoupled fallback where AllocateAndImport() pulls from the cleaned
// list, overwrites it with a new import, and sets is_imported to true.
VK_TEST_F(SemaphorePoolTest, AllocateAndImportPullsFromCleaned) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();

  SemaphorePool pool(device, dispatch_loader);

  // Put a semaphore into the cleaned list by allocating internally and releasing.
  auto sem1 = pool.Allocate();
  const auto raw_vk_sem = sem1->vk_semaphore();
  sem1 = nullptr;

  // Call AllocateAndImport() (with uncleaned list empty). It should fallback to cleaned,
  // import the new event, and return it.
  auto sem2 = pool.AllocateAndImport(CreateEvent());
  EXPECT_EQ(sem2->vk_semaphore(), raw_vk_sem);
  EXPECT_TRUE(sem2->is_imported());
}

// Verify that a cleaned semaphore can be signaled and waited on again by the GPU queue.
VK_TEST_F(SemaphorePoolTest, VerifySignalingCleanedSemaphore) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();
  auto queue = vulkan_queues->GetVulkanContext().queue;

  SemaphorePool pool(device, dispatch_loader);

  auto sem1 = pool.Allocate();
  const auto raw_vk_sem = sem1->vk_semaphore();

  // Signal semaphore via a queue submit.
  {
    vk::SubmitInfo submit_info;
    submit_info.signalSemaphoreCount = 1;
    submit_info.pSignalSemaphores = &raw_vk_sem;
    auto result = queue.submit(1, &submit_info, vk::Fence());
    EXPECT_EQ(result, vk::Result::eSuccess);
  }

  // Wait for GPU to finish executing the queue command.
  {
    auto idle_result = device.waitIdle();
    EXPECT_EQ(idle_result, vk::Result::eSuccess);
  }

  // Retire semaphore, so it goes to the cleaned list.
  sem1 = nullptr;

  auto sem2 = pool.Allocate();
  EXPECT_EQ(sem2->vk_semaphore(), raw_vk_sem);

  // Signal it again. If the clean failed to reset the Vulkan-internal signal state,
  // the Vulkan Validation Layers will instantly trigger a validation error.
  {
    vk::SubmitInfo submit_info;
    submit_info.signalSemaphoreCount = 1;
    const vk::Semaphore raw_vk_sem2 = sem2->vk_semaphore();
    submit_info.pSignalSemaphores = &raw_vk_sem2;
    auto result = queue.submit(1, &submit_info, vk::Fence());
    EXPECT_EQ(result, vk::Result::eSuccess);
  }

  // Wait for the queue to finish executing.
  auto idle_result = device.waitIdle();
  EXPECT_EQ(idle_result, vk::Result::eSuccess);
}

// Verify that an uncleaned semaphore is successfully signaled, and that importing a new event
// successfully replaces its payload, allowing it to be signaled and waited on again.
VK_TEST_F(SemaphorePoolTest, VerifySignalingRecycledUncleanedSemaphore) {
  auto vulkan_queues =
      escher::test::EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanDevice();
  auto device = vulkan_queues->GetVulkanContext().device;
  auto dispatch_loader = vulkan_queues->dispatch_loader();
  auto queue = vulkan_queues->GetVulkanContext().queue;

  SemaphorePool pool(device, dispatch_loader);

  zx::event event1 = CreateEvent();
  zx::event event1_dup;
  auto dup_status = event1.duplicate(ZX_RIGHT_SAME_RIGHTS, &event1_dup);
  ASSERT_EQ(dup_status, ZX_OK);

  auto sem1 = pool.AllocateAndImport(std::move(event1));
  const auto raw_vk_sem = sem1->vk_semaphore();

  // Signal the Vulkan semaphore.
  {
    vk::SubmitInfo submit_info;
    submit_info.signalSemaphoreCount = 1;
    submit_info.pSignalSemaphores = &raw_vk_sem;
    auto result = queue.submit(1, &submit_info, vk::Fence());
    EXPECT_EQ(result, vk::Result::eSuccess);
  }

  // Verify that the duplicate event is signaled on the CPU.
  auto wait_status = event1_dup.wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), nullptr);
  EXPECT_EQ(wait_status, ZX_OK);

  // Wait for GPU to finish executing the queue command.
  {
    auto idle_result = device.waitIdle();
    EXPECT_EQ(idle_result, vk::Result::eSuccess);
  }

  // Retire the semaphore, so it goes to the uncleaned list.
  sem1 = nullptr;

  zx::event event2 = CreateEvent();
  zx::event event2_dup;
  dup_status = event2.duplicate(ZX_RIGHT_SAME_RIGHTS, &event2_dup);
  ASSERT_EQ(dup_status, ZX_OK);

  auto sem2 = pool.AllocateAndImport(std::move(event2));
  EXPECT_EQ(sem2->vk_semaphore(), raw_vk_sem);

  // Signal it again.
  {
    vk::SubmitInfo submit_info;
    submit_info.signalSemaphoreCount = 1;
    const vk::Semaphore raw_vk_sem2 = sem2->vk_semaphore();
    submit_info.pSignalSemaphores = &raw_vk_sem2;
    auto result = queue.submit(1, &submit_info, vk::Fence());
    EXPECT_EQ(result, vk::Result::eSuccess);
  }

  // Verify that the new event is signaled on the CPU.
  wait_status = event2_dup.wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), nullptr);
  EXPECT_EQ(wait_status, ZX_OK);

  // Wait for the queue to finish.
  auto idle_result = device.waitIdle();
  EXPECT_EQ(idle_result, vk::Result::eSuccess);
}

}  // namespace
}  // namespace escher
