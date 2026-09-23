// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fpromise/single_threaded_executor.h>

#include <future>

#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "unit-lib.h"

namespace ufs {
using namespace ufs_mock_device;

class PowerTest : public UfsTest {
 public:
  void SetUp() override { InitMockDevice(); }
};

TEST_F(PowerTest, PowerSuspendResume) {
  libsync::Completion sleep_complete;
  libsync::Completion awake_complete;
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeHibernateEnter,
      [&](UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2, uint32_t ucmdarg3) {
        mock_device_.GetUicCmdProcessor().DefaultDmeHibernateEnterHandler(mock_device, ucmdarg1,
                                                                          ucmdarg2, ucmdarg3);
        sleep_complete.Signal();
      });
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeHibernateExit,
      [&](UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2, uint32_t ucmdarg3) {
        mock_device_.GetUicCmdProcessor().DefaultDmeHibernateExitHandler(mock_device, ucmdarg1,
                                                                         ucmdarg2, ucmdarg3);
        awake_complete.Signal();
      });

  ASSERT_NO_FATAL_FAILURE(StartDriver(/*supply_power_framework=*/true));

  scsi::BlockDevice* block_device = nullptr;
  while (true) {
    {
      std::lock_guard<std::mutex> lock(dut_->lock());
      if (!dut_->block_devs().empty() && dut_->block_devs().contains(0) &&
          !dut_->block_devs().at(0).empty()) {
        block_device = dut_->block_devs().at(0).at(0).get();
        break;
      }
    }
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
  std::unique_ptr<block_client::RemoteBlockDevice> client;
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    auto client_end = driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(
        block_device->DeviceName().c_str());
    ASSERT_OK(client_end);
    auto remote_device_result =
        block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
    ASSERT_OK(remote_device_result);
    client = std::move(remote_device_result.value());
  }));

  // 1. Initial power level is kPowerLevelOff.
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { sleep_complete.Wait(); }));

  // TODO(https://fxbug.dev/42075643): Check if suspend is enabled with inspect
  ASSERT_FALSE(dut_->IsResumed());
  UfsPowerMode power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  const zx::vmo inspect_vmo = dut_->inspect().DuplicateVmo();
  ASSERT_TRUE(inspect_vmo.is_valid());

  fpromise::result<inspect::Hierarchy> hierarchy =
      fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  const auto* ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  const auto* power_suspended =
      ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NE(power_suspended, nullptr);
  EXPECT_TRUE(power_suspended->value());

  // 2. Issue request while power is suspended.
  awake_complete.Reset();
  sleep_complete.Reset();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
  zx_vaddr_t vaddr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       ufs_mock_device::kMockBlockSize, &vaddr));
  char* mapped_vaddr = reinterpret_cast<char*>(vaddr);
  std::strncpy(mapped_vaddr, "test", ufs_mock_device::kMockBlockSize);

  storage::Vmoid owned_vmoid;
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync(
      [&]() { ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid)); }));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  std::future<zx_status_t> tx_future =
      std::async(std::launch::async, [&]() { return client->FifoTransaction(&request, 1); });

  // The driver should stay suspended, and the block operation incomplete.
  zx_status_t status;
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync(
      [&]() { status = awake_complete.Wait(zx::msec(100)); }));
  EXPECT_EQ(status, ZX_ERR_TIMED_OUT);
  EXPECT_EQ(tx_future.wait_for(std::chrono::milliseconds(100)), std::future_status::timeout);

  // Wake up the driver.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOn})
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });

  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { awake_complete.Wait(); }));
  while (tx_future.wait_for(std::chrono::milliseconds(10)) != std::future_status::ready) {
    driver_test().runtime().RunUntilIdle();
  }
  ASSERT_OK(tx_future.get());
  ASSERT_TRUE(dut_->IsResumed());
  ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, ufs_mock_device::kMockBlockSize));
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync(
      [&]() { ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid))); }));

  // Return the driver to the suspended state.
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOff})
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });

  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { sleep_complete.Wait(); }));

  ASSERT_FALSE(dut_->IsResumed());
  power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  power_suspended = ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NE(power_suspended, nullptr);
  EXPECT_TRUE(power_suspended->value());

  // 3. Trigger power level change to kPowerLevelOn.
  awake_complete.Reset();
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOn})
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });

  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { awake_complete.Wait(); }));

  ASSERT_TRUE(dut_->IsResumed());
  power_mode = UfsPowerMode::kActive;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  power_suspended = ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NE(power_suspended, nullptr);
  EXPECT_FALSE(power_suspended->value());

  // 4. Trigger power level change to kPowerLevelOff.
  sleep_complete.Reset();
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOff})
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });

  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { sleep_complete.Wait(); }));

  ASSERT_FALSE(dut_->IsResumed());
  power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  power_suspended = ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NE(power_suspended, nullptr);
  EXPECT_TRUE(power_suspended->value());

  // 5. Restore driver to active state.
  libsync::Completion power_reply_1;
  awake_complete.Reset();
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOn})
        .ThenExactlyOnce([&, &reply = power_reply_1](
                             fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          reply.Signal();
        });
  });
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { awake_complete.Wait(); }));
  power_reply_1.Wait();
  ASSERT_TRUE(dut_->IsResumed());
  mock_device_.GetUicCmdProcessor().Reset();
}

TEST_F(PowerTest, BackgroundOperations) {
  libsync::Completion sleep_complete;
  libsync::Completion awake_complete;
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeHibernateEnter,
      [&](UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2, uint32_t ucmdarg3) {
        mock_device_.GetUicCmdProcessor().DefaultDmeHibernateEnterHandler(mock_device, ucmdarg1,
                                                                          ucmdarg2, ucmdarg3);
        sleep_complete.Signal();
      });
  mock_device_.GetUicCmdProcessor().SetHook(
      UicCommandOpcode::kDmeHibernateExit,
      [&](UfsMockDevice& mock_device, uint32_t ucmdarg1, uint32_t ucmdarg2, uint32_t ucmdarg3) {
        mock_device_.GetUicCmdProcessor().DefaultDmeHibernateExitHandler(mock_device, ucmdarg1,
                                                                         ucmdarg2, ucmdarg3);
        awake_complete.Signal();
      });

  ASSERT_NO_FATAL_FAILURE(StartDriver(/*supply_power_framework=*/true));

  // 1. Background Operation is disabled at power off
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { sleep_complete.Wait(); }));

  ASSERT_FALSE(dut_->IsResumed());
  UfsPowerMode power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  const zx::vmo inspect_vmo = dut_->inspect().DuplicateVmo();
  ASSERT_TRUE(inspect_vmo.is_valid());

  fpromise::result<inspect::Hierarchy> hierarchy =
      fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  const auto* ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  const auto* power_suspended =
      ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NE(power_suspended, nullptr);
  EXPECT_TRUE(power_suspended->value());

  const auto* bkop_node = ufs->GetByPath({"controller"})->GetByPath({"background_operations"});
  const auto* is_background_op_enabled =
      bkop_node->node().get_property<inspect::BoolPropertyValue>("is_background_op_enabled");
  ASSERT_NE(is_background_op_enabled, nullptr);
  EXPECT_FALSE(is_background_op_enabled->value());

  // 2. Background Operation is enabled at power on
  awake_complete.Reset();
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOn})
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { awake_complete.Wait(); }));

  ASSERT_TRUE(dut_->IsResumed());
  power_mode = UfsPowerMode::kActive;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  power_suspended = ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  EXPECT_FALSE(power_suspended->value());

  bkop_node = ufs->GetByPath({"controller"})->GetByPath({"background_operations"});
  is_background_op_enabled =
      bkop_node->node().get_property<inspect::BoolPropertyValue>("is_background_op_enabled");
  EXPECT_TRUE(is_background_op_enabled->value());

  // 3. Background operations change from disabled to enabled when an Urgent Background Operation
  // Exception Event occurs.
  ASSERT_OK(DisableBackgroundOp());

  hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  bkop_node = ufs->GetByPath({"controller"})->GetByPath({"background_operations"});
  is_background_op_enabled =
      bkop_node->node().get_property<inspect::BoolPropertyValue>("is_background_op_enabled");
  EXPECT_FALSE(is_background_op_enabled->value());

  // Using Exception Event to trigger Urgent Background Operations.
  mock_device_.SetExceptionEventAlert(true);
  ExceptionEventStatus ee_status = {0};
  ee_status.set_urgent_bkops(true);
  mock_device_.SetAttribute(Attributes::wExceptionEventStatus,
                            static_cast<uint32_t>(ee_status.value));
  mock_device_.SetAttribute(Attributes::bBackgroundOpStatus,
                            static_cast<uint32_t>(BackgroundOpStatus::kCritical));

  // To check for an Exception Event, send a command.
  auto attribute = ReadAttribute(Attributes::bBackgroundOpStatus);
  EXPECT_OK(attribute);

  // Wait for exception event completion
  auto wait_for = [&]() -> bool {
    // Check that Background Operations is enabled
    hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
    ufs = hierarchy.value().GetByPath({"ufs"});

    bkop_node = ufs->GetByPath({"controller"})->GetByPath({"background_operations"});
    is_background_op_enabled =
        bkop_node->node().get_property<inspect::BoolPropertyValue>("is_background_op_enabled");
    return is_background_op_enabled->value();
  };
  fbl::String timeout_message = "Timeout waiting for enabling Background Op";
  constexpr uint32_t kTimeoutUs = 1000000;
  ASSERT_OK(dut_->WaitWithTimeout(wait_for, zx::usec(kTimeoutUs), timeout_message));

  // Clean up
  mock_device_.SetExceptionEventAlert(false);

  // 4. Background Operation is disabled at power off
  sleep_complete.Reset();
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOff})
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { sleep_complete.Wait(); }));

  ASSERT_FALSE(dut_->IsResumed());
  power_mode = UfsPowerMode::kSleep;
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerMode(), power_mode);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentPowerCondition(),
            GetPowerModeInfo(power_mode).power_condition);
  ASSERT_EQ(dut_->GetDeviceManager().GetCurrentLinkState(),
            GetPowerModeInfo(power_mode).link_state);

  hierarchy = fpromise::run_single_threaded(inspect::ReadFromInspector(dut_->inspect()));
  ASSERT_TRUE(hierarchy.is_ok());
  ufs = hierarchy.value().GetByPath({"ufs"});
  ASSERT_NE(ufs, nullptr);

  power_suspended = ufs->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  EXPECT_TRUE(power_suspended->value());

  bkop_node = ufs->GetByPath({"controller"})->GetByPath({"background_operations"});
  is_background_op_enabled =
      bkop_node->node().get_property<inspect::BoolPropertyValue>("is_background_op_enabled");
  EXPECT_FALSE(is_background_op_enabled->value());

  // Restore driver to active state.
  libsync::Completion power_reply;
  awake_complete.Reset();
  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel({Ufs::kPowerLevelOn})
        .ThenExactlyOnce([&, &reply = power_reply](
                             fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          reply.Signal();
        });
  });
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() { awake_complete.Wait(); }));
  power_reply.Wait();
  ASSERT_TRUE(dut_->IsResumed());
  mock_device_.GetUicCmdProcessor().Reset();
}

}  // namespace ufs
