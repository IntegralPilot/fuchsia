// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.component.test/cpp/driver/wire.h>
#include <fidl/fuchsia.driver.component.test/cpp/wire.h>
#include <lib/driver/component/cpp/tests/test_driver.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>

#include <gtest/gtest.h>

class ZirconProtocolServer
    : public fidl::WireServer<fuchsia_driver_component_test::ZirconProtocol> {
 public:
  fuchsia_driver_component_test::ZirconService::InstanceHandler GetInstanceHandler() {
    return fuchsia_driver_component_test::ZirconService::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

 private:
  void ZirconMethod(ZirconMethodCompleter::Sync& completer) override { completer.ReplySuccess(); }
  fidl::ServerBindingGroup<fuchsia_driver_component_test::ZirconProtocol> bindings_;
};

class DriverProtocolServer : public fdf::WireServer<fuchsia_driver_component_test::DriverProtocol> {
 public:
  fuchsia_driver_component_test::DriverService::InstanceHandler GetInstanceHandler() {
    return fuchsia_driver_component_test::DriverService::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

 private:
  void DriverMethod(fdf::Arena& arena, DriverMethodCompleter::Sync& completer) override {
    fdf::Arena reply_arena('TEST');
    completer.buffer(reply_arena).ReplySuccess();
  }

  fdf::ServerBindingGroup<fuchsia_driver_component_test::DriverProtocol> bindings_;
};

class FixtureBasedTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto result = to_driver_vfs.AddService<fuchsia_driver_component_test::ZirconService>(
        zircon_proto_server_.GetInstanceHandler());
    EXPECT_EQ(ZX_OK, result.status_value());

    result = to_driver_vfs.AddService<fuchsia_driver_component_test::DriverService>(
        driver_proto_server_.GetInstanceHandler());
    EXPECT_EQ(ZX_OK, result.status_value());

    device_server_.Initialize(component::kDefaultInstance);
    return zx::make_result(
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs));
  }

  std::string GetName() { return std::string(device_server_.name()); }

 private:
  ZirconProtocolServer zircon_proto_server_;
  DriverProtocolServer driver_proto_server_;
  compat::DeviceServer device_server_;
};

class FixtureConfig final {
 public:
  using DriverType = TestDriver;
  using EnvironmentType = FixtureBasedTestEnvironment;
};

// Demonstrates a test fixture that puts the driver on a background context. Using the driver
// requires going through |RunInDriverContext()| but sync client tasks can be ran directly on the
// main test thread.
class FixtureBasedTestBackground : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTestBackground, GetNameFromEnv) {
  driver_test().RunInEnvironmentTypeContext([](FixtureBasedTestEnvironment& env) {
    env.GetName();
    ASSERT_EQ(component::kDefaultInstance, env.GetName());
  });
  auto name = driver_test().RunInEnvironmentTypeContext<std::string>(
      [](FixtureBasedTestEnvironment& env) { return env.GetName(); });
  ASSERT_EQ(component::kDefaultInstance, name);
}

TEST_F(FixtureBasedTestBackground, ValidateDriverIncomingServices) {
  driver_test().RunInDriverContext([](TestDriver& driver) {
    zx::result result = driver.ValidateIncomingDriverService();
    ASSERT_EQ(ZX_OK, result.status_value());
    result = driver.ValidateIncomingZirconService();
    ASSERT_EQ(ZX_OK, result.status_value());
  });
}

TEST_F(FixtureBasedTestBackground, ConnectWithDevfs) {
  driver_test().RunInDriverContext([](TestDriver& driver) {
    zx::result export_result = driver.ExportDevfsNodeSync();
    ASSERT_EQ(ZX_OK, export_result.status_value());
  });

  zx::result device_result =
      driver_test().ConnectThroughDevfs<fuchsia_driver_component_test::ZirconProtocol>(
          "devfs_node");
  ASSERT_EQ(ZX_OK, device_result.status_value());

  fidl::WireSyncClient zircon_proto_client(std::move(device_result.value()));
  fidl::WireResult result = zircon_proto_client->ZirconMethod();
  ASSERT_EQ(ZX_OK, result.status());
  ASSERT_EQ(true, result.value().is_ok());
}

TEST_F(FixtureBasedTestBackground, ConnectWithZirconService) {
  driver_test().RunInDriverContext([](TestDriver& driver) {
    zx::result serve_result = driver.ServeZirconService();
    ASSERT_EQ(ZX_OK, serve_result.status_value());
  });

  zx::result result = driver_test().Connect<fuchsia_driver_component_test::ZirconService::Device>();
  ASSERT_EQ(ZX_OK, result.status_value());

  fidl::WireResult wire_result = fidl::WireCall(result.value())->ZirconMethod();
  ASSERT_EQ(ZX_OK, wire_result.status());
  ASSERT_EQ(true, wire_result.value().is_ok());
}

TEST_F(FixtureBasedTestBackground, ConnectWithDriverService) {
  driver_test().RunInDriverContext([](TestDriver& driver) {
    zx::result serve_result = driver.ServeDriverService();
    ASSERT_EQ(ZX_OK, serve_result.status_value());
  });

  zx::result driver_connect_result =
      driver_test().Connect<fuchsia_driver_component_test::DriverService::Device>();
  ASSERT_EQ(ZX_OK, driver_connect_result.status_value());

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult wire_result =
      fdf::WireCall(driver_connect_result.value()).buffer(arena)->DriverMethod();
  ASSERT_EQ(ZX_OK, wire_result.status());
  ASSERT_EQ(true, wire_result.value().is_ok());
}

// Demonstrates a test fixture that puts the driver on the foreground context. Using the driver
// is simply done using the |driver()| getter but sync client tasks must be ran in the background
// using |RunOnBackgroundDispatcherSync()|.
class FixtureBasedTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTest, GetNameFromEnv) {
  driver_test().RunInEnvironmentTypeContext([](FixtureBasedTestEnvironment& env) {
    env.GetName();
    ASSERT_EQ(component::kDefaultInstance, env.GetName());
  });
  auto name = driver_test().RunInEnvironmentTypeContext<std::string>(
      [](FixtureBasedTestEnvironment& env) { return env.GetName(); });
  ASSERT_EQ(component::kDefaultInstance, name);
}

TEST_F(FixtureBasedTest, ValidateDriverIncomingServices) {
  zx::result result = driver_test().driver()->ValidateIncomingDriverService();
  ASSERT_EQ(ZX_OK, result.status_value());
  result = driver_test().driver()->ValidateIncomingZirconService();
  ASSERT_EQ(ZX_OK, result.status_value());

  // Check the bad instance paths.
  result = driver_test().driver()->ValidateIncomingDriverService("bad_instance");
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, result.status_value());
  result = driver_test().driver()->ValidateIncomingZirconService("bad_instance");
  ASSERT_EQ(ZX_ERR_PEER_CLOSED, result.status_value());

  // Restart it to enable service validation.
  result = driver_test().StopDriver();
  ASSERT_EQ(ZX_OK, result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  driver_test().SetServiceValidator(true);
  result = driver_test().StartDriver();
  ASSERT_EQ(ZX_OK, result.status_value());

  // Check the valid instances again.
  result = driver_test().driver()->ValidateIncomingDriverService();
  ASSERT_EQ(ZX_OK, result.status_value());
  result = driver_test().driver()->ValidateIncomingZirconService();
  ASSERT_EQ(ZX_OK, result.status_value());

  // Check the bad instance.
  result = driver_test().driver()->ValidateIncomingDriverService("bad_instance");
  ASSERT_EQ(ZX_ERR_NOT_FOUND, result.status_value());
  result = driver_test().driver()->ValidateIncomingZirconService("bad_instance");
  ASSERT_EQ(ZX_ERR_NOT_FOUND, result.status_value());
}

TEST_F(FixtureBasedTest, ConnectWithDevfs) {
  zx::result export_result = driver_test().driver()->ExportDevfsNodeSync();
  ASSERT_EQ(ZX_OK, export_result.status_value());

  zx::result device_result =
      driver_test().ConnectThroughDevfs<fuchsia_driver_component_test::ZirconProtocol>(
          "devfs_node");
  ASSERT_EQ(ZX_OK, device_result.status_value());

  zx::result run_result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = std::move(device_result.value())]() mutable {
        fidl::WireSyncClient zircon_proto_client(std::move(client_end));
        fidl::WireResult result = zircon_proto_client->ZirconMethod();
        ASSERT_EQ(ZX_OK, result.status());
        ASSERT_EQ(true, result.value().is_ok());
      });
  ASSERT_EQ(ZX_OK, run_result.status_value());
}

TEST_F(FixtureBasedTest, ConnectWithZirconService) {
  zx::result serve_result = driver_test().driver()->ServeZirconService();
  ASSERT_EQ(ZX_OK, serve_result.status_value());

  zx::result result = driver_test().Connect<fuchsia_driver_component_test::ZirconService::Device>();
  ASSERT_EQ(ZX_OK, result.status_value());

  zx::result run_result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = std::move(result.value())]() mutable {
        fidl::WireResult wire_result = fidl::WireCall(client_end)->ZirconMethod();
        ASSERT_EQ(ZX_OK, wire_result.status());
        ASSERT_EQ(true, wire_result.value().is_ok());
      });
  ASSERT_EQ(ZX_OK, run_result.status_value());
}

TEST_F(FixtureBasedTest, ConnectWithDriverService) {
  zx::result serve_result = driver_test().driver()->ServeDriverService();
  ASSERT_EQ(ZX_OK, serve_result.status_value());

  zx::result driver_connect_result =
      driver_test().Connect<fuchsia_driver_component_test::DriverService::Device>();
  ASSERT_EQ(ZX_OK, driver_connect_result.status_value());

  zx::result run_result = driver_test().RunOnBackgroundDispatcherSync(
      [client_end = std::move(driver_connect_result.value())]() mutable {
        fdf::Arena arena('TEST');
        fdf::WireUnownedResult wire_result =
            fdf::WireCall(client_end).buffer(arena)->DriverMethod();
        ASSERT_EQ(ZX_OK, wire_result.status());
        ASSERT_EQ(true, wire_result.value().is_ok());
      });
  ASSERT_EQ(ZX_OK, run_result.status_value());
}

// Demonstrates a test fixture that tests out the manual stop and shutdown feature. Validates by
// checking the a global that gets set in the driver header.
class FixtureBasedTestManualStop : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTestManualStop, ShutdownAndCheckLogger) {
  ASSERT_EQ(false, g_driver_stopped);
  ASSERT_EQ(ZX_OK, driver_test().StopDriver().status_value());
  ASSERT_EQ(false, g_driver_stopped);
  driver_test().ShutdownAndDestroyDriver();
  ASSERT_EQ(true, g_driver_stopped);
}

class AddChildFixtureConfig final {
 public:
  using DriverType = TestDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

// Checks that adding a child and then managing it by the driver works.
class FixtureBasedTestAddChild : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::ForegroundDriverTest<AddChildFixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<AddChildFixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTestAddChild, AddChild) {
  EXPECT_EQ(ZX_OK, driver_test().driver()->InitSyncCompat().status_value());
  driver_test().driver()->CreateChildNodeSync();
  EXPECT_TRUE(driver_test().driver()->sync_added_child());
}

// Fixture config for testing start failure scenarios.
class FailStartFixtureConfig final {
 public:
  using DriverType = StartFailTestDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class FixtureBasedTestFailStart : public ::testing::Test {
 public:
  fdf_testing::ForegroundDriverTest<FailStartFixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FailStartFixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTestFailStart, FailStart) {
  auto start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_ERR_INTERNAL, start_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_ERR_INTERNAL, start_result.status_value());
  // calling StopDriver is optional and a no-op if start failed. Returns zx::ok in this case.
  auto stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_ERR_INTERNAL, start_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
}

// Tests that require manual start and stop calls. With the driver on the background.
class FixtureBasedTestManualBackground : public ::testing::Test {
 public:
  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTestManualBackground, MultiStart) {
  auto start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_OK, start_result.status_value());
  auto stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_OK, start_result.status_value());
  stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_OK, start_result.status_value());
  stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
}

// Tests that require manual start and stop calls. With the driver on the foreground.
class FixtureBasedTestManualForeground : public ::testing::Test {
 public:
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(FixtureBasedTestManualForeground, MultiStartAndValidateIncoming) {
  auto start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_OK, start_result.status_value());
  zx::result validate_result = driver_test().driver()->ValidateIncomingDriverService();
  ASSERT_EQ(ZX_OK, validate_result.status_value());
  auto stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_OK, start_result.status_value());
  validate_result = driver_test().driver()->ValidateIncomingZirconService();
  ASSERT_EQ(ZX_OK, validate_result.status_value());
  stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
  start_result = driver_test().StartDriver();
  EXPECT_EQ(ZX_OK, start_result.status_value());
  validate_result = driver_test().driver()->ValidateIncomingDriverService();
  ASSERT_EQ(ZX_OK, validate_result.status_value());
  stop_result = driver_test().StopDriver();
  EXPECT_EQ(ZX_OK, stop_result.status_value());
  driver_test().ShutdownAndDestroyDriver();
}
