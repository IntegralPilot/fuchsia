// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <wlan/drivers/components/network_port.h>

#include "src/lib/testing/predicates/status.h"
#include "test_driver.h"
#include "test_network_device_ifc.h"

namespace {

using wlan::drivers::components::NetworkPort;
using wlan::drivers::components::test::TestNetworkDeviceIfc;

constexpr uint32_t kTestMtu = 1514;

class TestNetworkPortInterface : public NetworkPort::Callbacks {
 public:
  void SetMtu(uint32_t mtu) { mtu_ = mtu; }

  // NetworkPort::Callbacks implementation and mocks
  uint32_t PortGetMtu() override { return mtu_; }
  MOCK_METHOD(void, PortGetStatus, (fuchsia_hardware_network::PortStatus * out_status), (override));
  MOCK_METHOD(void, PortRemoved, (), (override));
  MOCK_METHOD(void, MacGetAddress, (fuchsia_net::MacAddress * out_mac), (override));
  MOCK_METHOD(void, MacGetFeatures, (fuchsia_hardware_network_driver::Features * out_features),
              (override));
  MOCK_METHOD(void, MacSetMode,
              (fuchsia_hardware_network::wire::MacFilterMode mode,
               cpp20::span<const ::fuchsia_net::wire::MacAddress> multicast_macs),
              (override));

  uint32_t mtu_ = kTestMtu;
};

struct TestFixtureConfig {
  using DriverType = wlan::drivers::components::test::TestDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

struct NetworkPortTest : public ::testing::Test {
  static constexpr uint8_t kPortId = 13;

  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
    auto client_end = netdev_ifc_.Bind(driver_test().runtime().StartBackgroundDispatcher()->get());
    ASSERT_OK(client_end.status_value());
    ifc_client_.Bind(std::move(*client_end),
                     driver_test().runtime().StartBackgroundDispatcher()->get());
    ASSERT_TRUE(ifc_client_.is_valid());
  }

  void TearDown() override {
    if (port_) {
      // The port must be removed before it is destroyed by the default destructor. Don't worry
      // about the status here. Some tests will have already removed the port which will cause this
      // to fail and other tests should verify that the status is ZX_OK when the removal works.
      libsync::Completion port_removed;
      // We should call port |Init| and |RemovePort| operations from the same driver context
      // as the port dispatcher. This can be emulated by the test by posting a task to the port
      // dispatcher.
      //
      // This allows the driver runtime to detect when a driver would be reentrantly calling
      // to itself, and skip inlining the call.
      //
      // For example, |port_->RemovePort| will call the |netdev_ifc->RemovePort|,
      // which leads to |port_->Removed| being called. Since |RemovePort| and |Removed| both
      // acquire the port lock, we do not want the sequence of 3 calls to be inlined by
      // the driver runtime.
      async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_),
                      [&] { port_->RemovePort([&](zx_status_t) { port_removed.Signal(); }); });
      port_removed.Wait();
    }
    ASSERT_OK(driver_test().StopDriver().status_value());
  }

  NetworkPort& CreatePort(uint8_t port_id) {
    port_ = std::make_unique<NetworkPort>(std::move(ifc_client_), port_ifc_, port_id);
    return *port_;
  }

  zx_status_t InitPort(NetworkPort& port, NetworkPort::Role role = NetworkPort::Role::Client) {
    zx_status_t result = ZX_OK;
    libsync::Completion initialized;
    async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_), [&] {
      port.Init(role, dispatcher_, [&](zx_status_t status) {
        result = status;
        initialized.Signal();
      });
    });
    initialized.Wait();
    return result;
  }

  fdf_testing::BackgroundDriverTest<TestFixtureConfig>& driver_test() { return driver_test_; }
  fdf_testing::BackgroundDriverTest<TestFixtureConfig> driver_test_;

  TestNetworkPortInterface port_ifc_;
  TestNetworkDeviceIfc netdev_ifc_;
  std::unique_ptr<NetworkPort> port_;
  fdf_dispatcher_t* dispatcher_ = driver_test().runtime().StartBackgroundDispatcher()->get();
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> ifc_client_;
};

TEST_F(NetworkPortTest, Init) {
  constexpr uint8_t kPortId = 13;

  NetworkPort& port = CreatePort(kPortId);

  libsync::Completion add_port_called;
  netdev_ifc_.add_port_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest* request,
          fdf::Arena& arena, auto& completer) {
        EXPECT_EQ(request->id, kPortId);
        ASSERT_TRUE(request->port.is_valid());
        completer.buffer(arena).Reply(ZX_OK);
        add_port_called.Signal();
      };

  ASSERT_OK(InitPort(port));

  add_port_called.Wait();
}

TEST_F(NetworkPortTest, InitInvalidIfc) {
  constexpr uint8_t kPortId = 13;

  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> invalid_client;
  ASSERT_FALSE(invalid_client.is_valid());

  {
    NetworkPort port(std::move(invalid_client), port_ifc_, kPortId);

    // Should not be called because the ifc wasn't valid to begin with.
    netdev_ifc_.add_port_ =
        [](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest*, fdf::Arena&,
           auto&) { ADD_FAILURE() << "AddPort should NOT be called!"; };
    // Should not be called because the ifc wasn't valid to begin with.
    EXPECT_CALL(port_ifc_, PortRemoved).Times(0);

    ASSERT_EQ(InitPort(port), ZX_ERR_BAD_STATE);
  }
}

TEST_F(NetworkPortTest, InitAddPortFails) {
  constexpr uint8_t kPortId = 13;
  constexpr zx_status_t kAddPortError = ZX_ERR_INTERNAL;

  NetworkPort& port = CreatePort(kPortId);

  libsync::Completion add_port_called;
  netdev_ifc_.add_port_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest*, fdf::Arena& arena,
          auto& completer) {
        completer.buffer(arena).Reply(kAddPortError);
        add_port_called.Signal();
      };

  ASSERT_EQ(InitPort(port), kAddPortError);

  add_port_called.Wait();
}

TEST_F(NetworkPortTest, InitFailsAsyncAndDestroysPort) {
  constexpr uint8_t kPortId = 13;
  constexpr zx_status_t kAddPortError = ZX_ERR_INTERNAL;

  CreatePort(kPortId);

  netdev_ifc_.add_port_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest*, fdf::Arena& arena,
          auto& completer) { completer.buffer(arena).Reply(kAddPortError); };

  // Verify that the NetworkPort object can safely be destroyed inside the Init on_complete
  // callback if Init fails. This is important especially for failure cases where the caller might
  // want to discard a useless port if initialization failed.
  libsync::Completion initialized;
  port_->Init(NetworkPort::Role::Client, dispatcher_, [&](zx_status_t status) mutable {
    EXPECT_EQ(status, kAddPortError);
    port_.reset();
    initialized.Signal();
  });
  initialized.Wait();
}

TEST_F(NetworkPortTest, InitFailsInlineAndDestroysPort) {
  constexpr uint8_t kPortId = 13;

  // Verify that the NetworkPort object can safely be destroyed inside the Init on_complete
  // callback if Init fails inline. This verifies that the recursive call to the destructor works in
  // when the initialization fails before even making any asynchronous calls.
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> invalid_client;
  ASSERT_FALSE(invalid_client.is_valid());

  auto port = std::make_unique<NetworkPort>(std::move(invalid_client), port_ifc_, kPortId);

  libsync::Completion initialized;
  port->Init(NetworkPort::Role::Client, dispatcher_, [&](zx_status_t status) mutable {
    EXPECT_EQ(status, ZX_ERR_BAD_STATE);
    port.reset();
    initialized.Signal();
  });
  initialized.Wait();
}

TEST_F(NetworkPortTest, RemovePortByUser) {
  constexpr uint8_t kPortId = 13;
  NetworkPort& port = CreatePort(kPortId);

  libsync::Completion remove_port_called;
  netdev_ifc_.remove_port_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRemovePortRequest* request,
          fdf::Arena& arena, auto& completer) {
        ASSERT_EQ(request->id, kPortId);
        remove_port_called.Signal();
      };

  ASSERT_OK(InitPort(port));

  // PortRemoved should NOT be called for a user initiated port removal.
  EXPECT_CALL(port_ifc_, PortRemoved).Times(0);
  libsync::Completion remove_port_completed;
  async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_), [&]() {
    port.RemovePort([&](zx_status_t status) {
      EXPECT_OK(status);
      remove_port_completed.Signal();
    });
  });
  remove_port_called.Wait();
  remove_port_completed.Wait();
}

TEST_F(NetworkPortTest, RemovePortByNetdev) {
  constexpr uint8_t kPortId = 13;
  NetworkPort& port = CreatePort(kPortId);

  ASSERT_OK(InitPort(port));

  libsync::Completion port_removed;
  EXPECT_CALL(port_ifc_, PortRemoved).WillOnce([&] { port_removed.Signal(); });

  fdf::Arena arena('TEST');
  fidl::OneWayStatus status = netdev_ifc_.PortClient().sync().buffer(arena)->Removed();
  ASSERT_TRUE(status.ok());

  // For a port removal initiated by netdev's port client we should get a PortRemoved call.
  port_removed.Wait();
}

TEST_F(NetworkPortTest, RemovesPortInAddPortCallback) {
  constexpr uint8_t kPortId = 13;
  NetworkPort& port = CreatePort(kPortId);

  // Verify that the NetworkPort object can safely be removed inside the Init on_complete
  // callback if Init succeeds. This shouldn't be a common use case but could happen if the callback
  // performs other initialization that could fail, leaing to the removal of the port.
  libsync::Completion removed;
  async::PostTask(fdf_dispatcher_get_async_dispatcher(dispatcher_), [&]() {
    port.Init(NetworkPort::Role::Client, dispatcher_, [&](zx_status_t status) mutable {
      EXPECT_OK(status);
      port.RemovePort([&](zx_status_t status) {
        EXPECT_OK(status);
        removed.Signal();
      });
    });
  });
  removed.Wait();
}

TEST_F(NetworkPortTest, PortWithEmptyClient) {
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> empty_client;

  TestNetworkPortInterface port_ifc;
  constexpr uint8_t kPortId = 0;
  // Ensure that this object can be constructed and destructed even with an empty proto
  NetworkPort port(std::move(empty_client), port_ifc, kPortId);
}

TEST_F(NetworkPortTest, GetInfoClient) {
  constexpr uint8_t kPortId = 7;

  NetworkPort& port = CreatePort(kPortId);

  ASSERT_OK(InitPort(port));

  fdf::Arena arena('WLAN');
  auto result = netdev_ifc_.PortClient().sync().buffer(arena)->GetInfo();
  ASSERT_OK(result.status());
  EXPECT_EQ(result->info.port_class(), fuchsia_hardware_network::wire::PortClass::kWlanClient);
}

TEST_F(NetworkPortTest, GetInfoAp) {
  constexpr uint8_t kPortId = 7;

  NetworkPort& port = CreatePort(kPortId);

  ASSERT_OK(InitPort(port, NetworkPort::Role::Ap));

  fdf::Arena arena('WLAN');
  auto result = netdev_ifc_.PortClient().sync().buffer(arena)->GetInfo();
  ASSERT_OK(result.status());
  EXPECT_EQ(result->info.port_class(), fuchsia_hardware_network::wire::PortClass::kWlanAp);
}

struct NetworkPortTestWithPort : public NetworkPortTest {
  void SetUp() override {
    NetworkPortTest::SetUp();
    CreatePort(kPortId);

    ASSERT_OK(InitPort(*port_, NetworkPort::Role::Client));
  }
};

TEST_F(NetworkPortTestWithPort, PortId) { EXPECT_EQ(port_->PortId(), kPortId); }

TEST_F(NetworkPortTestWithPort, GetInfo) {
  constexpr auto kEthFrame = fuchsia_hardware_network::wire::FrameType::kEthernet;
  constexpr uint32_t kRawFrameFeature = fuchsia_hardware_network::wire::kFrameFeaturesRaw;

  fdf::Arena arena('WLAN');
  auto result = netdev_ifc_.PortClient().sync().buffer(arena)->GetInfo();
  ASSERT_OK(result.status());
  auto& info = result->info;
  // Should be a WLAN port in the default setting
  EXPECT_EQ(info.port_class(), fuchsia_hardware_network::wire::PortClass::kWlanClient);

  // Must support at least reception of ethernet frames
  const auto& rx_types = info.rx_types();
  EXPECT_NE(std::find(rx_types.begin(), rx_types.end(), kEthFrame), rx_types.end());

  // Must support at least transmission of raw ethernet frames
  auto is_raw_ethernet = [&](const fuchsia_hardware_network::wire::FrameTypeSupport& support) {
    return support.features == kRawFrameFeature && support.type == kEthFrame;
  };
  const auto& tx_types = info.tx_types();
  EXPECT_NE(std::find_if(tx_types.begin(), tx_types.end(), is_raw_ethernet), tx_types.end());
}

TEST_F(NetworkPortTestWithPort, GetPortStatus) {
  ASSERT_FALSE(port_->IsOnline());

  libsync::Completion port_get_status_called;
  EXPECT_CALL(port_ifc_, PortGetStatus).WillOnce([&](fuchsia_hardware_network::PortStatus* status) {
    ASSERT_NE(status, nullptr);
    port_get_status_called.Signal();
  });

  fdf::Arena arena('WLAN');
  auto result = netdev_ifc_.PortClient().sync().buffer(arena)->GetStatus();
  ASSERT_OK(result.status());
  // After construction status should be offline, which means no flags are set.
  const auto& status = result->status;
  EXPECT_EQ(status.flags(), fuchsia_hardware_network::wire::StatusFlags{});
  EXPECT_EQ(status.mtu(), kTestMtu);
  port_get_status_called.Wait();
  // Testing of online status in next test, this just verifies correct propagation of calls.
}

TEST_F(NetworkPortTestWithPort, PortStatus) {
  constexpr auto kOnline = fuchsia_hardware_network::wire::StatusFlags::kOnline;
  constexpr uint32_t kModifiedMtu = kTestMtu + 3;

  ASSERT_FALSE(port_->IsOnline());
  libsync::Completion port_get_status_called;
  EXPECT_CALL(port_ifc_, PortGetStatus).WillOnce([&](fuchsia_hardware_network::PortStatus* status) {
    EXPECT_EQ(status->mtu(), kTestMtu);
    EXPECT_EQ(status->flags(), kOnline);
    status->mtu().value() = kModifiedMtu;
    port_get_status_called.Signal();
  });
  // When the port goes online it should call PortStatusChanged and the flags field should now
  // indicate that the port is online.
  libsync::Completion port_status_changed;
  netdev_ifc_.port_status_changed_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
          fdf::Arena& arena, auto& completer) {
        EXPECT_EQ(request->id, kPortId);
        EXPECT_EQ(request->new_status.mtu(), kModifiedMtu);
        EXPECT_EQ(request->new_status.flags(), kOnline);
        port_status_changed.Signal();
      };
  port_->SetPortOnline(true);
  port_status_changed.Wait();
  // Ensure that the interface implementation gets to modify the same status if it wants to.
  EXPECT_TRUE(port_->IsOnline());
  port_status_changed.Reset();

  port_get_status_called.Wait();

  // Setting the port status to online again should NOT have any effect or call anything.
  EXPECT_CALL(port_ifc_, PortGetStatus).Times(0);
  port_->SetPortOnline(true);
  EXPECT_TRUE(port_->IsOnline());
  ASSERT_FALSE(port_status_changed.signaled());

  // Setting the port to offline should clear the flags field.
  netdev_ifc_.port_status_changed_ =
      [&](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
          fdf::Arena& arena, auto& completer) {
        EXPECT_EQ(request->id, kPortId);
        EXPECT_EQ(request->new_status.flags(), fuchsia_hardware_network::wire::StatusFlags{});
        port_status_changed.Signal();
      };
  EXPECT_CALL(port_ifc_, PortGetStatus).WillOnce([&](fuchsia_hardware_network::PortStatus* status) {
    EXPECT_EQ(status->flags(), fuchsia_hardware_network::wire::StatusFlags{});
    port_get_status_called.Signal();
  });
  port_->SetPortOnline(false);
  port_status_changed.Wait();
  EXPECT_FALSE(port_->IsOnline());

  port_get_status_called.Wait();
}

TEST_F(NetworkPortTestWithPort, MacGetProto) {
  fdf::Arena arena('WLAN');
  auto result = netdev_ifc_.PortClient().sync().buffer(arena)->GetMac();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->mac_ifc.is_valid());
}

struct NetworkPortMacTest : public NetworkPortTestWithPort {
  void SetUp() override {
    NetworkPortTestWithPort::SetUp();
    fdf::Arena arena('WLAN');
    auto result = netdev_ifc_.PortClient().sync().buffer(arena)->GetMac();
    ZX_ASSERT(result.ok());
    mac_client_.Bind(std::move(result->mac_ifc), dispatcher_);
  }

  fdf::WireSharedClient<fuchsia_hardware_network_driver::MacAddr> mac_client_;
};

TEST_F(NetworkPortMacTest, MacGetAddress) {
  constexpr fuchsia_net::wire::MacAddress kMacAddr{0x0C, 0x00, 0x0F, 0xF0, 0x0E, 0xE0};
  EXPECT_CALL(port_ifc_, MacGetAddress).WillOnce([&](fuchsia_net::MacAddress* out_mac) {
    memcpy(out_mac->octets().data(), kMacAddr.octets.data(), kMacAddr.octets.size());
  });

  fdf::Arena arena('WLAN');
  auto result = mac_client_.sync().buffer(arena)->GetAddress();
  ASSERT_OK(result.status());
  const auto& mac_addr = result->mac;
  ASSERT_EQ(mac_addr.octets.size(), kMacAddr.octets.size());
  EXPECT_EQ(memcmp(mac_addr.octets.data(), kMacAddr.octets.data(), kMacAddr.octets.size()), 0);
}

TEST_F(NetworkPortMacTest, MacGetFeatures) {
  constexpr fuchsia_hardware_network_driver::SupportedMacFilterMode kSupportedModes =
      fuchsia_hardware_network_driver::SupportedMacFilterMode::kPromiscuous |
      fuchsia_hardware_network_driver::SupportedMacFilterMode::kMulticastFilter;
  constexpr uint32_t kNumMulticastFilters = 42;
  libsync::Completion mac_get_features_called;
  EXPECT_CALL(port_ifc_, MacGetFeatures)
      .WillOnce([&](fuchsia_hardware_network_driver::Features* out_features) {
        out_features->supported_modes() = kSupportedModes;
        out_features->multicast_filter_count() = kNumMulticastFilters;
        mac_get_features_called.Signal();
      });

  fdf::Arena arena('WLAN');
  auto result = mac_client_.sync().buffer(arena)->GetFeatures();
  ASSERT_OK(result.status());
  const auto& features = result->features;
  EXPECT_EQ(features.supported_modes(), kSupportedModes);
  EXPECT_EQ(features.multicast_filter_count(), kNumMulticastFilters);
  mac_get_features_called.Wait();
}

TEST_F(NetworkPortMacTest, MacSetMode) {
  const std::array<fuchsia_net::wire::MacAddress, 2> kMulticastMacs(
      {fuchsia_net::wire::MacAddress({0x01, 0x02, 0x03, 0x04, 0x05, 0x06}),
       fuchsia_net::wire::MacAddress({0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A})});

  constexpr auto kMode = fuchsia_hardware_network::MacFilterMode::kMulticastFilter;

  libsync::Completion mac_set_mode_called;
  EXPECT_CALL(port_ifc_, MacSetMode)
      .WillOnce([&](fuchsia_hardware_network::wire::MacFilterMode mode,
                    cpp20::span<const fuchsia_net::wire::MacAddress> multicast_macs) {
        EXPECT_EQ(mode, kMode);
        ASSERT_EQ(multicast_macs.size(), std::size(kMulticastMacs));
        for (size_t i = 0; i < multicast_macs.size(); ++i) {
          EXPECT_EQ(memcmp(kMulticastMacs[i].octets.data(), multicast_macs[i].octets.data(),
                           multicast_macs[i].octets.size()),
                    0);
        }
        mac_set_mode_called.Signal();
      });

  fdf::Arena arena('WLAN');
  auto result = mac_client_.sync().buffer(arena)->SetMode(kMode, {arena, kMulticastMacs});
  ASSERT_OK(result.status());
  mac_set_mode_called.Wait();
}

}  // namespace
