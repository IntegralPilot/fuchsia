// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-bti/bti.h>

#include <algorithm>
#include <array>
#include <latch>

#include <fbl/auto_lock.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "netdevice_migration.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/testing/predicates/status.h"

namespace netdev = fuchsia_hardware_network_driver;

namespace {
constexpr size_t kMaxBufferSize = netdevice_migration::NetdeviceMigration::kMaxBufferSize;
constexpr size_t kVmoSize = 2 * kMaxBufferSize;
}  // namespace

namespace netdevice_migration {

class NetdeviceMigrationTestHelper {
 public:
  explicit NetdeviceMigrationTestHelper(NetdeviceMigration& netdev) : netdev_(netdev) {}
  // Returns true iff the driver is ready to send frames.
  bool IsTxStarted() __TA_EXCLUDES(netdev_.tx_lock_) {
    std::lock_guard<std::mutex> tx_lock(netdev_.tx_lock_);
    return netdev_.tx_started_;
  }
  // Returns true iff the driver is ready to receive frames.
  bool IsRxStarted() __TA_EXCLUDES(netdev_.rx_lock_) {
    std::lock_guard<std::mutex> rx_lock(netdev_.rx_lock_);
    return netdev_.rx_started_;
  }
  size_t netbuf_size() const { return netdev_.netbuf_size_; }
  const netdev::DeviceImplInfo& Info() { return netdev_.info_; }
  const fuchsia_hardware_network::PortBaseInfo& PortInfo() { return netdev_.port_info_; }
  const std::array<uint8_t, ETH_MAC_SIZE>& Mac() { return netdev_.mac_; }
  const zx::bti& Bti() { return netdev_.eth_bti_; }
  template <typename T, typename F>
  T WithRxSpaces(F fn) __TA_EXCLUDES(netdev_.rx_lock_) {
    std::lock_guard<std::mutex> rx_lock(netdev_.rx_lock_);
    std::queue<netdev::wire::RxSpaceBuffer>& rx_spaces = netdev_.rx_spaces_;
    fn(rx_spaces);
  }
  template <typename T, typename F>
  T WithVmoStore(F fn) __TA_EXCLUDES(netdev_.vmo_lock_) {
    fbl::AutoLock lock(&netdev_.vmo_lock_);
    NetdeviceMigrationVmoStore& vmo_store = *netdev_.vmo_store_;
    return fn(vmo_store);
  }
  template <typename T, typename F>
  T WithNetbufPool(F fn) __TA_EXCLUDES(netdev_.tx_lock_) {
    std::lock_guard<std::mutex> lock(netdev_.tx_lock_);
    NetbufPool& netbuf_pool = netdev_.netbuf_pool_;
    return fn(netbuf_pool);
  }

 private:
  NetdeviceMigration& netdev_;
};

}  // namespace netdevice_migration

namespace {

constexpr uint8_t kVmoId = 13;
constexpr uint32_t kFifoDepth = netdevice_migration::NetdeviceMigration::kFifoDepth;
// Include arbitrary bytes to exercise fuchsia.hardware.ethernet API contract.
constexpr size_t kNetbufSz = sizeof(ethernet_netbuf_t) + 40;

class MockNetworkDeviceIfc : public fdf::WireServer<netdev::NetworkDeviceIfc> {
 public:
  fdf::ClientEnd<netdev::NetworkDeviceIfc> Serve() {
    dispatcher_ = fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();

    auto [client, server] = fdf::Endpoints<netdev::NetworkDeviceIfc>::Create();
    fdf::BindServer(dispatcher_->get(), std::move(server), this);
    return std::move(client);
  }

  MOCK_METHOD(void, PortStatusChanged,
              (netdev::wire::NetworkDeviceIfcPortStatusChangedRequest * request, fdf::Arena& arena,
               PortStatusChangedCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, AddPort,
              (netdev::wire::NetworkDeviceIfcAddPortRequest * request, fdf::Arena& arena,
               AddPortCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, RemovePort,
              (netdev::wire::NetworkDeviceIfcRemovePortRequest * request, fdf::Arena& arena,
               RemovePortCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, CompleteRx,
              (netdev::wire::NetworkDeviceIfcCompleteRxRequest * request, fdf::Arena& arena,
               CompleteRxCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, CompleteTx,
              (netdev::wire::NetworkDeviceIfcCompleteTxRequest * request, fdf::Arena& arena,
               CompleteTxCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, DelegateRxLease,
              (netdev::wire::NetworkDeviceIfcDelegateRxLeaseRequest * request, fdf::Arena& arena,
               DelegateRxLeaseCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, UpdateRxBufferParams,
              (netdev::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest * request,
               fdf::Arena& arena, UpdateRxBufferParamsCompleter::Sync& completer),
              (override));
  MOCK_METHOD(void, RequestRxSpace,
              (netdev::wire::NetworkDeviceIfcRequestRxSpaceRequest * request, fdf::Arena& arena,
               RequestRxSpaceCompleter::Sync& completer),
              (override));

  void WaitForDispatcher() {
    libsync::Completion completion;
    async::PostTask(dispatcher_->async_dispatcher(), [&] { completion.Signal(); });
    completion.Wait();
  }

 private:
  fdf::UnownedSynchronizedDispatcher dispatcher_;
};

class MockEthernetImpl : public ddk::EthernetImplProtocol<MockEthernetImpl> {
 public:
  MOCK_METHOD(zx_status_t, EthernetImplQuery, (uint32_t options, ethernet_info_t* out_info));
  MOCK_METHOD(void, EthernetImplStop, ());
  MOCK_METHOD(zx_status_t, EthernetImplStart, (const ethernet_ifc_protocol_t* ifc));
  MOCK_METHOD(void, EthernetImplQueueTx,
              (uint32_t options, ethernet_netbuf_t* netbuf,
               ethernet_impl_queue_tx_callback callback, void* cookie));
  MOCK_METHOD(zx_status_t, EthernetImplSetParam,
              (uint32_t param, int32_t value, const uint8_t* data_buffer, size_t data_size));
  MOCK_METHOD(void, EthernetImplGetBti, (zx::bti * out_bti));

  compat::DeviceServer::BanjoConfig GetBanjoConfig() {
    compat::DeviceServer::BanjoConfig banjo_config;
    banjo_config.callbacks[ZX_PROTOCOL_ETHERNET_IMPL] = banjo_server_.callback();
    return banjo_config;
  }

 private:
  compat::BanjoServer banjo_server_{ZX_PROTOCOL_ETHERNET_IMPL, this, &ethernet_impl_protocol_ops_};
};

class NetdeviceMigrationTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    device_server_.Initialize(component::kDefaultInstance, {}, mock_ethernet_.GetBanjoConfig());
    if (zx_status_t status =
            device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs);
        status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok();
  }

  MockEthernetImpl& MockEthernet() { return mock_ethernet_; }

 private:
  compat::DeviceServer device_server_;
  MockEthernetImpl mock_ethernet_;
};

class TestConfig final {
 public:
  using DriverType = netdevice_migration::NetdeviceMigration;
  using EnvironmentType = NetdeviceMigrationTestEnvironment;
};

class NetdeviceMigrationTest : public ::testing::Test {
 protected:
  void TearDown() override { StopDriver(); }

  zx::result<> StartDriver() {
    zx::result<> result = driver_test_.StartDriver();
    if (result.is_ok()) {
      driver_started_ = true;
    }
    return result;
  }

  void StopDriver() {
    if (driver_started_) {
      ASSERT_OK(driver_test_.StopDriver().status_value());
      driver_started_ = false;
    }
  }

  void AssertDriverIsRunning() {
    // Provide a method with the explicit verification that the driver is running. This should
    // hopefully prevent tests from accidentally using IsDriverRunning to test that the driver was
    // removed. Doing so is not safe since it's an asynchronous process that might not be completed
    // by the time the test performs the check.
    ASSERT_TRUE(IsDriverRunning());
  }

  void WaitUntilDriverIsNotRunning() {
    for (size_t i = 0; i < 20'000; ++i) {
      if (!IsDriverRunning()) {
        return;
      }
      zx_nanosleep(ZX_MSEC(1));
    }
    FAIL() << "Driver did not stop running within the timeout";
  }

  void ConnectToNetDevice() {
    zx::result client = driver_test_.Connect<netdev::Service::NetworkDeviceImpl>();
    ASSERT_OK(client.status_value());
    netdevice_client_.Bind(std::move(client.value()));
    ASSERT_TRUE(netdevice_client_.is_valid());
  }

  void SetUpWithFeatures(uint32_t features) {
    RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
      EXPECT_CALL(mock_ethernet, EthernetImplQuery(0, testing::_))
          .WillOnce([features](uint32_t options, ethernet_info_t* out_info) -> zx_status_t {
            *out_info = {
                .features = features,
                .mtu = ETH_MTU_SIZE,
                .mac = {0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF},
                .netbuf_size = kNetbufSz,
            };
            return ZX_OK;
          });
    });
    ASSERT_OK(StartDriver().status_value());

    ConnectToNetDevice();

    EXPECT_CALL(mock_network_device_ifc_, AddPort)
        .Times(1)
        .WillOnce([this](netdev::wire::NetworkDeviceIfcAddPortRequest* request, fdf::Arena& arena,
                         MockNetworkDeviceIfc::AddPortCompleter::Sync& completer) {
          EXPECT_EQ(request->id, netdevice_migration::NetdeviceMigration::kPortId);
          port_client_.Bind(std::move(request->port));
          completer.buffer(arena).Reply(ZX_OK);
        });

    fdf::Arena arena(0u);
    fdf::WireUnownedResult result =
        netdevice_client_.buffer(arena)->Init(mock_network_device_ifc_.Serve());
    ASSERT_OK(result.status());
    ASSERT_OK(result->s);
  }

  // Perform all the steps necessary to trigger a call to EthernetImpl::Start
  void StartEthernet() {
    ASSERT_TRUE(netdevice_client_.is_valid());
    ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
    ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
    // Call QueueRxSpace to start Ethernet, the start is deferred until QueueRxSpace is called the
    // first time. No need to pass any buffers.
    ASSERT_NO_FATAL_FAILURE(QueueRxSpace(nullptr, 0));
  }

  void NetdevImplStart(zx_status_t expected) {
    fdf::Arena arena(0u);
    fdf::WireUnownedResult result = netdevice_client_.buffer(arena)->Start();

    ASSERT_OK(result.status());
    ASSERT_EQ(result->s, expected);
  }

  void NetdevImplPrepareVmo(uint8_t vmo_id) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    NetdevImplPrepareVmo(vmo_id, std::move(vmo));
  }

  void NetdevImplPrepareVmo(uint8_t vmo_id, zx::vmo vmo) {
    fdf::Arena arena(0u);
    fdf::WireUnownedResult result =
        netdevice_client_.buffer(arena)->PrepareVmo(vmo_id, std::move(vmo));
    ASSERT_OK(result.status());
    ASSERT_OK(result->s);
  }

  // Use a helper method rather than a parameterized test so that we can leverage test fixtures for
  // alternate SetUp() implementations (parameterized tests can only use one test fixture).
  void QueueTx(bool has_phys) {
    ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
    ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
    const uint8_t* vmo_start = nullptr;
    RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
      vmo_start = helper.WithVmoStore<uint8_t*>(
          [](netdevice_migration::NetdeviceMigrationVmoStore& vmo_store) {
            auto* vmo = vmo_store.GetVmo(kVmoId);
            return vmo->data().data();
          });
    });

    constexpr uint32_t kBufId = 42;
    netdev::wire::BufferRegion region = {.vmo = kVmoId, .length = ETH_MTU_SIZE};
    netdev::wire::TxBuffer buf = {
        .id = kBufId,
        .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&region, 1)};
    RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
      EXPECT_CALL(
          mock_ethernet,
          EthernetImplQueueTx(0,
                              testing::Pointee(testing::FieldsAre(
                                  testing::A<const uint8_t*>(), region.length,
                                  testing::A<zx_paddr_t>(), static_cast<short>(0u), 0)),
                              testing::An<ethernet_impl_queue_tx_callback>(), testing::A<void*>()))
          .WillOnce([has_phys, vmo_start](uint32_t options, ethernet_netbuf_t* netbuf,
                                          ethernet_impl_queue_tx_callback callback, void* cookie) {
            ASSERT_EQ(netbuf->data_buffer, vmo_start);
            ASSERT_EQ(netbuf->data_size, ETH_MTU_SIZE);
            if (has_phys) {
              ASSERT_NE(netbuf->phys, 0ul);
            } else {
              ASSERT_EQ(netbuf->phys, 0ul);
            }
            callback(cookie, ZX_OK, netbuf);
          });
    });
    libsync::Completion completed_tx;
    EXPECT_CALL(MockNetworkDevice(), CompleteTx)
        .Times(1)
        .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                      MockNetworkDeviceIfc::CompleteTxCompleter::Sync& completer) {
          EXPECT_EQ(request->tx.size(), 1u);
          EXPECT_EQ(request->tx[0].id, kBufId);
          EXPECT_EQ(request->tx[0].status, ZX_OK);
          completed_tx.Signal();
        });
    fdf::Arena arena(0u);
    EXPECT_TRUE(netdevice_client_.buffer(arena)
                    ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(&buf, 1))
                    .ok());
    completed_tx.Wait();
  }

  enum class QueueRxBehavior : uint8_t {
    kExpectEthernetStart,
    kExpectQueueSuccess,
  };

  // Helper method that will queue rx space buffers and set some expectations if requested. This
  // includes the deferred call to Ethernet start and taking the ifc out of that call to create the
  // ethernet_ifc client. It also waits until the expected number of RX spaces actually was queued
  // up in the driver. This is useful to ensure a synchronous flow of events. Note that this does
  // not work reliably when |buffers_count| is zero as there can be no measured difference after
  // the operation.
  void QueueRxSpace(netdev::wire::RxSpaceBuffer* buffers, size_t buffers_count,
                    std::initializer_list<QueueRxBehavior> expectations = {
                        QueueRxBehavior::kExpectEthernetStart,
                        QueueRxBehavior::kExpectQueueSuccess}) {
    auto has_expectation = [&](QueueRxBehavior expectation) {
      return std::ranges::find(expectations, expectation) != expectations.end();
    };

    libsync::Completion ethernet_impl_start_done;
    if (!queued_rx_space_ && has_expectation(QueueRxBehavior::kExpectEthernetStart)) {
      // The call to Ethernet.Start is deferred to the first call to QueueRxSpace to ensure there
      // are receive buffers available as soon as Start is called. After the first call it is not
      // expected to be called again.
      RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
        EXPECT_CALL(mock_ethernet, EthernetImplStart)
            .WillOnce([&](const ethernet_ifc_protocol_t* ifc) {
              ethernet_ifc_client_ = ddk::EthernetIfcProtocolClient(ifc);
              ethernet_impl_start_done.Signal();
              return ZX_OK;
            });
      });
      queued_rx_space_ = true;
    } else {
      // Not actually done but any wait on this completion should immediately succeed.
      ethernet_impl_start_done.Signal();
    }
    size_t rx_space_before = 0;
    if (has_expectation(QueueRxBehavior::kExpectQueueSuccess)) {
      RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
        helper.WithRxSpaces<void>([&](auto& rx_spaces) { rx_space_before = rx_spaces.size(); });
      });
    }
    fdf::Arena arena(0u);
    EXPECT_TRUE(netdevice_client_.buffer(arena)
                    ->QueueRxSpace(fidl::VectorView<netdev::wire::RxSpaceBuffer>::FromExternal(
                        buffers, buffers_count))
                    .ok());
    if (has_expectation(QueueRxBehavior::kExpectQueueSuccess)) {
      bool success = false;
      for (size_t i = 0; !success && i < 20'000; ++i) {
        RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
          size_t rx_space = 0;
          helper.WithRxSpaces<void>([&](auto& rx_spaces) { rx_space = rx_spaces.size(); });
          success = rx_space == rx_space_before + buffers_count;
        });
        if (!success) {
          zx_nanosleep(ZX_MSEC(1));
        }
      }
      EXPECT_TRUE(success) << "Never reached expected amount of rx space";
    }
    ethernet_impl_start_done.Wait();
  }

  void RunWithDriver(fit::callback<void(netdevice_migration::NetdeviceMigration&)> callback) {
    driver_test_.RunInDriverContext(std::move(callback));
  }
  void RunWithHelper(
      fit::callback<void(netdevice_migration::NetdeviceMigrationTestHelper&)> callback) {
    driver_test_.RunInDriverContext([&](netdevice_migration::NetdeviceMigration& driver) {
      netdevice_migration::NetdeviceMigrationTestHelper helper(driver);
      callback(helper);
    });
  }
  void RunWithMockEthernet(fit::callback<void(MockEthernetImpl&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [&](NetdeviceMigrationTestEnvironment& env) { callback(env.MockEthernet()); });
  }

  fdf::WireSyncClient<netdev::NetworkDeviceImpl>& NetDeviceClient() { return netdevice_client_; }
  fdf::WireSyncClient<netdev::NetworkPort>& PortClient() { return port_client_; }
  ddk::EthernetIfcProtocolClient& EthernetIfcClient() { return ethernet_ifc_client_; }

  fdf::WireSyncClient<netdev::MacAddr> CreateMacAddrClient() {
    fdf::Arena arena(0u);
    fdf::WireUnownedResult mac = PortClient().buffer(arena)->GetMac();
    ZX_ASSERT_MSG(mac.ok(), "Failed to get mac: %s", mac.FormatDescription().c_str());

    return fdf::WireSyncClient<netdev::MacAddr>(std::move(mac->mac_ifc));
  }

  testing::StrictMock<MockNetworkDeviceIfc>& MockNetworkDevice() {
    return mock_network_device_ifc_;
  }

 private:
  bool IsDriverRunning() {
    bool is_running = false;
    driver_test_.RunInNodeContext(
        [&](fdf_testing::TestNode& node) { is_running = node.HasNode(); });
    return is_running;
  }

  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fdf::UnownedSynchronizedDispatcher netdevice_dispatcher_ =
      driver_test_.runtime().StartBackgroundDispatcher();

  testing::StrictMock<MockNetworkDeviceIfc> mock_network_device_ifc_;
  fdf::WireSyncClient<netdev::NetworkDeviceImpl> netdevice_client_;
  fdf::WireSyncClient<netdev::NetworkPort> port_client_;
  ddk::EthernetIfcProtocolClient ethernet_ifc_client_;
  bool driver_started_ = false;
  bool queued_rx_space_ = false;
};

class NetdeviceMigrationDefaultSetupTest : public NetdeviceMigrationTest {
 protected:
  void SetUp() override {
    NetdeviceMigrationTest::SetUp();
    SetUpWithFeatures(0);
  }
};

class NetdeviceMigrationEthernetDmaSetupTest : public NetdeviceMigrationTest {
 protected:
  void SetUp() override {
    RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
      EXPECT_CALL(mock_ethernet, EthernetImplGetBti(testing::_))
          .WillOnce([](zx::bti* out_bti) -> zx_status_t {
            return fake_bti_create(out_bti->reset_and_get_address());
          });
    });
    SetUpWithFeatures(ETHERNET_FEATURE_DMA);
  }
};

class NetdeviceMigrationEthernetSetupTest : public NetdeviceMigrationDefaultSetupTest {
 protected:
  void SetUp() override {
    NetdeviceMigrationDefaultSetupTest::SetUp();
    StartEthernet();
  }
};

struct PortClassTestCase {
  std::string name;
  ethernet_feature_t features;
  fuchsia_hardware_network::PortClass expected_port_class;
};

const PortClassTestCase port_class_test_cases[]{
    {
        .name = "Ethernet",
        .features = 0,
        .expected_port_class = fuchsia_hardware_network::PortClass::kEthernet,
    },
    {
        .name = "WLAN",
        .features = ETHERNET_FEATURE_WLAN,
        .expected_port_class = fuchsia_hardware_network::PortClass::kWlanClient,
    },
    {
        .name = "WLAN_AP",
        .features = ETHERNET_FEATURE_WLAN_AP,
        .expected_port_class = fuchsia_hardware_network::PortClass::kWlanAp,
    },
    {
        .name = "WLAN_AP_and_WLAN",
        .features = ETHERNET_FEATURE_WLAN_AP | ETHERNET_FEATURE_WLAN,
        .expected_port_class = fuchsia_hardware_network::PortClass::kWlanAp,
    },
    {
        .name = "Virtual",
        .features = ETHERNET_FEATURE_SYNTH,
        .expected_port_class = fuchsia_hardware_network::PortClass::kVirtual,
    },
};

class PortClassSetupTest : public NetdeviceMigrationTest,
                           public testing::WithParamInterface<PortClassTestCase> {};

TEST_P(PortClassSetupTest, PortClassTest) {
  const PortClassTestCase test_case = GetParam();
  SetUpWithFeatures(test_case.features);
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    const fuchsia_hardware_network::PortBaseInfo port_info = helper.PortInfo();
    ASSERT_EQ(test_case.expected_port_class, port_info.port_class());
  });
}

INSTANTIATE_TEST_SUITE_P(NetdeviceMigration, PortClassSetupTest,
                         testing::ValuesIn<PortClassTestCase>(port_class_test_cases),
                         [](const testing::TestParamInfo<PortClassSetupTest::ParamType>& info) {
                           return info.param.name;
                         });

TEST_F(NetdeviceMigrationDefaultSetupTest, DeviceInfoPreconditions) {
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    const netdev::DeviceImplInfo& info = helper.Info();
    // buffer_alignment > max_buffer_length leads to either unnecessary wasting of contiguous
    // memory, or for the configuration to be rejected altogether.
    ASSERT_LE(info.buffer_alignment(), info.max_buffer_length());
  });
}

TEST_F(NetdeviceMigrationDefaultSetupTest, NetworkDeviceImplInit) {
  fdf::Arena arena(0u);
  auto [client, server] = fdf::Endpoints<netdev::NetworkDeviceIfc>::Create();
  fdf::WireUnownedResult result = NetDeviceClient().buffer(arena)->Init(std::move(client));
  ASSERT_OK(result.status());
  EXPECT_STATUS(result->s, ZX_ERR_ALREADY_BOUND);
}

TEST_F(NetdeviceMigrationDefaultSetupTest, NetworkDeviceImplStartStop) {
  constexpr struct {
    const char* name;
    bool device_started;
    // Step calls ImplStart if set, ImplStop otherwise.
    std::optional<zx_status_t> start_status;
  } kTestSteps[] = {
      {
          .name = "successful start",
          .device_started = true,
          .start_status = ZX_OK,
      },
      {
          .name = "already bound start",
          .device_started = true,
          .start_status = ZX_ERR_ALREADY_BOUND,
      },
      {
          .name = "stop",
      },
  };
  for (const auto& step : kTestSteps) {
    SCOPED_TRACE(step.name);
    if (step.start_status.has_value()) {
      ASSERT_NO_FATAL_FAILURE(NetdevImplStart(step.start_status.value()));
    } else {
      RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
        EXPECT_CALL(mock_ethernet, EthernetImplStop()).Times(1);
      });
      fdf::Arena arena(0u);
      ASSERT_OK(NetDeviceClient().buffer(arena)->Stop().status());
    }
    RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
      EXPECT_EQ(helper.IsTxStarted(), step.device_started);
      EXPECT_EQ(helper.IsRxStarted(), step.device_started);
    });
  }
}

TEST_F(NetdeviceMigrationEthernetSetupTest, EthernetIfcStatus) {
  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = PortClient().buffer(arena)->GetStatus();
  ASSERT_OK(result.status());

  EXPECT_EQ(result->status.mtu(), ETH_MTU_SIZE);
  EXPECT_EQ(result->status.flags(), fuchsia_hardware_network::wire::StatusFlags{});

  libsync::Completion port_status_changed;
  EXPECT_CALL(MockNetworkDevice(),
              PortStatusChanged(
                  testing::Pointee(testing::FieldsAre(
                      netdevice_migration::NetdeviceMigration::kPortId,
                      testing::AllOf(
                          testing::Property(&fuchsia_hardware_network::wire::PortStatus::flags,
                                            fuchsia_hardware_network::wire::StatusFlags::kOnline),
                          testing::Property(&fuchsia_hardware_network::wire::PortStatus::mtu,
                                            ETH_MTU_SIZE)))),
                  testing::_, testing::_))
      .WillOnce([&] { port_status_changed.Signal(); });

  EthernetIfcClient().Status(ETHERNET_STATUS_ONLINE);
  port_status_changed.Wait();

  result = PortClient().buffer(arena)->GetStatus();
  ASSERT_OK(result.status());
  EXPECT_EQ(result->status.mtu(), ETH_MTU_SIZE);
  EXPECT_EQ(result->status.flags(), fuchsia_hardware_network::wire::StatusFlags::kOnline);
}

TEST_F(NetdeviceMigrationDefaultSetupTest, EthernetIfcStatusCalledFromEthernetImplStart) {
  libsync::Completion eth_started;
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplStart)
        .WillOnce([&](const ethernet_ifc_protocol_t* proto) -> zx_status_t {
          auto client = ddk::EthernetIfcProtocolClient(proto);
          client.Status(ETHERNET_STATUS_ONLINE);
          eth_started.Signal();
          return ZX_OK;
        });
  });
  libsync::Completion port_status_changed;
  EXPECT_CALL(MockNetworkDevice(),
              PortStatusChanged(
                  testing::Pointee(testing::FieldsAre(
                      netdevice_migration::NetdeviceMigration::kPortId,
                      testing::AllOf(
                          testing::Property(&fuchsia_hardware_network::wire::PortStatus::flags,
                                            fuchsia_hardware_network::wire::StatusFlags::kOnline),
                          testing::Property(&fuchsia_hardware_network::wire::PortStatus::mtu,
                                            ETH_MTU_SIZE)))),
                  testing::_, testing::_))
      .WillOnce([&] { port_status_changed.Signal(); });

  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  // We don't need to provide any buffers. Just calling QueueRxSpace will trigger Ethernet start.
  // Provide an empty list of expectations so that the QueueRxSpace method doesn't overwrite our
  // expectation above.
  QueueRxSpace({}, 0, {});

  eth_started.Wait();
  port_status_changed.Wait();

  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = PortClient().buffer(arena)->GetStatus();
  ASSERT_OK(result.status());
  EXPECT_EQ(result->status.mtu(), ETH_MTU_SIZE);
  EXPECT_EQ(result->status.flags(), fuchsia_hardware_network::wire::StatusFlags::kOnline);
}

TEST_F(NetdeviceMigrationEthernetDmaSetupTest, NetworkDeviceImplPrepareReleaseVmo) {
  constexpr uint8_t kVMOs = 3;
  std::array<fake_bti_pinned_vmo_info_t, kVMOs> pinned_vmos;
  size_t pinned;
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    ASSERT_OK(fake_bti_get_pinned_vmos(helper.Bti().get(), pinned_vmos.data(), pinned_vmos.size(),
                                       &pinned));
  });
  ASSERT_EQ(pinned, 0u);

  for (uint8_t vmo_id = 1; vmo_id <= kVMOs; vmo_id++) {
    ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(vmo_id));
    RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
      ASSERT_OK(fake_bti_get_pinned_vmos(helper.Bti().get(), pinned_vmos.data(), pinned_vmos.size(),
                                         &pinned));
      ASSERT_EQ(pinned, vmo_id);
      helper.WithVmoStore<void>(
          [vmo_id](netdevice_migration::NetdeviceMigrationVmoStore& vmo_store) {
            auto* stored = vmo_store.GetVmo(vmo_id);
            ASSERT_NE(stored, nullptr);
            auto data = stored->data();
            ASSERT_EQ(data.size(), kVmoSize);
          });
    });
  }

  fdf::Arena arena(0u);
  for (uint8_t vmo_id = pinned_vmos.size(); vmo_id > 0;) {
    ASSERT_OK(NetDeviceClient().buffer(arena)->ReleaseVmo(vmo_id--).status());
    RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
      ASSERT_OK(fake_bti_get_pinned_vmos(helper.Bti().get(), pinned_vmos.data(), pinned_vmos.size(),
                                         &pinned));
    });
    ASSERT_EQ(pinned, vmo_id);
  }
}

TEST_F(NetdeviceMigrationTest, NetworkDeviceDoesNotGetBtiIfEthDoesNotSupportDma) {
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplQuery(0, testing::_))
        .WillOnce([](uint32_t options, ethernet_info_t* out_info) -> zx_status_t {
          *out_info = {
              .features = 0,
              .netbuf_size = sizeof(ethernet_netbuf_t),
          };
          return ZX_OK;
        });
    EXPECT_CALL(mock_ethernet, EthernetImplGetBti(testing::_)).Times(0);
  });
  ASSERT_OK(StartDriver().status_value());
}

TEST_F(NetdeviceMigrationTest, InvalidNetbufSzRemovesDriver) {
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplQuery(0, testing::_))
        .WillOnce([](uint32_t options, ethernet_info_t* out_info) -> zx_status_t {
          *out_info = {
              .netbuf_size = sizeof(ethernet_netbuf_t) / 2,
          };
          return ZX_OK;
        });
  });
  EXPECT_EQ(StartDriver().status_value(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(NetdeviceMigrationDefaultSetupTest, ObservesNetbufSz) {
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    helper.WithNetbufPool<void>([](netdevice_migration::NetbufPool& netbuf_pool) {
      std::optional netbuf = netbuf_pool.pop();
      ASSERT_TRUE(netbuf.has_value());
      ASSERT_GE(netbuf->size(), kNetbufSz);
    });
  });
}

TEST_F(NetdeviceMigrationDefaultSetupTest, NetworkDeviceImplQueueRxSpace) {
  // Literals have been arbitrarily selected in order to have distinct space
  // buffers to assert on, while observing preconditions on length.
  netdev::wire::RxSpaceBuffer spaces[] = {
      {
          .region =
              {
                  .offset = 42,
                  .length = ETH_MTU_SIZE,
              },
      },
      {
          .region =
              {
                  .offset = 0,
                  .length = kMaxBufferSize,
              },
      },
      {
          .region =
              {
                  .offset = 13,
                  .length = ETH_MTU_SIZE + 100,
              },
      },
  };
  // An unstarted netdevice will immediately return queued buffers.
  std::latch completed_rx(std::size(spaces));
  EXPECT_CALL(
      MockNetworkDevice(),
      CompleteRx(testing::Pointee(testing::Field(
                     &netdev::wire::NetworkDeviceIfcCompleteRxRequest::rx, testing::SizeIs(1))),
                 testing::_, testing::_))
      .Times(std::size(spaces))
      .WillRepeatedly([&] { completed_rx.count_down(); });

  // Do not expect anything from QueueRxSpace here, it shouldn't do anything other than returns the
  // buffers before Start is called.
  QueueRxSpace(spaces, std::size(spaces), {});
  completed_rx.wait();

  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    helper.WithRxSpaces<void>([](auto& rx_spaces) { EXPECT_TRUE(rx_spaces.empty()); });
  });
  QueueRxSpace(spaces, std::size(spaces));
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    helper.WithRxSpaces<void>([&spaces](auto& rx_spaces) {
      ASSERT_EQ(rx_spaces.size(), std::size(spaces));
      for (const netdev::wire::RxSpaceBuffer& space : spaces) {
        EXPECT_EQ(rx_spaces.front().region.offset, space.region.offset);
        EXPECT_EQ(rx_spaces.front().region.length, space.region.length);
        rx_spaces.pop();
      }
    });
  });
}

class QueueRxSpaceFailedPreconditionTest : public NetdeviceMigrationDefaultSetupTest,
                                           public testing::WithParamInterface<uint64_t> {};

TEST_P(QueueRxSpaceFailedPreconditionTest, RemovesDriver) {
  constexpr uint32_t kSpaceId = 13;
  netdev::wire::RxSpaceBuffer spaces[] = {
      {
          .id = kSpaceId,
          .region =
              {
                  .length = GetParam(),
              },
      },
  };
  AssertDriverIsRunning();
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  // CompleteRx will not be called so set no call expectations. Also don't set any expectations for
  // the QueueRxSpace helper method since nothing should be called when the input is not valid.
  QueueRxSpace(spaces, std::size(spaces), {});
  WaitUntilDriverIsNotRunning();
}

INSTANTIATE_TEST_SUITE_P(
    NetdeviceMigration, QueueRxSpaceFailedPreconditionTest, testing::Values(2 * kMaxBufferSize, 0),
    [](const testing::TestParamInfo<QueueRxSpaceFailedPreconditionTest::ParamType>& info) {
      if (info.param == 2 * kMaxBufferSize) {
        return std::string("TooBig");
      }
      if (info.param == 0) {
        return std::string("TooSmall");
      }
      return fxl::StringPrintf("UnknownParam_%lu", info.param);
    });

TEST_F(NetdeviceMigrationDefaultSetupTest, EthernetIfcRecv) {
  constexpr uint32_t kSpaceId = 42;
  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  netdev::wire::RxSpaceBuffer spaces[] = {
      {
          .id = kSpaceId,
          .region =
              {
                  .vmo = kVmoId,
                  .offset = 0,
                  .length = kMaxBufferSize,
              },
      },
  };
  QueueRxSpace(spaces, std::size(spaces));
  constexpr uint8_t rcvd[] = {0, 1, 2, 3, 4, 5, 6, 7};
  libsync::Completion completed_rx;
  EXPECT_CALL(MockNetworkDevice(), CompleteRx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                    MockNetworkDeviceIfc::CompleteRxCompleter::Sync& completer) {
        ASSERT_EQ(request->rx.size(), std::size(spaces));
        EXPECT_EQ(request->rx[0].meta.port, netdevice_migration::NetdeviceMigration::kPortId);
        EXPECT_EQ(request->rx[0].meta.flags,
                  static_cast<uint32_t>(fuchsia_hardware_network::wire::RxFlags{}));
        EXPECT_EQ(request->rx[0].meta.frame_type,
                  fuchsia_hardware_network::wire::FrameType::kEthernet);
        EXPECT_EQ(request->rx[0].data.size(), 1u);
        EXPECT_EQ(request->rx[0].data[0].id, kSpaceId);
        EXPECT_EQ(request->rx[0].data[0].offset, 0u);
        EXPECT_EQ(request->rx[0].data[0].length, std::size(rcvd));
        completed_rx.Signal();
      });

  EthernetIfcClient().Recv(rcvd, sizeof(rcvd), 0);
  completed_rx.Wait();

  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    helper.WithVmoStore<void>([&rcvd](auto& vmo_store) {
      auto* vmo = vmo_store.GetVmo(kVmoId);
      cpp20::span<uint8_t> data = vmo->data();
      data = data.subspan(0, std::size(rcvd));
      for (size_t i = 0; i < std::size(rcvd); ++i) {
        EXPECT_EQ(data[i], rcvd[i]);
      }
    });
  });
}

TEST_F(NetdeviceMigrationEthernetSetupTest, EthernetIfcRecvNoBuffers) {
  constexpr uint8_t bytes[] = {0, 1, 2, 3, 4, 5, 6, 7};
  // CompleteRx will not be called so do not set mock expectation.
  EthernetIfcClient().Recv(bytes, sizeof(bytes), 0);
}

struct RecvFailedPreconditionInput {
  const char* name;
  const size_t buf_len;
  const uint8_t vmo_id;
  const uint64_t offset = 0;
};

class RecvFailedPreconditionTest : public NetdeviceMigrationDefaultSetupTest,
                                   public testing::WithParamInterface<RecvFailedPreconditionInput> {
};

TEST_P(RecvFailedPreconditionTest, RemovesDriver) {
  RecvFailedPreconditionInput input = GetParam();
  constexpr uint32_t kSpaceId = 42;
  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  netdev::wire::RxSpaceBuffer spaces[] = {
      {
          .id = kSpaceId,
          .region =
              {
                  .vmo = input.vmo_id,
                  .offset = input.offset,
                  .length = kMaxBufferSize,
              },
      },
  };
  QueueRxSpace(spaces, std::size(spaces));
  uint8_t bytes[input.buf_len];
  AssertDriverIsRunning();
  // CompleteRx will not be called so do not set mock expectation.
  EthernetIfcClient().Recv(bytes, input.buf_len, 0);
  WaitUntilDriverIsNotRunning();
}

INSTANTIATE_TEST_SUITE_P(
    NetdeviceMigration, RecvFailedPreconditionTest,
    testing::Values(
        RecvFailedPreconditionInput{
            .name = "BufferTooBig",
            .buf_len = 2 * kMaxBufferSize,
            .vmo_id = kVmoId,
        },
        RecvFailedPreconditionInput{
            .name = "UnknownVmoId",
            .buf_len = ETH_FRAME_MAX_SIZE,
            .vmo_id = 24,
        },
        RecvFailedPreconditionInput{
            .name = "OffsetOutOfRange",
            .buf_len = ETH_FRAME_MAX_SIZE,
            .vmo_id = kVmoId,
            .offset = kVmoSize + 10,
        },
        RecvFailedPreconditionInput{
            .name = "LengthOutOfRange",
            .buf_len = ETH_FRAME_MAX_SIZE,
            .vmo_id = kVmoId,
            .offset = kVmoSize - 10,
        },
        RecvFailedPreconditionInput{
            .name = "IntegerOverflow",
            .buf_len = ETH_FRAME_MAX_SIZE,
            .vmo_id = kVmoId,
            .offset = UINT64_MAX - 10,
        }),
    [](const testing::TestParamInfo<RecvFailedPreconditionTest::ParamType>& info) {
      RecvFailedPreconditionInput input = info.param;
      return input.name;
    });

TEST_F(NetdeviceMigrationDefaultSetupTest, NetworkDeviceImplQueueTx) {
  ASSERT_NO_FATAL_FAILURE(QueueTx(false));
}

TEST_F(NetdeviceMigrationEthernetDmaSetupTest, NetworkDeviceImplQueueTxDma) {
  ASSERT_NO_FATAL_FAILURE(QueueTx(true));
}

struct FillTxQueueInput {
  const char* name;
  uint32_t buffer_count;
  uint32_t tx_queue_calls;
};

struct OutOfLineCallbacks {
  const char* name;
  bool enabled;
};

class FillTxQueueTest
    : public NetdeviceMigrationDefaultSetupTest,
      public testing::WithParamInterface<std::tuple<FillTxQueueInput, OutOfLineCallbacks>> {};

TEST_P(FillTxQueueTest, Succeeds) {
  FillTxQueueInput input = std::get<0>(GetParam());
  OutOfLineCallbacks ool = std::get<1>(GetParam());
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId, std::move(vmo)));
  const uint8_t* vmo_start = nullptr;
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    vmo_start = helper.WithVmoStore<uint8_t*>(
        [](netdevice_migration::NetdeviceMigrationVmoStore& vmo_store) {
          auto* vmo = vmo_store.GetVmo(kVmoId);
          return vmo->data().data();
        });
  });

  // This should be guarded by expectations_mutex. Unfortunately thread annotations don't work on
  // local variables.
  // Use a multiset to track expected TX IDs, this allows the wrap around of IDs (the multi part) as
  // well as prevents a reliance on the ordering of the CompleteTx calls (the set part, as opposed
  // to a queue for example).
  std::unordered_multiset<uint32_t> expected_tx_ids;
  std::mutex expectations_mutex;

  // Use a latch to ensure that the test only continues after a certain number of CompleteTx calls.
  std::latch all_transmissions_completed(input.tx_queue_calls * input.buffer_count);
  EXPECT_CALL(MockNetworkDevice(), CompleteTx)
      .WillRepeatedly([&](netdev::wire::NetworkDeviceIfcCompleteTxRequest* request,
                          fdf::Arena& arena,
                          MockNetworkDeviceIfc::CompleteTxCompleter::Sync& completer) {
        ASSERT_EQ(request->tx.size(), 1u);
        EXPECT_OK(request->tx[0].status);
        std::scoped_lock lock(expectations_mutex);
        auto expected_id = expected_tx_ids.find(request->tx[0].id);
        ASSERT_NE(expected_id, expected_tx_ids.end());
        expected_tx_ids.erase(expected_id);
        all_transmissions_completed.count_down();
      });

  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  for (uint32_t call = 0; call < input.tx_queue_calls; ++call) {
    netdev::wire::TxBuffer buffers[input.buffer_count];
    netdev::wire::BufferRegion region = {.vmo = kVmoId, .length = ETH_MTU_SIZE};
    for (uint32_t buf_id = 0; buf_id < input.buffer_count; ++buf_id) {
      netdev::wire::TxBuffer buf = {
          .id = ((input.buffer_count * call) + buf_id) % kFifoDepth,
          .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&region, 1)};
      buffers[buf_id] = buf;
      std::scoped_lock lock(expectations_mutex);
      expected_tx_ids.insert(buf.id);
    }
    struct CallbackRecord {
      ethernet_netbuf_t* netbuf;
      ethernet_impl_queue_tx_callback cb;
      void* cookie;
    };
    std::vector<CallbackRecord> callbacks;
    std::latch all_transmissions_queued(input.buffer_count);
    RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
      EXPECT_CALL(
          mock_ethernet,
          EthernetImplQueueTx(0,
                              testing::Pointee(testing::FieldsAre(
                                  testing::A<const uint8_t*>(), ETH_MTU_SIZE,
                                  testing::A<zx_paddr_t>(), static_cast<short>(0u), 0)),
                              testing::An<ethernet_impl_queue_tx_callback>(), testing::A<void*>()))
          .WillRepeatedly([&](uint32_t options, ethernet_netbuf_t* netbuf,
                              ethernet_impl_queue_tx_callback callback, void* cookie) {
            EXPECT_EQ(netbuf->data_buffer, vmo_start);
            EXPECT_EQ(netbuf->data_size, ETH_MTU_SIZE);
            EXPECT_EQ(netbuf->phys, 0ul);
            if (ool.enabled) {
              callbacks.push_back({.netbuf = netbuf, .cb = callback, .cookie = cookie});
            } else {
              callback(cookie, ZX_OK, netbuf);
            }
            all_transmissions_queued.count_down();
          });
    });
    fdf::Arena arena(0u);
    ASSERT_OK(NetDeviceClient()
                  .buffer(arena)
                  ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(
                      buffers, input.buffer_count))
                  .status());
    all_transmissions_queued.wait();
    if (ool.enabled) {
      for (CallbackRecord& callback : callbacks) {
        callback.cb(callback.cookie, ZX_OK, callback.netbuf);
      }
    }
  }
  all_transmissions_completed.wait();
  std::scoped_lock lock(expectations_mutex);
  EXPECT_TRUE(expected_tx_ids.empty());
}

INSTANTIATE_TEST_SUITE_P(NetdeviceMigration, FillTxQueueTest,
                         testing::Combine(testing::Values(
                                              FillTxQueueInput{
                                                  .name = "FillQueueInOneCall",
                                                  .buffer_count = kFifoDepth,
                                                  .tx_queue_calls = 1,
                                              },
                                              FillTxQueueInput{
                                                  .name = "FillQueueAcrossTwoCalls",
                                                  .buffer_count = (3 * kFifoDepth) / 4,
                                                  .tx_queue_calls = 2,
                                              }),
                                          testing::Values(
                                              OutOfLineCallbacks{
                                                  .name = "OutOfLineCallbacks",
                                                  .enabled = true,
                                              },
                                              OutOfLineCallbacks{
                                                  .name = "InLineCallbacks",
                                                  .enabled = false,
                                              })),
                         [](const testing::TestParamInfo<FillTxQueueTest::ParamType>& info) {
                           FillTxQueueInput input = std::get<0>(info.param);
                           OutOfLineCallbacks callbacks = std::get<1>(info.param);
                           return fxl::StringPrintf("%s_%s", input.name, callbacks.name);
                         });

TEST_F(NetdeviceMigrationDefaultSetupTest, NetworkDeviceImplQueueTxNotStarted) {
  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
  constexpr uint32_t kBufId = 42;
  netdev::wire::TxBuffer buf = {.id = kBufId};
  libsync::Completion completed_tx;
  EXPECT_CALL(
      MockNetworkDevice(),
      CompleteTx(
          testing::Field(&netdev::wire::NetworkDeviceIfcCompleteTxRequest::tx,
                         testing::Property(
                             &fidl::VectorView<::netdev::wire::TxResult>::get,
                             testing::AllOf(testing::SizeIs(1),
                                            testing::Property(
                                                &std::span<netdev::wire::TxResult>::front,
                                                testing::FieldsAre(kBufId, ZX_ERR_UNAVAILABLE))))),
          testing::_, testing::_))

      .Times(1)
      .WillOnce([&] { completed_tx.Signal(); });
  fdf::Arena arena(0u);
  EXPECT_TRUE(NetDeviceClient()
                  .buffer(arena)
                  ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(&buf, 1))
                  .ok());
  completed_tx.Wait();
}

struct QueueTxFailedPreconditionInput {
  const char* name;
  size_t bufs;
  size_t parts;
  size_t buf_len;
  uint8_t vmo_id;
  uint64_t offset = 0;
};

class QueueTxFailedPreconditionTest
    : public NetdeviceMigrationDefaultSetupTest,
      public testing::WithParamInterface<QueueTxFailedPreconditionInput> {};

TEST_P(QueueTxFailedPreconditionTest, RemovesDriver) {
  QueueTxFailedPreconditionInput param = GetParam();
  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  AssertDriverIsRunning();
  std::vector<netdev::wire::BufferRegion> regions;
  for (size_t i = 0; i < param.parts; ++i) {
    regions.push_back({.vmo = param.vmo_id, .offset = param.offset, .length = param.buf_len});
  }
  constexpr uint32_t kBufId = 42;
  std::vector<netdev::wire::TxBuffer> buffers;
  for (size_t i = 0; i < param.bufs; ++i) {
    buffers.push_back({
        .id = kBufId,
        .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(regions),
    });
  }
  fdf::Arena arena(0u);
  EXPECT_TRUE(NetDeviceClient()
                  .buffer(arena)
                  ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(buffers))
                  .ok());
  WaitUntilDriverIsNotRunning();
}

INSTANTIATE_TEST_SUITE_P(
    NetdeviceMigration, QueueTxFailedPreconditionTest,
    testing::Values(
        QueueTxFailedPreconditionInput{
            .name = "TooManyBuffers",
            .bufs = kFifoDepth + 1,
            .parts = 1,
            .buf_len = ETH_FRAME_MAX_SIZE,
            .vmo_id = kVmoId,
        },
        QueueTxFailedPreconditionInput{.name = "MoreThanOneBufferPart",
                                       .bufs = 1,
                                       .parts = 2,
                                       .buf_len = ETH_FRAME_MAX_SIZE,
                                       .vmo_id = kVmoId},
        QueueTxFailedPreconditionInput{.name = "BufferTooLong",
                                       .bufs = 1,
                                       .parts = 1,
                                       .buf_len = 2 * kMaxBufferSize,
                                       .vmo_id = kVmoId},
        QueueTxFailedPreconditionInput{.name = "UnknownVmoId",
                                       .bufs = 1,
                                       .parts = 1,
                                       .buf_len = ETH_FRAME_MAX_SIZE,
                                       .vmo_id = 42},
        QueueTxFailedPreconditionInput{.name = "OffsetOutOfRange",
                                       .bufs = 1,
                                       .parts = 1,
                                       .buf_len = ETH_FRAME_MAX_SIZE,
                                       .vmo_id = kVmoId,
                                       .offset = kVmoSize + 10},
        QueueTxFailedPreconditionInput{.name = "LengthOutOfRange",
                                       .bufs = 1,
                                       .parts = 1,
                                       .buf_len = ETH_FRAME_MAX_SIZE,
                                       .vmo_id = kVmoId,
                                       .offset = kVmoSize - 10},
        QueueTxFailedPreconditionInput{.name = "IntegerOverflow",
                                       .bufs = 1,
                                       .parts = 1,
                                       .buf_len = ETH_FRAME_MAX_SIZE,
                                       .vmo_id = kVmoId,
                                       .offset = UINT64_MAX - 10}),
    [](const testing::TestParamInfo<QueueTxFailedPreconditionTest::ParamType>& info) {
      QueueTxFailedPreconditionInput input = info.param;
      return input.name;
    });

TEST_F(NetdeviceMigrationDefaultSetupTest, MacAddrGetAddress) {
  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = CreateMacAddrClient().buffer(arena)->GetAddress();
  ASSERT_OK(result.status());
  uint8_t expected[] = {0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF};
  for (size_t i = 0; i < ETH_MAC_SIZE; ++i) {
    EXPECT_EQ(result->mac.octets[i], expected[i]);
  }
}

TEST_F(NetdeviceMigrationDefaultSetupTest, MacAddrGetFeatures) {
  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = CreateMacAddrClient().buffer(arena)->GetFeatures();
  ASSERT_OK(result.status());
  EXPECT_EQ(result->features.multicast_filter_count(),
            netdevice_migration::NetdeviceMigration::kMulticastFilterMax);
  EXPECT_EQ(result->features.supported_modes(),
            netdevice_migration::NetdeviceMigration::kSupportedMacFilteringModes);
}

TEST_F(NetdeviceMigrationDefaultSetupTest, TooManyMulticastMacFilters) {
  fdf::Arena arena(0u);
  fidl::VectorView<fuchsia_net::wire::MacAddress> mcast_macs(
      arena, netdevice_migration::NetdeviceMigration::kMulticastFilterMax + 1);
  // This should fail at the FIDL level, it should reject a vector that's too big.
  ASSERT_FALSE(
      CreateMacAddrClient()
          .buffer(arena)
          ->SetMode(fuchsia_hardware_network::wire::MacFilterMode::kMulticastFilter, mcast_macs)
          .ok());
}

TEST_F(NetdeviceMigrationDefaultSetupTest, InvalidMacMode) {
  fdf::Arena arena(0u);
  fidl::VectorView<fuchsia_net::wire::MacAddress> mcast_macs;
  fuchsia_hardware_network::wire::MacFilterMode invalid_mode =
      static_cast<fuchsia_hardware_network::wire::MacFilterMode>(
          static_cast<uint32_t>(fuchsia_hardware_network::wire::MacFilterMode::kMulticastFilter) |
          static_cast<uint32_t>(
              fuchsia_hardware_network::wire::MacFilterMode::kMulticastPromiscuous) |
          static_cast<uint32_t>(fuchsia_hardware_network::wire::MacFilterMode::kPromiscuous));

  AssertDriverIsRunning();
  ASSERT_OK(CreateMacAddrClient().buffer(arena)->SetMode(invalid_mode, mcast_macs).status());
  WaitUntilDriverIsNotRunning();
}

TEST_F(NetdeviceMigrationDefaultSetupTest, MacAddrSetMode) {
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet,
                EthernetImplSetParam(ETHERNET_SETPARAM_MULTICAST_PROMISC, 0, nullptr, 0))
        .WillOnce(
            [](uint32_t p, int32_t v, const uint8_t* data, size_t data_len) { return ZX_OK; });
    EXPECT_CALL(mock_ethernet, EthernetImplSetParam(ETHERNET_SETPARAM_PROMISC, 0, nullptr, 0))
        .WillOnce(
            [](uint32_t p, int32_t v, const uint8_t* data, size_t data_len) { return ZX_OK; });
  });
  fdf::Arena arena(0u);
  fidl::VectorView<fuchsia_net::wire::MacAddress> mac_filter(
      arena, netdevice_migration::NetdeviceMigration::kMulticastFilterMax);
  for (size_t i = 0; i < mac_filter.size(); ++i) {
    // Fill up each mac address with {i, i + 1, i + 2, ...} to have some distinct test data.
    std::iota(std::begin(mac_filter[i].octets), std::end(mac_filter[i].octets), i);
  }
  auto mcast_macs_match = [&](const uint8_t* data) -> bool {
    const uint8_t* addr = data;
    for (auto& mac : mac_filter) {
      if (std::memcmp(mac.octets.data(), addr, ETH_MAC_SIZE) != 0) {
        return false;
      }
      addr += ETH_MAC_SIZE;
    }
    return true;
  };
  using fuchsia_hardware_network::wire::MacFilterMode;
  fdf::WireSyncClient<netdev::MacAddr> mac_addr_client = CreateMacAddrClient();
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet,
                EthernetImplSetParam(ETHERNET_SETPARAM_MULTICAST_FILTER,
                                     netdevice_migration::NetdeviceMigration::kMulticastFilterMax,
                                     testing::ResultOf(mcast_macs_match, true),
                                     mac_filter.size() * ETH_MAC_SIZE))
        .WillOnce(
            [](uint32_t p, int32_t v, const uint8_t* data, size_t data_len) { return ZX_OK; });
  });
  ASSERT_OK(
      mac_addr_client.buffer(arena)->SetMode(MacFilterMode::kMulticastFilter, mac_filter).status());

  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplSetParam(ETHERNET_SETPARAM_PROMISC, 0, nullptr, 0))
        .WillOnce(
            [](uint32_t p, int32_t v, const uint8_t* data, size_t data_len) { return ZX_OK; });
    EXPECT_CALL(mock_ethernet,
                EthernetImplSetParam(ETHERNET_SETPARAM_MULTICAST_PROMISC, 1, nullptr, 0))
        .WillOnce(
            [](uint32_t p, int32_t v, const uint8_t* data, size_t data_len) { return ZX_OK; });
  });
  ASSERT_OK(
      mac_addr_client.buffer(arena)->SetMode(MacFilterMode::kMulticastPromiscuous, {}).status());

  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplSetParam(ETHERNET_SETPARAM_PROMISC, 1, nullptr, 0))
        .WillOnce(
            [](uint32_t p, int32_t v, const uint8_t* data, size_t data_len) { return ZX_OK; });
  });
  ASSERT_OK(mac_addr_client.buffer(arena)->SetMode(MacFilterMode::kPromiscuous, {}).status());
}

TEST_F(NetdeviceMigrationDefaultSetupTest, GetMac) {
  fdf::Arena arena(0u);
  fdf::WireUnownedResult result = PortClient().buffer(arena)->GetMac();
  ASSERT_OK(result.status());
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    fdf::WireUnownedResult mac = fdf::WireCall(result->mac_ifc).buffer(arena)->GetAddress();
    ASSERT_OK(mac.status());
    for (size_t i = 0; i < ETH_MAC_SIZE; ++i) {
      EXPECT_EQ(mac->mac.octets[i], helper.Mac()[i]);
    }
  });
}

TEST_F(NetdeviceMigrationDefaultSetupTest, ReturnsRxBuffersOnStop) {
  constexpr uint32_t kBufId = 27;
  netdev::wire::RxSpaceBuffer spaces[] = {
      {
          .id = kBufId,
          .region =
              {
                  .offset = 42,
                  .length = ETH_MTU_SIZE,
              },
      },
  };
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  RunWithHelper([&](netdevice_migration::NetdeviceMigrationTestHelper& helper) {
    helper.WithRxSpaces<void>([](auto& rx_spaces) { EXPECT_TRUE(rx_spaces.empty()); });
  });
  QueueRxSpace(spaces, std::size(spaces));
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplStop()).Times(1);
  });
  libsync::Completion completed_rx;
  EXPECT_CALL(MockNetworkDevice(), CompleteRx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                    MockNetworkDeviceIfc::CompleteRxCompleter::Sync& completer) {
        ASSERT_EQ(request->rx.size(), 1u);
        const netdev::wire::RxBuffer& buffer = request->rx[0];
        ASSERT_EQ(buffer.data.size(), 1u);
        const netdev::wire::RxBufferPart& part = buffer.data[0];
        EXPECT_EQ(part.id, kBufId);
        completed_rx.Signal();
      });
  fdf::Arena arena(0u);
  ASSERT_OK(NetDeviceClient().buffer(arena)->Stop().status());
  completed_rx.Wait();
}

TEST_F(NetdeviceMigrationDefaultSetupTest, ReturnsTxBuffersOnStop) {
  ASSERT_NO_FATAL_FAILURE(NetdevImplPrepareVmo(kVmoId));
  ASSERT_NO_FATAL_FAILURE(NetdevImplStart(ZX_OK));
  constexpr uint32_t kBufId = 42;
  netdev::wire::BufferRegion region = {.vmo = kVmoId, .length = ETH_MTU_SIZE};
  netdev::wire::TxBuffer buf = {
      .id = kBufId,
      .data = fidl::VectorView<netdev::wire::BufferRegion>::FromExternal(&region, 1),
  };

  fit::callback<void()> complete_tx_callback;
  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplQueueTx)
        .WillOnce([&complete_tx_callback](uint32_t options, ethernet_netbuf_t* netbuf,
                                          ethernet_impl_queue_tx_callback callback, void* cookie) {
          complete_tx_callback = [callback, cookie, netbuf]() {
            callback(cookie, ZX_ERR_CANCELED, netbuf);
          };
        });
  });
  fdf::Arena arena(0u);
  ASSERT_OK(NetDeviceClient()
                .buffer(arena)
                ->QueueTx(fidl::VectorView<netdev::wire::TxBuffer>::FromExternal(&buf, 1))
                .status());

  RunWithMockEthernet([&](MockEthernetImpl& mock_ethernet) {
    EXPECT_CALL(mock_ethernet, EthernetImplStop()).Times(1);
  });
  libsync::Completion completed_tx;
  EXPECT_CALL(MockNetworkDevice(), CompleteTx)
      .WillOnce([&](netdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                    MockNetworkDeviceIfc::CompleteTxCompleter::Sync& completer) {
        ASSERT_EQ(request->tx.size(), 1u);
        const netdev::wire::TxResult& result = request->tx[0];
        EXPECT_EQ(result.id, kBufId);
        EXPECT_STATUS(result.status, ZX_ERR_CANCELED);
        completed_tx.Signal();
      });
  ASSERT_OK(NetDeviceClient().buffer(arena)->Stop().status());
  if (complete_tx_callback) {
    complete_tx_callback();
  }
  completed_tx.Wait();
}

}  // namespace
