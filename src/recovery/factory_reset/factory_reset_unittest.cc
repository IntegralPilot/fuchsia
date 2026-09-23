// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "factory_reset.h"

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.fshost/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire_test_base.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>
#include <lib/zx/vmo.h>
#include <zircon/hw/gpt.h>

#include <string_view>

#include <fbl/algorithm.h>
#include <gtest/gtest.h>

#include "gmock/gmock.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace {

class MockAdmin : public fidl::testing::WireTestBase<fuchsia_hardware_power_statecontrol::Admin> {
 public:
  bool suspend_called() const { return suspend_called_; }

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "'" << name << "' was called unexpectedly";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Shutdown(ShutdownRequestView request, ShutdownCompleter::Sync& completer) override {
    ASSERT_FALSE(suspend_called_);
    suspend_called_ = true;
    ASSERT_TRUE(request->options.has_reasons());
    ASSERT_TRUE(request->options.has_action());
    ASSERT_EQ(request->options.action(),
              fuchsia_hardware_power_statecontrol::ShutdownAction::kReboot);
    ASSERT_THAT(request->options.reasons(),
                testing::ElementsAre(
                    fuchsia_hardware_power_statecontrol::ShutdownReason::kFactoryDataReset));
    completer.ReplySuccess();
  }

  bool suspend_called_ = false;
};

class MockFshostAdmin : public fidl::testing::WireTestBase<fuchsia_fshost::Admin> {
 public:
  bool shred_data_volume_called() const { return shred_data_volume_called_; }

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "'" << name << "' was called unexpectedly";
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void ShredDataVolume(ShredDataVolumeCompleter::Sync& completer) override {
    shred_data_volume_called_ = true;
    completer.ReplySuccess();
  }

  bool shred_data_volume_called_ = false;
};

class FactoryResetTest : public gtest::RealLoopFixture {
 protected:
  // Create an IsolatedDevmgr that can load device drivers such as fvm,
  // zxcrypt, etc.
  void SetUp() override {
    // No isolated devmgr or ramdisk needed for mock tests.
  }

  void RunReset(fit::function<void(const MockAdmin&)> cb,
                std::optional<std::reference_wrapper<MockFshostAdmin>> mock_fshost = std::nullopt) {
    fidl::ClientEnd<fuchsia_io::Directory> dev;
    MockAdmin mock_admin;
    fidl::ServerBindingGroup<fuchsia_hardware_power_statecontrol::Admin> binding;
    auto [admin, server_end] =
        fidl::Endpoints<fuchsia_hardware_power_statecontrol::Admin>::Create();
    binding.AddBinding(dispatcher(), std::move(server_end), &mock_admin,
                       fidl::kIgnoreBindingClosure);

    std::optional<fidl::ServerBindingGroup<fuchsia_fshost::Admin>> fshost_binding;
    fidl::ClientEnd<fuchsia_fshost::Admin> fshost_admin;
    ASSERT_TRUE(mock_fshost.has_value());
    zx::result fshost_server_end = fidl::CreateEndpoints<fuchsia_fshost::Admin>(&fshost_admin);
    ASSERT_TRUE(fshost_server_end.is_ok()) << fshost_server_end.status_string();
    fshost_binding.emplace().AddBinding(dispatcher(), std::move(fshost_server_end.value()),
                                        &mock_fshost.value().get(), fidl::kIgnoreBindingClosure);

    factory_reset::FactoryReset reset(dispatcher(), std::move(dev), std::move(admin),
                                      std::move(fshost_admin), {});

    std::optional<zx_status_t> status;
    reset.Reset([&status](zx_status_t s) { status = s; });
    RunLoopUntil([&status]() { return status.has_value(); });
    EXPECT_EQ(status.value(), ZX_OK);

    cb(mock_admin);
  }

 private:
};

TEST_F(FactoryResetTest, ShredUsingFshostMock) {
  // For now, the fshost component in the test environment does not support the ShredDataVolume
  // method, so this tests that we actually call that method.  The other tests are all testing that
  // the fallback behaviour works as intended.

  MockFshostAdmin mock_fshost;
  RunReset([](const MockAdmin& mock_admin) { EXPECT_TRUE(mock_admin.suspend_called()); },
           mock_fshost);
  EXPECT_TRUE(mock_fshost.shred_data_volume_called());
}

}  // namespace
