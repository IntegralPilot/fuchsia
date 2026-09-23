// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/sys_driver/magma_system_connection.h>
#include <lib/magma_service/sys_driver/magma_system_context.h>
#include <lib/magma_service/sys_driver/magma_system_device.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-arm-mali/include/magma_arm_mali_types.h"

namespace {

class Test {
 public:
  void TestValidImmediateInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom[2];
    atom[0].size = sizeof(atom[0]);
    atom[0].atom_number = 1;
    atom[0].flags = 1;
    atom[0].dependencies[0].atom_number = 0;
    atom[0].dependencies[1].atom_number = 0;
    atom[1].size = sizeof(atom[1]);
    atom[1].atom_number = 2;
    atom[1].flags = 1;
    atom[1].dependencies[0].atom_number = 1;
    atom[1].dependencies[0].type = kArmMaliDependencyOrder;
    atom[1].dependencies[1].atom_number = 0;

    magma::Status status =
        ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{.data = &atom[0],
                                                                  .size = sizeof(atom[0]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0},
                                    magma_inline_command_buffer_t{.data = &atom[1],
                                                                  .size = sizeof(atom[1]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_OK, status.get());
  }

  void TestValidSlot2Inline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom[1];
    atom[0].size = sizeof(atom[0]);
    atom[0].atom_number = 1;
    atom[0].flags = kAtomFlagForceSlot2;
    atom[0].dependencies[0].atom_number = 0;
    atom[0].dependencies[1].atom_number = 0;

    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = &atom, .size = sizeof(atom[0]), .semaphore_ids = nullptr, .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_OK, status.get());
  }

  void TestValidLargerInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    uint8_t buffer[100];
    ASSERT_GT(sizeof(buffer), sizeof(magma_arm_mali_atom));
    auto atom = reinterpret_cast<magma_arm_mali_atom*>(buffer);
    atom->size = sizeof(buffer);
    atom->atom_number = 1;
    atom->flags = 1;
    atom->dependencies[0].atom_number = 0;
    atom->dependencies[1].atom_number = 0;

    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = buffer, .size = sizeof(buffer), .semaphore_ids = nullptr, .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_OK, status.get());
  }

  void TestInvalidTooLargeInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    uint8_t buffer[100];
    ASSERT_GT(sizeof(buffer), sizeof(magma_arm_mali_atom) + 1);
    auto atom = reinterpret_cast<magma_arm_mali_atom*>(buffer);
    atom->size = sizeof(buffer);
    atom->atom_number = 1;
    atom->flags = 1;
    atom->dependencies[0].atom_number = 0;
    atom->dependencies[1].atom_number = 0;

    magma::Status status =
        ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{.data = buffer,
                                                                  .size = sizeof(buffer) - 1,
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidOverflowInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom[3];
    for (uint8_t i = 0; i < 3; i++) {
      atom[i].size = sizeof(atom[i]);
      atom[i].atom_number = i + 1;
      atom[i].flags = 1;
      atom[i].dependencies[0].atom_number = 0;
      atom[i].dependencies[1].atom_number = 0;
    }
    atom[2].size = reinterpret_cast<uintptr_t>(&atom[1]) - reinterpret_cast<uintptr_t>(&atom[2]);
    EXPECT_GE(atom[2].size, static_cast<uint64_t>(INT64_MAX));

    magma::Status status =
        ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{.data = &atom[0],
                                                                  .size = sizeof(atom[0]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0},
                                    magma_inline_command_buffer_t{.data = &atom[1],
                                                                  .size = sizeof(atom[1]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0},
                                    magma_inline_command_buffer_t{.data = &atom[2],
                                                                  .size = sizeof(atom[2]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0},
                                    magma_inline_command_buffer_t{.data = &atom[3],
                                                                  .size = sizeof(atom[3]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidZeroSizeInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom;
    atom.size = 0;
    atom.atom_number = 1;
    atom.flags = 1;
    atom.dependencies[0].atom_number = 0;
    atom.dependencies[1].atom_number = 0;

    // Shouldn't infinite loop, for example.
    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = &atom, .size = sizeof(atom), .semaphore_ids = nullptr, .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidSmallerInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom;
    atom.size = sizeof(atom) - 1;
    atom.atom_number = 1;
    atom.flags = 1;
    atom.dependencies[0].atom_number = 0;
    atom.dependencies[1].atom_number = 0;

    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = &atom, .size = sizeof(atom), .semaphore_ids = nullptr, .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidInUseInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom[2];
    atom[0].size = sizeof(atom[0]);
    atom[0].atom_number = 0;
    atom[0].flags = 1;
    atom[0].dependencies[0].atom_number = 0;
    atom[0].dependencies[1].atom_number = 0;
    atom[1].size = sizeof(atom[1]);
    atom[1].atom_number = 0;
    atom[1].flags = 1;
    atom[1].dependencies[0].atom_number = 0;
    atom[1].dependencies[1].atom_number = 0;

    magma::Status status =
        ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{.data = &atom[0],
                                                                  .size = sizeof(atom[0]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0},
                                    magma_inline_command_buffer_t{.data = &atom[1],
                                                                  .size = sizeof(atom[1]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0}});
    // There's no device thread, so the atoms shouldn't be able to complete.
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidDependencyNotSubmittedInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom;
    atom.size = sizeof(atom);
    atom.atom_number = 1;
    atom.flags = 1;
    // Can't depend on self or on later atoms.
    atom.dependencies[0].atom_number = 1;
    atom.dependencies[0].type = kArmMaliDependencyOrder;
    atom.dependencies[1].atom_number = 0;

    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = &atom, .size = sizeof(atom), .semaphore_ids = nullptr, .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidDependencyTypeInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom[2];
    atom[0].size = sizeof(atom[0]);
    atom[0].atom_number = 1;
    atom[0].flags = 1;
    atom[0].dependencies[0].atom_number = 0;
    atom[0].dependencies[1].atom_number = 0;
    atom[1].size = sizeof(atom[1]);
    atom[1].atom_number = 2;
    atom[1].flags = 1;
    atom[1].dependencies[0].atom_number = 1;
    atom[1].dependencies[0].type = 5;
    atom[1].dependencies[1].atom_number = 0;

    magma::Status status =
        ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{.data = &atom[0],
                                                                  .size = sizeof(atom[0]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0},
                                    magma_inline_command_buffer_t{.data = &atom[1],
                                                                  .size = sizeof(atom[1]),
                                                                  .semaphore_ids = nullptr,
                                                                  .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestInvalidSemaphoreImmediateInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);

    magma_arm_mali_atom atom;
    atom.size = sizeof(atom);
    atom.atom_number = 0;
    atom.flags = kAtomFlagSemaphoreSet;
    atom.dependencies[0].atom_number = 0;
    atom.dependencies[1].atom_number = 0;

    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = &atom, .size = sizeof(atom), .semaphore_ids = nullptr, .semaphore_count = 0}});
    EXPECT_EQ(MAGMA_STATUS_CONTEXT_KILLED, status.get());
  }

  void TestSemaphoreImmediateInline() {
    auto ctx = InitializeContext();
    ASSERT_TRUE(ctx);
    auto platform_semaphore = magma::PlatformSemaphore::Create();
    zx::handle handle;
    platform_semaphore->duplicate_handle(&handle);
    connection_->ImportObject(std::move(handle), /*flags=*/0,
                              fuchsia_gpu_magma::wire::ObjectType::kSemaphore,
                              platform_semaphore->id());

    magma_arm_mali_atom atom;
    atom.size = sizeof(atom);
    atom.atom_number = 0;
    atom.flags = kAtomFlagSemaphoreSet;
    atom.dependencies[0].atom_number = 0;
    atom.dependencies[1].atom_number = 0;
    uint64_t semaphores[] = {platform_semaphore->id()};

    magma::Status status = ctx->ExecuteInlineCommands({magma_inline_command_buffer_t{
        .data = &atom, .size = sizeof(atom), .semaphore_ids = semaphores, .semaphore_count = 1}});
    EXPECT_EQ(MAGMA_STATUS_OK, status.get());
  }

 private:
  msd::MagmaSystemContext* InitializeContext() {
    msd_drv_ = msd::Driver::MsdCreate();
    if (!msd_drv_)
      return DRETP(nullptr, "failed to create msd driver");

    msd_drv_->MsdConfigure(MSD_DRIVER_CONFIG_TEST_NO_DEVICE_THREAD);

    auto msd_dev = msd_drv_->MsdCreateDevice(GetTestDeviceHandle());
    if (!msd_dev)
      return DRETP(nullptr, "failed to create msd device");
    auto msd_connection_t = msd_dev->MsdOpen(0);
    if (!msd_connection_t)
      return DRETP(nullptr, "msd_device_open failed");
    system_dev_ = msd::MagmaSystemDevice::Create(msd_drv_.get(), std::move(msd_dev));
    uint32_t ctx_id = 0;
    connection_ = std::make_unique<msd::MagmaSystemConnection>(system_dev_.get(),
                                                               std::move(msd_connection_t));
    if (!connection_)
      return DRETP(nullptr, "failed to connect to msd device");
    connection_->CreateContext(ctx_id);
    auto ctx = connection_->LookupContext(ctx_id);
    if (!ctx)
      return DRETP(nullptr, "failed to create context");
    return ctx;
  }

  std::unique_ptr<msd::Driver> msd_drv_;
  std::unique_ptr<msd::MagmaSystemDevice> system_dev_;
  std::unique_ptr<msd::MagmaSystemConnection> connection_;
};

TEST(CommandBuffer, TestInvalidSemaphoreImmediateInline) {
  ::Test().TestInvalidSemaphoreImmediateInline();
}

TEST(CommandBuffer, TestSemaphoreImmediateInline) { ::Test().TestSemaphoreImmediateInline(); }

TEST(CommandBuffer, TestValidImmediateInline) { ::Test().TestValidImmediateInline(); }

TEST(CommandBuffer, TestValidLargerInline) { ::Test().TestValidLargerInline(); }

TEST(CommandBuffer, TestValidSlot2Inline) { ::Test().TestValidSlot2Inline(); }

TEST(CommandBuffer, TestInvalidTooLargeInline) { ::Test().TestInvalidTooLargeInline(); }

TEST(CommandBuffer, TestInvalidOverflowInline) { ::Test().TestInvalidOverflowInline(); }

TEST(CommandBuffer, TestInvalidZeroSizeInline) { ::Test().TestInvalidZeroSizeInline(); }

TEST(CommandBuffer, TestInvalidSmallerInline) { ::Test().TestInvalidSmallerInline(); }

TEST(CommandBuffer, TestInvalidInUseInline) { ::Test().TestInvalidInUseInline(); }

TEST(CommandBuffer, TestInvalidDependencyTypeInline) { ::Test().TestInvalidDependencyTypeInline(); }

TEST(CommandBuffer, TestInvalidDependencyNotSubmittedInline) {
  ::Test().TestInvalidDependencyNotSubmittedInline();
}
}  // namespace
