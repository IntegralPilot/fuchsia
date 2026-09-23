// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power.battery/cpp/fidl.h>
#include <fidl/fuchsia.power.battery/cpp/fidl.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <gtest/gtest.h>

class FakeBatteryRealmTest : public ::testing::Test {
 public:
 protected:
  void SetUp() override {
    loop_.StartThread();

    std::vector<fuchsia_component_test::Capability> exposes;
    exposes.emplace_back(fuchsia_component_test::Capability::WithService(
        fuchsia_component_test::Service{{.name = fuchsia_power_battery::InfoService::Name}}));
    exposes.emplace_back(fuchsia_component_test::Capability::WithService(
        fuchsia_component_test::Service{{.name = fuchsia_hardware_power_battery::Service::Name}}));

    fuchsia_driver_test::RealmArgs args{{
        .root_driver = "fuchsia-boot:///platform-bus#meta/platform-bus.cm",
        .software_devices = std::vector{fuchsia_driver_test::SoftwareDevice(
            "fake-battery", bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_FAKE_BATTERY)},
    }};

    // Create and build the realm.
    auto realm_builder = component_testing::RealmBuilder::Create();
    driver_test_realm::Setup(realm_builder, loop_.dispatcher(),
                             driver_test_realm::OptionsBuilder().driver_exposes(exposes).Build(),
                             std::move(args));

    realm_ = realm_builder.Build(loop_.dispatcher());

    // Start DriverTestRealm.
    zx::result<> boot_result = driver_test_realm::WaitForBootup(*realm_);
    ASSERT_EQ(ZX_OK, boot_result.status_value());
  }

  component_testing::RealmRoot& Realm() { return *realm_; }

  void TearDown() override { driver_test_realm::ShutdownRealm(*realm_); }

 private:
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::optional<component_testing::RealmRoot> realm_;
};

TEST_F(FakeBatteryRealmTest, LegacyBatteryDriversExist) {
  fidl::ClientEnd<fuchsia_io::Directory> svc_root(
      Realm().component().CloneExposedDir().TakeChannel());
  component::SyncServiceMemberWatcher<fuchsia_power_battery::InfoService::Device> watcher(
      svc_root.borrow());

  zx::result result1 = watcher.GetNextInstance(false);
  ASSERT_TRUE(result1.is_ok());

  auto client_end = std::move(result1.value());
  fidl::WireSyncClient client(std::move(client_end));

  // Send a FIDL request.
  fidl::WireResult result2 = client->GetBatteryInfo();
  ASSERT_EQ(ZX_OK, result2.status());
  const auto& info = result2.value().info;
  ASSERT_EQ(info.charge_source(), fuchsia_power_battery::ChargeSource::kAcAdapter);
}

TEST_F(FakeBatteryRealmTest, HardwareBatteryDriversExist) {
  fidl::ClientEnd<fuchsia_io::Directory> svc_root(
      Realm().component().CloneExposedDir().TakeChannel());
  component::SyncServiceMemberWatcher<fuchsia_hardware_power_battery::Service::Battery> watcher(
      svc_root.borrow());

  zx::result result1 = watcher.GetNextInstance(false);
  ASSERT_TRUE(result1.is_ok());

  auto client_end = std::move(result1.value());
  fidl::WireSyncClient client(std::move(client_end));

  // Send a FIDL request.
  fidl::WireResult result2 = client->GetStatus();
  ASSERT_EQ(ZX_OK, result2.status());
  ASSERT_TRUE(result2->is_ok());
  const auto& status = result2.value()->status;
  ASSERT_TRUE(status.has_level_percent());
}
