// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/ethernet/drivers/virtio/netdevice.h"

#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/fake-bti/bti.h>
#include <lib/virtio/backends/fake.h>

#include <atomic>
#include <queue>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/connectivity/ethernet/drivers/virtio/virtio_net_driver.h"
#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/testing/predicates/status.h"

namespace virtio {

class FakeBackendForNetdeviceTest : public FakeBackend {
 public:
  using Base = FakeBackend;
  static constexpr uint8_t kMac[] = {0x02, 0x03, 0x04, 0x05, 0x06, 0x07};

  FakeBackendForNetdeviceTest()
      : FakeBackend({{NetworkDevice::kRxId, NetworkDevice::kMaxDepth},
                     {NetworkDevice::kTxId, NetworkDevice::kMaxDepth}}) {
    // Provide a reasonable default behavior for tests that don't care to mock this call.
    ON_CALL(*this, RingKick).WillByDefault([this](uint16_t index) {
      // Call base class implementation to ensure proper bookkeeping of kicks.
      FakeBackend::RingKick(index);
    });
    for (size_t i = 0; i < sizeof(virtio_net_config_t); i++) {
      AddClassRegister(static_cast<uint16_t>(i), static_cast<uint8_t>(0));
    }
    SetLinkUp();
  }

  void Terminate() override { sync_completion_signal(&completion_); }
  // We'll trigger interrupts manually during testing, keep the interrupt thread
  // locked until termination.
  zx::result<uint32_t> WaitForInterrupt() override {
    sync_completion_wait(&completion_, ZX_TIME_INFINITE);
    return zx::ok(0);
  }

  void SetLinkUp() { UpdateStatus(true); }
  void SetLinkDown() { UpdateStatus(false); }

  void UpdateStatus(bool link_up) {
    virtio_net_config_t config = {};
    if (link_up) {
      config.status = VIRTIO_NET_S_LINK_UP;
    };
    static_assert(sizeof(kMac) == sizeof(config.mac));
    std::copy(std::begin(kMac), std::end(kMac), config.mac);
    for (size_t i = 0; i < sizeof(config); ++i) {
      SetClassRegister(static_cast<uint16_t>(i), reinterpret_cast<uint8_t*>(&config)[i]);
    }
  }

  bool IsQueueKicked(uint16_t queue_index) { return QueueKicked(queue_index); }

  void DeviceReset() override {
    FakeBackend::DeviceReset();
    rx_ring_started_ = false;
    tx_ring_started_ = false;
  }

  void SetFeatures(uint64_t bitmap) override { feature_bits_ |= bitmap; }

  uint64_t ReadFeatures() override {
    uint64_t bitmap = FakeBackend::ReadFeatures();

    if (support_feature_v1_) {
      bitmap |= VIRTIO_F_VERSION_1;
    }

    // Declare support for VIRTIO_NET_F_MAC. It is required by the driver implementation.
    bitmap |= VIRTIO_NET_F_MAC;

    if (with_status_feature_) {
      // Declare support for VIRTIO_NET_F_STATUS. If not supported, the spec assumes that the link
      // is always active. Enable this so we can test link status changes.
      bitmap |= VIRTIO_NET_F_STATUS;
    }

    return bitmap;
  }

  zx_status_t SetRing(uint16_t index, uint16_t count, zx_paddr_t pa_desc, zx_paddr_t pa_avail,
                      zx_paddr_t pa_used) override {
    switch (index) {
      case NetworkDevice::kRxId:
        EXPECT_FALSE(rx_ring_started_);
        rx_ring_started_ = true;
        break;
      case NetworkDevice::kTxId:
        EXPECT_FALSE(tx_ring_started_);
        tx_ring_started_ = true;
        break;
      default:
        ADD_FAILURE() << "unexpected ring index " << index;
        return ZX_ERR_INTERNAL;
    }
    EXPECT_EQ(count, NetworkDevice::kMaxDepth);
    return ZX_OK;
  }

  MOCK_METHOD(void, RingKick, (uint16_t ring_index), (override));

  bool rx_ring_started() const { return rx_ring_started_; }
  bool tx_ring_started() const { return tx_ring_started_; }
  uint64_t feature_bits() const { return feature_bits_; }
  void SetSupportFeatureV1(bool v1) { support_feature_v1_ = v1; }
  void SetWithStatusFeature(bool with_status_feature) {
    with_status_feature_ = with_status_feature;
  }

 private:
  sync_completion_t completion_;
  bool rx_ring_started_ = false;
  bool tx_ring_started_ = false;
  bool support_feature_v1_ = false;
  bool with_status_feature_ = true;
  uint64_t feature_bits_ = 0;
};

// The test driver exists to override the creation of the NetworkDevice object. It needs to be
// created with the fake backend, something that can't be injected since it's instantiated with a
// specific type in the driver class. Since the driver class is extremely simplistic, it really only
// exists to create the NetworkDevice (which does all the work), we assume that covering only
// NetworkDevice gives us enough confidence.
class TestVirtioNetDriver : public VirtioNetDriver {
 public:
  TestVirtioNetDriver() = default;

  // Allow tests to configure the backend before the driver starts. If these values are set they
  // will be applied to the backend at creation.
  static void SetWithStatusFeature(bool supported) { with_status_features_ = supported; }
  static void SetSupportFeatureV1(bool v1) { support_feature_v1_ = v1; }

  FakeBackendForNetdeviceTest* backend() { return backend_; }

 private:
  zx::result<std::unique_ptr<NetworkDevice>> CreateNetworkDevice(
      const std::shared_ptr<fdf::Namespace>& incoming,
      const std::optional<std::string>& node_name) override {
    auto backend = std::make_unique<FakeBackendForNetdeviceTest>();
    if (with_status_features_.has_value()) {
      backend->SetWithStatusFeature(with_status_features_.value());
    }
    if (support_feature_v1_.has_value()) {
      backend->SetSupportFeatureV1(support_feature_v1_.value());
    }
    backend_ = backend.get();
    zx::bti bti(ZX_HANDLE_INVALID);
    if (zx_status_t status = fake_bti_create(bti.reset_and_get_address()); status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::make_unique<NetworkDevice>(this, std::move(bti), std::move(backend),
                                                  incoming, node_name));
  }

  FakeBackendForNetdeviceTest* backend_ = nullptr;

  static std::optional<bool> with_status_features_;
  static std::optional<bool> support_feature_v1_;
};
std::optional<bool> TestVirtioNetDriver::with_status_features_;
std::optional<bool> TestVirtioNetDriver::support_feature_v1_;

class TestConfig final {
 public:
  using DriverType = TestVirtioNetDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class NetworkDeviceTests : public testing::Test, public fdf::WireServer<netdev::NetworkDeviceIfc> {
 public:
  static constexpr uint8_t kVmoId = 1;
  static constexpr netdev::wire::BufferMetadata kFrameMetadata = {
      .port = NetworkDevice::kPortId,
      .frame_type = fuchsia_hardware_network::wire::FrameType::kEthernet,
  };
  static constexpr size_t kVmoFrameCount = 256;

  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver().status_value());

    ConnectToNetDevice();

    fdf::Arena arena(0u);
    fdf::WireUnownedResult result = netdevice_client_.buffer(arena)->Init(ServeNetDevIfc());
    ASSERT_OK(result.status());
    ASSERT_OK(result->s);
    ASSERT_TRUE(port_.is_valid());

    fdf::WireUnownedResult mac = port_.buffer(arena)->GetMac();
    ZX_ASSERT_MSG(mac.ok(), "Failed to get mac: %s", mac.FormatDescription().c_str());
    mac_.Bind(std::move(mac->mac_ifc));
    ASSERT_TRUE(mac_.is_valid());
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver().status_value()); }

  void ConnectToNetDevice() {
    zx::result client = driver_test_.Connect<netdev::Service::NetworkDeviceImpl>();
    ASSERT_OK(client.status_value());
    netdevice_client_.Bind(std::move(client.value()));
    ASSERT_TRUE(netdevice_client_.is_valid());
  }

  fdf::ClientEnd<netdev::NetworkDeviceIfc> ServeNetDevIfc() {
    auto [client, server] = fdf::Endpoints<netdev::NetworkDeviceIfc>::Create();
    fdf::BindServer(netdevice_dispatcher_->get(), std::move(server), this);
    return std::move(client);
  }

  void PrepareVmo() {
    ASSERT_FALSE(vmo_.is_valid());
    ASSERT_OK(zx::vmo::create(NetworkDevice::kFrameSize * kVmoFrameCount, 0, &vmo_));
    zx::vmo device_vmo;
    ASSERT_OK(vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &device_vmo));
    fdf::Arena arena(0u);
    fdf::WireUnownedResult result =
        netdevice_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(device_vmo));
    ASSERT_OK(result.status());
    ASSERT_OK(result->s);
  }

  void StartDevice() {
    fdf::Arena arena(0u);
    fdf::WireUnownedResult result = netdevice_client_.buffer(arena)->Start();
    ASSERT_OK(result.status());
    ASSERT_OK(result->s);
  }

  // NetworkDevice interface implementation.
  MOCK_METHOD(void, PortStatusChanged,
              (netdev::wire::NetworkDeviceIfcPortStatusChangedRequest * request, fdf::Arena& arena,
               PortStatusChangedCompleter::Sync& completer),
              (override));
  void AddPort(netdev::wire::NetworkDeviceIfcAddPortRequest* request, fdf::Arena& arena,
               AddPortCompleter::Sync& completer) override {
    EXPECT_EQ(request->id, NetworkDevice::kPortId);
    EXPECT_FALSE(port_.is_valid());
    port_.Bind(std::move(request->port));
    completer.buffer(arena).Reply(ZX_OK);
  }
  void RemovePort(netdev::wire::NetworkDeviceIfcRemovePortRequest* request, fdf::Arena& arena,
                  RemovePortCompleter::Sync& completer) override {
    ADD_FAILURE() << "Port should never be removed";
  }
  MOCK_METHOD(void, CompleteRx,
              (netdev::wire::NetworkDeviceIfcCompleteRxRequest * request, fdf::Arena& arena,
               CompleteRxCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, CompleteTx,
              (netdev::wire::NetworkDeviceIfcCompleteTxRequest * request, fdf::Arena& arena,
               CompleteTxCompleter::Sync& completer),
              (override));
  void DelegateRxLease(netdev::wire::NetworkDeviceIfcDelegateRxLeaseRequest* request,
                       fdf::Arena& arena, DelegateRxLeaseCompleter::Sync& completer) override {
    ADD_FAILURE() << "DelegateRxLease should never be called";
  }
  void UpdateRxBufferParams(netdev::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest* request,
                            fdf::Arena& arena,
                            UpdateRxBufferParamsCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(fit::ok());
  }
  MOCK_METHOD(void, RequestRxSpace,
              (netdev::wire::NetworkDeviceIfcRequestRxSpaceRequest * request, fdf::Arena& arena,
               RequestRxSpaceCompleter::Sync& completer),
              (override));

  void WithDevice(fit::callback<void(NetworkDevice&)> callback) {
    driver_test_.RunInDriverContext([&](VirtioNetDriver& driver) {
      ASSERT_NE(driver.GetNetworkDevice(), nullptr);
      callback(*driver.GetNetworkDevice());
    });
  }
  fdf::WireSyncClient<netdev::NetworkDeviceImpl>& netdev() { return netdevice_client_; }
  fdf::WireSyncClient<netdev::NetworkPort>& port() { return port_; }
  fdf::WireSyncClient<netdev::MacAddr>& mac() { return mac_; }
  FakeBackendForNetdeviceTest& backend() {
    FakeBackendForNetdeviceTest* ptr = nullptr;
    driver_test_.RunInDriverContext([&](TestVirtioNetDriver& driver) { ptr = driver.backend(); });
    return *ptr;
  }
  zx::vmo& vmo() { return vmo_; }
  void WithTxRing(fit::callback<void(NetworkDevice&, vring&)> callback) {
    WithDevice([&](NetworkDevice& device) {
      std::scoped_lock lock(device.tx_lock_);
      callback(device, device.tx_.vring_unsafe());
    });
  }
  void WithRxRing(fit::callback<void(NetworkDevice&, vring&)> callback) {
    WithDevice([&](NetworkDevice& device) {
      std::scoped_lock lock(device.rx_lock_);
      callback(device, device.rx_.vring_unsafe());
    });
  }
  bool IsRxSpaceRequested(NetworkDevice& device) {
    std::scoped_lock lock(device.rx_lock_);
    return device.rx_space_requested_;
  }

 private:
  template <typename T>
  static std::optional<T> PopHelper(std::queue<T>& queue) {
    if (queue.empty()) {
      return std::nullopt;
    }
    T ret = queue.front();
    queue.pop();
    return ret;
  }

  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fdf::UnownedSynchronizedDispatcher netdevice_dispatcher_ =
      driver_test_.runtime().StartBackgroundDispatcher();

  zx::vmo vmo_;
  fdf::WireSyncClient<netdev::NetworkPort> port_;
  fdf::WireSyncClient<netdev::MacAddr> mac_;

  fdf::WireSyncClient<netdev::NetworkDeviceImpl> netdevice_client_;
};

class VirtioVersionTests : public NetworkDeviceTests, public testing::WithParamInterface<bool> {
 public:
  void SetUp() override {
    TestVirtioNetDriver::SetSupportFeatureV1(IsV1Virtio());
    NetworkDeviceTests::SetUp();
  }

  bool IsV1Virtio() { return GetParam(); }
};

TEST_F(NetworkDeviceTests, PortGetStatus) {
  fdf::Arena arena(0u);

  fdf::WireUnownedResult result = port().buffer(arena)->GetStatus();
  ASSERT_OK(result.status());
  fuchsia_hardware_network::wire::PortStatus status = result->status;
  EXPECT_EQ(status.mtu(), NetworkDevice::kMtu);
  EXPECT_EQ(status.flags(), fuchsia_hardware_network::wire::StatusFlags::kOnline);

  backend().SetLinkDown();

  result = port().buffer(arena)->GetStatus();
  ASSERT_OK(result.status());
  status = result->status;
  EXPECT_EQ(status.mtu(), NetworkDevice::kMtu);
  EXPECT_EQ(status.flags(), fuchsia_hardware_network::wire::StatusFlags());
}

TEST_F(NetworkDeviceTests, StartReportsOnlineOnLinkUp) {
  // Link is up by default.
  libsync::Completion port_status_changed;
  EXPECT_CALL(*this, PortStatusChanged)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                    fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) {
        EXPECT_EQ(request->id, NetworkDevice::kPortId);
        EXPECT_EQ(request->new_status.flags(),
                  fuchsia_hardware_network::wire::StatusFlags::kOnline);
        port_status_changed.Signal();
      });

  ASSERT_NO_FATAL_FAILURE(StartDevice());
  port_status_changed.Wait();
}

TEST_F(NetworkDeviceTests, StartReportsOfflineOnLinkDown) {
  libsync::Completion port_status_changed;
  EXPECT_CALL(*this, PortStatusChanged)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                    fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) {
        EXPECT_EQ(request->id, NetworkDevice::kPortId);
        EXPECT_EQ(request->new_status.flags(), fuchsia_hardware_network::wire::StatusFlags{});
        port_status_changed.Signal();
      });

  backend().SetLinkDown();
  ASSERT_NO_FATAL_FAILURE(StartDevice());
  port_status_changed.Wait();
}

class StatusNotSupportedTests : public NetworkDeviceTests {
 public:
  void SetUp() override {
    TestVirtioNetDriver::SetWithStatusFeature(false);
    NetworkDeviceTests::SetUp();
  }
};

TEST_F(StatusNotSupportedTests, StartReportsOnlineWhenStatusNotSupported) {
  libsync::Completion port_status_changed;
  EXPECT_CALL(*this, PortStatusChanged)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                    fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) {
        EXPECT_EQ(request->id, NetworkDevice::kPortId);
        EXPECT_EQ(request->new_status.flags(),
                  fuchsia_hardware_network::wire::StatusFlags::kOnline);
        port_status_changed.Signal();
      });

  backend().SetLinkDown();
  ASSERT_NO_FATAL_FAILURE(StartDevice());
  port_status_changed.Wait();

  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = netdev().buffer(arena)->Stop();
  ASSERT_OK(result.status());

  // When the status feature is not supported, the device should NOT report an
  // offline status.
  EXPECT_CALL(*this, PortStatusChanged).Times(0);
}

TEST_F(NetworkDeviceTests, MacGetAddr) {
  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = mac().buffer(arena)->GetAddress();
  ASSERT_OK(result.status());

  EXPECT_THAT(result->mac.octets, testing::ElementsAreArray(FakeBackendForNetdeviceTest::kMac));
}

TEST_P(VirtioVersionTests, Start) {
  EXPECT_FALSE(backend().rx_ring_started());
  EXPECT_FALSE(backend().tx_ring_started());
  EXPECT_EQ(backend().DeviceState(), FakeBackend::State::DEVICE_STATUS_ACK);

  ASSERT_NO_FATAL_FAILURE(StartDevice());

  EXPECT_TRUE(backend().rx_ring_started());
  EXPECT_TRUE(backend().tx_ring_started());
  ASSERT_EQ(backend().DeviceState(), FakeBackend::State::DRIVER_OK);
  if (IsV1Virtio()) {
    EXPECT_TRUE(backend().feature_bits() & VIRTIO_F_VERSION_1);
  } else {
    EXPECT_FALSE(backend().feature_bits() & VIRTIO_F_VERSION_1);
  }
}

TEST_F(NetworkDeviceTests, Stop) {
  libsync::Completion port_status_changed;
  EXPECT_CALL(*this, PortStatusChanged)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                    fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) {
        EXPECT_EQ(request->id, NetworkDevice::kPortId);
        EXPECT_EQ(request->new_status.flags(),
                  fuchsia_hardware_network::wire::StatusFlags::kOnline);
        port_status_changed.Signal();
      });

  ASSERT_NO_FATAL_FAILURE(StartDevice());
  // After starting the device, a status update is sent.
  port_status_changed.Wait();

  ASSERT_NO_FATAL_FAILURE(PrepareVmo());

  netdev::wire::BufferRegion region = {
      .vmo = kVmoId,
      .offset = 0,
      .length = NetworkDevice::kFrameSize,
  };
  netdev::wire::RxSpaceBuffer rx_spaces[] = {
      {.id = 1, .region = region},
      {.id = 2, .region = region},
  };
  const uint16_t header_len = [&] {
    uint16_t len = 0;
    WithDevice([&](NetworkDevice& device) { len = device.virtio_header_len(); });
    return len;
  }();
  netdev::wire::TxBuffer tx_buffers[] = {
      {
          .id = 1,
          .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&region, 1),
          .meta = kFrameMetadata,
          .head_length = header_len,
      },
      {
          .id = 2,
          .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&region, 1),
          .meta = kFrameMetadata,
          .head_length = header_len,
      },
  };

  fdf::Arena arena(0u);
  // Queue some rx and tx buffers so we observe them being returned on stop.
  ASSERT_OK(
      netdev()
          .buffer(arena)
          ->QueueRxSpace(fidl::VectorView<netdev::wire::RxSpaceBuffer>::FromExternal(rx_spaces))
          .status());
  ASSERT_OK(netdev()
                .buffer(arena)
                ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(tx_buffers))
                .status());

  port_status_changed.Reset();
  EXPECT_CALL(*this, PortStatusChanged)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                    fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) {
        EXPECT_EQ(request->id, NetworkDevice::kPortId);
        EXPECT_EQ(request->new_status.flags(), fuchsia_hardware_network::wire::StatusFlags{});
        port_status_changed.Signal();
      });

  libsync::Completion completed_rx;
  EXPECT_CALL(*this, CompleteRx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                    CompleteRxCompleter::Sync& completer) {
        EXPECT_EQ(request->rx.size(), std::size(rx_spaces));
        for (size_t i = 0; i < std::size(rx_spaces); ++i) {
          SCOPED_TRACE(fxl::StringPrintf("rx space %d", rx_spaces[i].id));
          EXPECT_EQ(request->rx[i].data.size(), 1u);
          EXPECT_EQ(request->rx[i].data[0].id, rx_spaces[i].id);
          EXPECT_EQ(request->rx[i].data[0].offset, 0u);
          EXPECT_EQ(request->rx[i].data[0].length, 0u);
        }
        completed_rx.Signal();
      });

  libsync::Completion completed_tx;
  EXPECT_CALL(*this, CompleteTx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                    CompleteTxCompleter::Sync& completer) {
        EXPECT_EQ(request->tx.size(), std::size(tx_buffers));
        for (size_t i = 0; i < std::size(tx_buffers); ++i) {
          EXPECT_STATUS(request->tx[i].status, ZX_ERR_BAD_STATE);
          // Buffers are completed in reverse order.
          EXPECT_EQ(request->tx[i].id, tx_buffers[std::size(tx_buffers) - 1 - i].id);
        }
        completed_tx.Signal();
      });
  ASSERT_OK(netdev().buffer(arena)->Stop().status());

  // After stopping, the device should report offline status.
  port_status_changed.Wait();
  completed_rx.Wait();
  completed_tx.Wait();

  EXPECT_EQ(backend().DeviceState(), FakeBackend::State::DEVICE_RESET);
}

TEST_F(NetworkDeviceTests, StopDuringIrqUpdate) {
  ASSERT_NO_FATAL_FAILURE(StartDevice());
  ASSERT_NO_FATAL_FAILURE(PrepareVmo());

  std::array<netdev::wire::TxBuffer, NetworkDevice::kMaxDepth> tx_buffers;
  std::array<netdev::wire::RxSpaceBuffer, NetworkDevice::kMaxDepth> rx_spaces;

  netdev::wire::BufferRegion placeholder_region = {
      .vmo = kVmoId,
      .offset = 0,
      .length = NetworkDevice::kFrameSize,
  };

  WithDevice([&](NetworkDevice& device) {
    for (uint32_t i = 0; i < tx_buffers.size(); ++i) {
      tx_buffers[i] = {.id = i,
                       .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(
                           &placeholder_region, 1),
                       .meta = kFrameMetadata,
                       .head_length = device.virtio_header_len()};
    }
  });

  for (uint32_t i = 0; i < rx_spaces.size(); ++i) {
    rx_spaces[i] = {.id = i, .region = placeholder_region};
  }

  fdf::Arena arena(0u);

  // Queue some rx and tx buffers so we observe them being returned on stop.
  ASSERT_OK(
      netdev()
          .buffer(arena)
          ->QueueRxSpace(fidl::VectorView<netdev::wire::RxSpaceBuffer>::FromExternal(rx_spaces))
          .status());
  ASSERT_OK(netdev()
                .buffer(arena)
                ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(tx_buffers))
                .status());
  // Populate TX descriptors in the ring to indicate that everything was transmitted so that
  // IrqRingUpdate has something to do.
  WithTxRing([&](NetworkDevice&, vring& tx_ring) {
    for (const auto& tx_buffer : tx_buffers) {
      tx_ring.used[tx_ring.used->idx++] = {.idx = static_cast<uint16_t>(tx_buffer.id)};
    }
  });

  // Also Populate RX descriptors in the ring to indicate that everything was received.
  WithRxRing([&](NetworkDevice&, vring& rx_ring) {
    for (const auto& rx_space : rx_spaces) {
      rx_ring.used[rx_ring.used->idx++] = {.idx = static_cast<uint16_t>(rx_space.id)};
    }
  });

  // Now call Stop first to return all buffers.
  ASSERT_OK(netdev().buffer(arena)->Stop().status());

  // Then behave as if an IRQ ring update was pending but blocked on one of the locks in Stop and is
  // now allowed to continue running. If the TX or RX ring is not correctly cleared in Stop this
  // will trigger an assertion and crash.
  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });
}

TEST_F(NetworkDeviceTests, UpdateStatus) {
  const struct {
    const char* name;
    fit::function<void()> set_state;
    fuchsia_hardware_network::wire::StatusFlags expect;
  } kTests[] = {
      {
          .name = "link down",
          .set_state = fit::bind_member(&backend(), &FakeBackendForNetdeviceTest::SetLinkDown),
          .expect = fuchsia_hardware_network::wire::StatusFlags(),
      },
      {
          .name = "link up",
          .set_state = fit::bind_member(&backend(), &FakeBackendForNetdeviceTest::SetLinkUp),
          .expect = fuchsia_hardware_network::wire::StatusFlags::kOnline,
      },
  };
  for (const auto& test : kTests) {
    SCOPED_TRACE(test.name);
    test.set_state();
    libsync::Completion port_status_changed;
    EXPECT_CALL(*this, PortStatusChanged)
        .WillOnce([&](netdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                      fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) {
          EXPECT_EQ(request->id, NetworkDevice::kPortId);
          EXPECT_EQ(request->new_status.mtu(), NetworkDevice::kMtu);
          EXPECT_EQ(request->new_status.flags(), test.expect);
          port_status_changed.Signal();
        });

    WithDevice([&](NetworkDevice& device) { device.IrqConfigChange(); });
    port_status_changed.Wait();
  }
}

TEST_P(VirtioVersionTests, Rx) {
  ASSERT_NO_FATAL_FAILURE(StartDevice());
  ASSERT_NO_FATAL_FAILURE(PrepareVmo());
  netdev::wire::RxSpaceBuffer rx_space[] = {
      {
          .id = 1,
          .region =
              {
                  .vmo = kVmoId,
                  .offset = 0,
                  .length = NetworkDevice::kFrameSize,
              },
      },
      {
          .id = 2,
          .region =
              {
                  .vmo = kVmoId,
                  .offset = NetworkDevice::kFrameSize,
                  .length = NetworkDevice::kFrameSize,
              },
      },
  };

  // Each call to QueueRxSpace should kick the RX ring once.
  libsync::Completion ring_kicked;
  EXPECT_CALL(backend(), RingKick(NetworkDevice::kRxId)).WillOnce([&](uint16_t index) {
    // Call base class implementation to ensure proper bookkeeping of kicks.
    backend().FakeBackend::RingKick(index);
    ring_kicked.Signal();
  });

  fdf::Arena arena(0u);
  ASSERT_OK(
      netdev()
          .buffer(arena)
          ->QueueRxSpace(fidl::VectorView<netdev::wire::RxSpaceBuffer>::FromExternal(rx_space))
          .status());

  ring_kicked.Wait();
  EXPECT_TRUE(backend().IsQueueKicked(NetworkDevice::kRxId));

  constexpr uint32_t kReceivedLenMultiplier = 10;

  // Check build descriptors and write into registers as the device does.
  WithRxRing([&](NetworkDevice& device, vring& vring) {
    size_t avail_ring_offset = std::size(rx_space);
    for (const auto& space : rx_space) {
      uint16_t desc_idx = vring.avail->ring[vring.avail->idx - avail_ring_offset--];
      vring_desc& desc = vring.desc[desc_idx];
      EXPECT_EQ(desc.flags, VRING_DESC_F_WRITE);
      EXPECT_EQ(desc.len, space.region.length);
      EXPECT_EQ(desc.addr, FAKE_BTI_PHYS_ADDR + space.region.offset);
      EXPECT_EQ(desc.next, 0);

      vring.used->ring[vring.used->idx++] = {
          .id = desc_idx,
          .len = device.virtio_header_len() + space.id * kReceivedLenMultiplier,
      };
    }
  });

  // It's not safe to use WithDevice inside the NetworkDeviceIfc mock call. It will most likely be
  // inlined from the IrqRingUpdate call below which already happens in WithDevice. Recursively
  // calling it will cause a deadlock. Capture the virtio header length here instead.
  uint16_t virtio_header_len = 0;
  WithDevice([&](NetworkDevice& device) { virtio_header_len = device.virtio_header_len(); });

  libsync::Completion completed_rx;
  EXPECT_CALL(*this, CompleteRx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                    CompleteRxCompleter::Sync& completer) {
        EXPECT_EQ(request->rx.size(), std::size(rx_space));
        for (size_t i = 0; i < std::size(rx_space); ++i) {
          SCOPED_TRACE(fxl::StringPrintf("rx space %d", rx_space[i].id));
          EXPECT_EQ(request->rx[i].data.size(), 1u);
          EXPECT_EQ(request->rx[i].data[0].id, rx_space[i].id);
          EXPECT_EQ(request->rx[i].data[0].offset, virtio_header_len);
          EXPECT_EQ(request->rx[i].data[0].length, rx_space[i].id * kReceivedLenMultiplier);
          EXPECT_EQ(request->rx[i].meta.frame_type,
                    fuchsia_hardware_network::wire::FrameType::kEthernet);
          EXPECT_EQ(request->rx[i].meta.flags, 0u);
          EXPECT_EQ(request->rx[i].meta.port, NetworkDevice::kPortId);
        }
        completed_rx.Signal();
      });

  // Call irq handler and verify all buffer are returned.
  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });
  completed_rx.Wait();
}

TEST_P(VirtioVersionTests, Tx) {
  ASSERT_NO_FATAL_FAILURE(StartDevice());
  ASSERT_NO_FATAL_FAILURE(PrepareVmo());
  const uint16_t header_len = [&] {
    uint16_t len = 0;
    WithDevice([&](NetworkDevice& device) { len = device.virtio_header_len(); });
    return len;
  }();
  netdev::wire::BufferRegion buffer_regions[] = {
      {
          .vmo = kVmoId,
          .offset = 0,
          .length = static_cast<uint64_t>(header_len + 25),
      },
      {
          .vmo = kVmoId,
          .offset = NetworkDevice::kFrameSize,
          .length = static_cast<uint64_t>(header_len + 88),
      },
  };
  netdev::wire::TxBuffer tx_buffers[] = {
      {
          .id = 1,
          .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&buffer_regions[0], 1),
          .meta = kFrameMetadata,
          .head_length = header_len,
      },
      {
          .id = 2,
          .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&buffer_regions[1], 1),
          .meta = kFrameMetadata,
          .head_length = header_len,
      },
  };
  constexpr uint8_t kInitValue = 0xAA;
  {
    std::array<uint8_t, sizeof(virtio_net_hdr_t) + 1> header;
    header.fill(kInitValue);
    // Write garbage to the VMO where virtio headers are inserted.
    for (const auto& region : buffer_regions) {
      ASSERT_OK(vmo().write(header.data(), region.offset, header.size()));
    }
  }

  // Each call to QueueTx should kick the TX ring once.
  libsync::Completion ring_kicked;
  EXPECT_CALL(backend(), RingKick(NetworkDevice::kTxId)).WillOnce([&](uint16_t index) {
    // Call base class implementation to ensure proper bookkeeping of kicks.
    backend().FakeBackend::RingKick(index);
    ring_kicked.Signal();
  });

  fdf::Arena arena(0u);
  ASSERT_OK(netdev()
                .buffer(arena)
                ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(tx_buffers))
                .status());
  ring_kicked.Wait();
  EXPECT_TRUE(backend().IsQueueKicked(NetworkDevice::kTxId));

  for (const auto& region : buffer_regions) {
    SCOPED_TRACE(fxl::StringPrintf("region at %zu", region.offset));
    virtio_net_hdr_t header;
    WithDevice([&](NetworkDevice& device) {
      ASSERT_OK(vmo().read(reinterpret_cast<uint8_t*>(&header), region.offset,
                           device.virtio_header_len()));
    });
    EXPECT_EQ(header.base.flags, 0);
    EXPECT_EQ(header.base.gso_type, 0);
    EXPECT_EQ(header.base.hdr_len, 0);
    EXPECT_EQ(header.base.gso_size, 0);
    EXPECT_EQ(header.base.csum_start, 0);
    EXPECT_EQ(header.base.csum_offset, 0);

    if (IsV1Virtio()) {
      EXPECT_EQ(header.num_buffers, 0);
    } else {
      // Num buffers is not present if the V1 feature flag is not set, this
      // should be considered part of the payload.
      union {
        uint16_t value;
        std::array<uint8_t, sizeof(uint16_t)> bytes;
      } v;
      v.bytes.fill(kInitValue);
      EXPECT_EQ(header.num_buffers, v.value);
    }

    // The byte immediately after the header is payload, and thus should not
    // have been touched.
    uint8_t next;
    WithDevice([&](NetworkDevice& device) {
      ASSERT_OK(vmo().read(&next, region.offset + device.virtio_header_len(), 1));
    });
    EXPECT_EQ(next, kInitValue);
  }

  // Check build descriptors and write into registers as the device does.
  size_t avail_offset = 0;
  WithTxRing([&](NetworkDevice&, vring& vring) {
    avail_offset = vring.avail->idx - std::size(tx_buffers);
    for (const auto& tx_buffer : tx_buffers) {
      uint16_t desc_idx = vring.avail->ring[avail_offset++];
      vring_desc& desc = vring.desc[desc_idx];
      ASSERT_EQ(tx_buffer.data.size(), 1u);
      const netdev::wire::BufferRegion& region = tx_buffer.data[0];
      EXPECT_EQ(desc.flags, 0);
      EXPECT_EQ(desc.len, region.length);
      EXPECT_EQ(desc.addr, FAKE_BTI_PHYS_ADDR + region.offset);
      EXPECT_EQ(desc.next, 0);

      vring.used->ring[vring.used->idx++] = {
          .id = desc_idx,
      };
    }
  });

  libsync::Completion completed_tx;
  EXPECT_CALL(*this, CompleteTx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                    CompleteTxCompleter::Sync& completer) {
        EXPECT_EQ(request->tx.size(), std::size(tx_buffers));
        for (size_t i = 0; i < std::size(tx_buffers); ++i) {
          EXPECT_OK(request->tx[i].status);
          EXPECT_EQ(request->tx[i].id, tx_buffers[i].id);
        }
        completed_tx.Signal();
      });

  // Call irq handler and verify all buffers are returned.
  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });
  completed_tx.Wait();

  ring_kicked.Reset();
  EXPECT_CALL(backend(), RingKick(NetworkDevice::kTxId)).WillOnce([&](uint16_t index) {
    // Call base class implementation to ensure proper bookkeeping of kicks.
    backend().FakeBackend::RingKick(index);
    ring_kicked.Signal();
  });

  // Submit the buffers again, but this time report them used in the opposite order.
  ASSERT_OK(netdev()
                .buffer(arena)
                ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(tx_buffers))
                .status());

  ring_kicked.Wait();

  WithTxRing([&](NetworkDevice&, vring& vring) {
    vring.used->idx += std::size(tx_buffers);
    uint16_t idx = vring.used->idx;
    for (const auto& tx_buffer : tx_buffers) {
      uint16_t desc_idx = vring.avail->ring[avail_offset++];
      vring_desc& desc = vring.desc[desc_idx];
      ASSERT_EQ(tx_buffer.data.size(), 1u);
      const netdev::wire::BufferRegion& region = tx_buffer.data[0];
      EXPECT_EQ(desc.flags, 0);
      EXPECT_EQ(desc.len, region.length);
      EXPECT_EQ(desc.addr, FAKE_BTI_PHYS_ADDR + region.offset);
      EXPECT_EQ(desc.next, 0);

      vring.used->ring[--idx] = {
          .id = desc_idx,
      };
    }
  });

  completed_tx.Reset();
  EXPECT_CALL(*this, CompleteTx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                    CompleteTxCompleter::Sync& completer) {
        EXPECT_EQ(request->tx.size(), std::size(tx_buffers));
        for (size_t i = 0; i < std::size(tx_buffers); ++i) {
          EXPECT_OK(request->tx[i].status);
          // Expect that buffers are returned in the opposite order to match behavior above.
          EXPECT_EQ(request->tx[i].id, tx_buffers[std::size(tx_buffers) - 1 - i].id);
        }
        completed_tx.Signal();
      });

  // Call irq handler and verify all buffers are returned.
  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });
  completed_tx.Wait();
}

TEST_F(NetworkDeviceTests, RequestRxSpaceDebounced) {
  ASSERT_NO_FATAL_FAILURE(StartDevice());
  ASSERT_NO_FATAL_FAILURE(PrepareVmo());

  netdev::wire::BufferRegion region = {
      .vmo = kVmoId,
      .offset = 0,
      .length = NetworkDevice::kFrameSize,
  };
  netdev::wire::RxSpaceBuffer rx_space = {
      .id = 1,
      .region = region,
  };

  fdf::Arena arena(0u);
  // Queue 1 Rx buffer.
  ASSERT_OK(
      netdev()
          .buffer(arena)
          ->QueueRxSpace(fidl::VectorView<netdev::wire::RxSpaceBuffer>::FromExternal(&rx_space, 1))
          .status());

  WithDevice([&](NetworkDevice& device) { EXPECT_FALSE(IsRxSpaceRequested(device)); });

  // Mark the Rx buffer as used so it is completed during IrqRingUpdate, leaving rx_in_flight_
  // empty.
  WithRxRing([&](NetworkDevice& device, vring& rx_ring) {
    uint16_t desc_idx = rx_ring.avail->ring[rx_ring.avail->idx - 1];
    rx_ring.used->ring[rx_ring.used->idx++] = {
        .id = desc_idx,
        .len = static_cast<uint32_t>(device.virtio_header_len() + 10),
    };
  });

  libsync::Completion rx_space_requested;
  EXPECT_CALL(*this, RequestRxSpace)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcRequestRxSpaceRequest* request, fdf::Arena& arena,
                    RequestRxSpaceCompleter::Sync& completer) {
        EXPECT_EQ(request->requested, NetworkDevice::kMaxDepth);
        rx_space_requested.Signal();
      });
  EXPECT_CALL(*this, CompleteRx).Times(testing::AnyNumber());

  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });

  rx_space_requested.Wait();
  WithDevice([&](NetworkDevice& device) { EXPECT_TRUE(IsRxSpaceRequested(device)); });

  // Subsequent IrqRingUpdate calls while rx_in_flight_ is still empty will not call RequestRxSpace
  // again.
  EXPECT_CALL(*this, RequestRxSpace).Times(0);
  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });

  // Queueing new Rx buffers resets the debouncing flag.
  ASSERT_OK(
      netdev()
          .buffer(arena)
          ->QueueRxSpace(fidl::VectorView<netdev::wire::RxSpaceBuffer>::FromExternal(&rx_space, 1))
          .status());
  WithDevice([&](NetworkDevice& device) { EXPECT_FALSE(IsRxSpaceRequested(device)); });

  // Now consume the newly queued Rx buffer. It should trigger RequestRxSpace again.
  WithRxRing([&](NetworkDevice& device, vring& rx_ring) {
    uint16_t desc_idx = rx_ring.avail->ring[rx_ring.avail->idx - 1];
    rx_ring.used->ring[rx_ring.used->idx++] = {
        .id = desc_idx,
        .len = static_cast<uint32_t>(device.virtio_header_len() + 10),
    };
  });

  rx_space_requested.Reset();
  EXPECT_CALL(*this, RequestRxSpace)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcRequestRxSpaceRequest* request, fdf::Arena& arena,
                    RequestRxSpaceCompleter::Sync& completer) {
        EXPECT_EQ(request->requested, NetworkDevice::kMaxDepth);
        rx_space_requested.Signal();
      });

  WithDevice([&](NetworkDevice& device) { device.IrqRingUpdate(); });
  rx_space_requested.Wait();
  WithDevice([&](NetworkDevice& device) { EXPECT_TRUE(IsRxSpaceRequested(device)); });

  // Stop resets rx_space_requested_.
  ASSERT_OK(netdev().buffer(arena)->Stop().status());
  WithDevice([&](NetworkDevice& device) { EXPECT_FALSE(IsRxSpaceRequested(device)); });
}

INSTANTIATE_TEST_SUITE_P(NetworkDeviceTests, VirtioVersionTests, testing::Values(true, false),
                         [](const testing::TestParamInfo<VirtioVersionTests::ParamType>& info) {
                           if (info.param) {
                             return "V1Feature";
                           }
                           return "NoV1Feature";
                         });

}  // namespace virtio

FUCHSIA_DRIVER_EXPORT2(virtio::TestVirtioNetDriver);
