// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "network_device.h"

#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "device/test_session.h"
#include "device/test_util.h"
#include "mac/test_util.h"
#include "src/lib/testing/predicates/status.h"

namespace {
// Enable timeouts only to test things locally, committed code should not use timeouts.
constexpr zx::duration kTestTimeout = zx::duration::infinite();
}  // namespace

namespace network::testing {

// The environment represents what exists before the driver starts. This includes the parent driver
// which implements NetworkDeviceImpl. In the tests this is done by FakeNetworkDeviceImpl, therefore
// an object of that type must live in the environment to provide a discoverable service for the
// network device driver to connect to.
struct TestFixtureEnvironment : public fdf_testing::Environment {
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing) override {
    zx::result result = outgoing.AddService<fuchsia_hardware_network_driver::Service>(
        fuchsia_hardware_network_driver::Service::InstanceHandler(
            {.network_device_impl =
                 device_impl_.bind_handler(fdf::Dispatcher::GetCurrent()->get())}));
    if (result.is_error()) {
      return result;
    }
    return zx::ok();
  }

  FakeNetworkDeviceImpl device_impl_{fdf::Dispatcher::GetCurrent()->get()};
};

struct TestFixtureConfig {
  using DriverType = network::NetworkDevice;
  using EnvironmentType = TestFixtureEnvironment;
};

class NetDeviceDriverTest : public ::testing::Test {
 public:
  // Use a nonzero port identifier to avoid default value traps.
  static constexpr uint8_t kPortId = 11;

  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    ShutdownDriver();
    port_impl_.WaitPortRemoved();
  }

  void ShutdownDriver() {
    if (shutdown_) {
      return;
    }
    EXPECT_OK(driver_test().StopDriver().status_value());
    shutdown_ = true;
  }

  zx_status_t CreateDevice(bool with_mac = false) {
    if (with_mac) {
      port_impl_.SetMac(mac_impl_.Bind(port_dispatcher_));
    }
    port_impl_.SetStatus({.mtu = 2048, .flags = netdev::wire::StatusFlags::kOnline});

    fidl::WireSyncClient<netdev::Device> client;
    driver_test().RunInDriverContext([&](network::NetworkDevice& driver) {
      auto [client_end, server_end] = fidl::Endpoints<netdev::Device>::Create();
      driver.GetInterface()->Bind(std::move(server_end));
      client.Bind(std::move(client_end));
    });
    return driver_test().RunInEnvironmentTypeContext<zx_status_t>([&](TestFixtureEnvironment& env) {
      return port_impl_.AddPort(kPortId, port_dispatcher_, std::move(client),
                                env.device_impl_.client());
    });
  }

  zx::result<fidl::WireSyncClient<netdev::Device>> ConnectNetDevice() {
    auto [dev_client_end, dev_server_end] = fidl::Endpoints<netdev::Device>::Create();
    driver_test().RunInDriverContext(
        [server_end = std::move(dev_server_end)](network::NetworkDevice& driver) mutable {
          driver.GetInterface()->Bind(std::move(server_end));
        });

    return zx::ok(fidl::WireSyncClient(std::move(dev_client_end)));
  }

  FakeNetworkPortImpl& port_impl() { return port_impl_; }

  zx::result<netdev::wire::PortId> GetSaltedPortId(uint8_t base_id) {
    // List all existing ports from the device until we find the right port id.
    zx::result connect_result = ConnectNetDevice();
    if (connect_result.is_error()) {
      return connect_result.take_error();
    }
    fidl::WireSyncClient<netdev::Device>& netdevice = connect_result.value();
    auto [client_end, server_end] = fidl::Endpoints<netdev::PortWatcher>::Create();
    if (zx_status_t status = netdevice->GetPortWatcher(std::move(server_end)).status();
        status != ZX_OK) {
      return zx::error(status);
    }
    fidl::WireSyncClient watcher{std::move(client_end)};
    for (;;) {
      fidl::WireResult result = watcher->Watch();
      if (!result.ok()) {
        return zx::error(result.status());
      }
      const netdev::wire::DevicePortEvent& event = result.value().event;

      netdev::wire::PortId id;
      switch (event.Which()) {
        case netdev::wire::DevicePortEvent::Tag::kAdded:
          id = event.added();
          break;
        case netdev::wire::DevicePortEvent::Tag::kExisting:
          id = event.existing();
          break;
        case netdev::wire::DevicePortEvent::Tag::kIdle:
        case netdev::wire::DevicePortEvent::Tag::kRemoved:
          ADD_FAILURE() << "Unexpected port watcher event " << static_cast<uint32_t>(event.Which());
          return zx::error(ZX_ERR_INTERNAL);
      }
      if (id.base == base_id) {
        return zx::ok(id);
      }
    }
  }

  zx_status_t AttachSessionPort(TestSession& session, FakeNetworkPortImpl& impl) {
    std::vector<netdev::wire::FrameType> rx_types = impl.port_info().rx_types;
    zx::result port_id = GetSaltedPortId(impl.id());
    if (port_id.is_error()) {
      return port_id.status_value();
    }
    return session.AttachPort(port_id.value(), std::move(rx_types));
  }

  zx_status_t WaitForEvent(zx_signals_t event, zx::duration timeout = kTestTimeout) {
    // Don't wait inside the environment context as that might be running on the dispatcher that
    // triggers the event. Waiting on it would then deadlock.
    zx::unowned_event events = driver_test().RunInEnvironmentTypeContext<zx::unowned_event>(
        [&](TestFixtureEnvironment& env) { return env.device_impl_.events().borrow(); });
    return events->wait_one(event, zx::deadline_after(timeout), nullptr);
  }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<TestFixtureConfig> driver_test_;
  bool shutdown_ = false;
  async_dispatcher_t* fidl_dispatcher_ =
      driver_test().runtime().StartBackgroundDispatcher()->async_dispatcher();
  fdf_dispatcher_t* port_dispatcher_ = driver_test().runtime().StartBackgroundDispatcher()->get();
  FakeMacDeviceImpl mac_impl_;
  FakeNetworkPortImpl port_impl_;
};

TEST_F(NetDeviceDriverTest, TestCreateSimple) { ASSERT_OK(this->CreateDevice()); }

TEST_F(NetDeviceDriverTest, TestOpenSession) {
  ASSERT_OK(this->CreateDevice());
  TestSession session;
  zx::result connect_result = this->ConnectNetDevice();
  ASSERT_OK(connect_result.status_value());
  fidl::WireSyncClient<netdev::Device>& netdevice = connect_result.value();
  ASSERT_OK(session.Open(netdevice, "test-session"));
  ASSERT_OK(this->AttachSessionPort(session, this->port_impl()));
  ASSERT_OK(this->WaitForEvent(kEventStartInitiated));
  this->ShutdownDriver();
  ASSERT_OK(session.WaitClosed(zx::deadline_after(kTestTimeout)));
  // netdevice should also have been closed after device unbind:
  ASSERT_OK(netdevice.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                      zx::deadline_after(kTestTimeout), nullptr));
}

TEST_F(NetDeviceDriverTest, TestWatcherDestruction) {
  // Test that on device removal watcher channels get closed.
  ASSERT_OK(this->CreateDevice());

  zx::result connect_result = this->ConnectNetDevice();
  ASSERT_OK(connect_result.status_value());
  fidl::WireSyncClient<netdev::Device>& netdevice = connect_result.value();

  zx::result maybe_port_id = this->GetSaltedPortId(this->kPortId);
  ASSERT_OK(maybe_port_id.status_value());
  const netdev::wire::PortId& port_id = maybe_port_id.value();

  auto [port_client_end, port_server_end] = fidl::Endpoints<netdev::Port>::Create();
  ASSERT_OK(netdevice->GetPort(port_id, std::move(port_server_end)).status());
  fidl::WireSyncClient port{std::move(port_client_end)};

  auto [client_end, server_end] = fidl::Endpoints<netdev::StatusWatcher>::Create();
  ASSERT_OK(port->GetStatusWatcher(std::move(server_end), 1).status());
  fidl::WireSyncClient watcher{std::move(client_end)};
  ASSERT_OK(watcher->WatchStatus().status());
  this->ShutdownDriver();
  // Watcher, port, and netdevice should all observe channel closure.
  ASSERT_OK(watcher.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                    zx::deadline_after(kTestTimeout), nullptr));
  ASSERT_OK(port.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                 zx::deadline_after(kTestTimeout), nullptr));
  ASSERT_OK(netdevice.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                      zx::deadline_after(kTestTimeout), nullptr));
}

}  // namespace network::testing
