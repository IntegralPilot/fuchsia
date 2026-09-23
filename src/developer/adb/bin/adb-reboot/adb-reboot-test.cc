// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adb-reboot.h"

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fuchsia/hardware/adb/cpp/fidl.h>
#include <fuchsia/hardware/power/statecontrol/cpp/fidl.h>
#include <fuchsia/hardware/power/statecontrol/cpp/fidl_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>

#include <atomic>
#include <memory>
#include <optional>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/storage/lib/block_server/fake_server.h"

namespace adb_reboot {

class FakeBlockDeviceComponent : public component_testing::LocalComponentImpl {
 public:
  FakeBlockDeviceComponent() {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(4096, 0, &vmo);
    ZX_ASSERT(status == ZX_OK);
    status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_);
    ZX_ASSERT(status == ZX_OK);
    block_server::PartitionInfo info = {};
    info.block_count = 4096 / 512;
    info.block_size = 512;
    info.type_guid[0] = 1;
    info.instance_guid[0] = 2;
    info.name = "misc";
    server_ = std::make_unique<block_server::FakeServer>(info, std::move(vmo));
  }

  void OnStart() override {
    outgoing()->GetOrCreateDirectory("misc")->AddEntry(
        fidl::DiscoverableProtocolName<fuchsia_storage_block::Block>,
        std::make_unique<vfs::Service>(
            [this](fidl::ServerEnd<fuchsia_storage_block::Block> request) {
              server_->Serve(std::move(request));
            }));
  }

  void ReadVmo(void* buffer, uint64_t offset, size_t size) {
    zx_status_t status = vmo_.read(buffer, offset, size);
    ZX_ASSERT(status == ZX_OK);
  }

 private:
  zx::vmo vmo_;
  std::unique_ptr<block_server::FakeServer> server_;
};

class LocalPowerStateControl
    : public fuchsia::hardware::power::statecontrol::testing::Admin_TestBase,
      public component_testing::LocalComponentImpl {
 public:
  explicit LocalPowerStateControl(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  ~LocalPowerStateControl() override { ClearExpectations(); }

  // component_testing::LocalComponentImpl methods.
  void OnStart() override {
    ASSERT_EQ(outgoing()->AddPublicService(bindings_.GetHandler(this, dispatcher_)), ZX_OK);
  }

  // fuchsia::hardware::power::statecontrol::Admin methods.
  void NotImplemented_(const std::string& name) override {
    FX_LOGS(ERROR) << "Not implemented " << name;
  }

  void Shutdown(::fuchsia::hardware::power::statecontrol::ShutdownOptions options,
                ShutdownCallback callback) override {
    if (options.has_action() &&
        options.action() == fuchsia::hardware::power::statecontrol::ShutdownAction::REBOOT) {
      expect_reboot_--;
      reboot_complete = true;
    } else if (options.has_action() &&
               options.action() ==
                   fuchsia::hardware::power::statecontrol::ShutdownAction::REBOOT_TO_BOOTLOADER) {
      expect_reboot_bootloader_--;
      reboot_complete = true;
    } else if (options.has_action() &&
               options.action() ==
                   fuchsia::hardware::power::statecontrol::ShutdownAction::REBOOT_TO_RECOVERY) {
      expect_reboot_recovery_--;
      reboot_complete = true;
    }
  }

  void ExpectReboot() { expect_reboot_++; }
  void ExpectRebootBootloader() { expect_reboot_bootloader_++; }
  void ExpectRebootRecovery() { expect_reboot_recovery_++; }

  void ClearExpectations() const {
    ASSERT_EQ(expect_reboot_, 0);
    ASSERT_EQ(expect_reboot_bootloader_, 0);
    ASSERT_EQ(expect_reboot_recovery_, 0);
  }

  std::atomic_bool reboot_complete = false;

 private:
  async_dispatcher_t* dispatcher_;
  fidl::BindingSet<fuchsia::hardware::power::statecontrol::Admin> bindings_;
  int expect_reboot_ = 0;
  int expect_reboot_bootloader_ = 0;
  int expect_reboot_recovery_ = 0;
};

class AdbRebootTest : public gtest::RealLoopFixture {
 public:
  void SetUp() override {
    using namespace component_testing;

    auto builder = RealmBuilder::Create();

    auto local_power_state_control = std::make_unique<LocalPowerStateControl>(dispatcher());
    local_power_state_control_ptr_ = local_power_state_control.get();
    builder.AddLocalChild(
        "power_statecontrol",
        [&, local_power_state_control = std::move(local_power_state_control)]() mutable {
          return std::move(local_power_state_control);
        });

    auto fake_block_device = std::make_unique<FakeBlockDeviceComponent>();
    fake_block_device_ptr_ = fake_block_device.get();
    builder.AddLocalChild("block_device",
                          [&, fake_block_device = std::move(fake_block_device)]() mutable {
                            return std::move(fake_block_device);
                          });

    builder.AddChild("adb-reboot", "#meta/adb-reboot.cm");

    builder.AddRoute(
        Route{.capabilities = {Protocol{fuchsia::hardware::power::statecontrol::Admin::Name_}},
              .source = ChildRef{"power_statecontrol"},
              .targets = {ChildRef{"adb-reboot"}}});
    builder.AddRoute(Route{.capabilities = {Directory{
                               .name = "misc", .rights = fuchsia::io::R_STAR_DIR, .path = "/misc"}},
                           .source = ChildRef{"block_device"},
                           .targets = {ChildRef{"adb-reboot"}}});
    builder.AddRoute(Route{.capabilities = {Protocol{fuchsia::hardware::adb::Provider::Name_}},
                           .source = ChildRef{"adb-reboot"},
                           .targets = {ParentRef()}});
    realm_ = builder.Build(dispatcher());

    ASSERT_EQ(realm_->component().Connect<fuchsia::hardware::adb::Provider>(
                  reboot_.NewRequest(dispatcher())),
              ZX_OK);

    reboot_.set_error_handler(
        [](zx_status_t status) { FX_LOGS(INFO) << "adb reboot could not connect " << status; });
  }

  void TearDown() override {
    bool complete = false;
    realm_->Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  }

 protected:
  std::optional<component_testing::RealmRoot> realm_;
  fidl::InterfacePtr<fuchsia::hardware::adb::Provider> reboot_;
  LocalPowerStateControl* local_power_state_control_ptr_ = nullptr;
  FakeBlockDeviceComponent* fake_block_device_ptr_ = nullptr;
};

TEST_F(AdbRebootTest, Reboot) {
  local_power_state_control_ptr_->ExpectReboot();

  zx::socket server, client;
  ASSERT_EQ(zx::socket::create(ZX_SOCKET_STREAM, &server, &client), ZX_OK);

  reboot_->ConnectToService(std::move(client), "",
                            [&](auto result) { ASSERT_FALSE(result.is_err()); });
  // Wait for reboot to complete.
  RunLoopUntil([&]() { return local_power_state_control_ptr_->reboot_complete.load(); });
}

TEST_F(AdbRebootTest, RebootBootloader) {
  local_power_state_control_ptr_->ExpectRebootBootloader();

  zx::socket server, client;
  ASSERT_EQ(zx::socket::create(ZX_SOCKET_STREAM, &server, &client), ZX_OK);

  reboot_->ConnectToService(std::move(client), "bootloader",
                            [&](auto result) { ASSERT_FALSE(result.is_err()); });
  // Wait for reboot to complete.
  RunLoopUntil([&]() { return local_power_state_control_ptr_->reboot_complete.load(); });
}

TEST_F(AdbRebootTest, RebootRecovery) {
  local_power_state_control_ptr_->ExpectRebootRecovery();

  zx::socket server, client;
  ASSERT_EQ(zx::socket::create(ZX_SOCKET_STREAM, &server, &client), ZX_OK);

  reboot_->ConnectToService(std::move(client), "recovery",
                            [&](auto result) { ASSERT_FALSE(result.is_err()); });
  // Wait for reboot to complete.
  RunLoopUntil([&]() { return local_power_state_control_ptr_->reboot_complete.load(); });
}

TEST_F(AdbRebootTest, RebootFastboot) {
  local_power_state_control_ptr_->ExpectRebootRecovery();

  zx::socket server, client;
  ASSERT_EQ(zx::socket::create(ZX_SOCKET_STREAM, &server, &client), ZX_OK);

  reboot_->ConnectToService(std::move(client), "fastboot",
                            [&](auto result) { ASSERT_FALSE(result.is_err()); });
  // Wait for reboot to complete.
  RunLoopUntil([&]() { return local_power_state_control_ptr_->reboot_complete.load(); });

  bootloader_message message;
  fake_block_device_ptr_->ReadVmo(&message, 0, sizeof(message));
  ASSERT_STREQ(message.command, "boot-recovery");
  ASSERT_STREQ(message.recovery, "recovery\n--fastboot\n");
}

}  // namespace adb_reboot
