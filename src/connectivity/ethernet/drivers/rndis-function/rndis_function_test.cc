// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rndis_function.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire_test_base.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/driver.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <usb-inspect/usb-inspect-test-helper.h>

#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"
#include "src/lib/testing/predicates/status.h"

namespace ffunction = fuchsia_hardware_usb_function;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

namespace rndis_function {
namespace {

class FakeEndpoint : public fake_usb_endpoint::FakeEndpoint {
 public:
  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override {
    fake_usb_endpoint::FakeEndpoint::QueueRequests(request, completer);
    if (on_queue_requests_) {
      on_queue_requests_(*this);
    }
  }

  void set_on_queue_requests(fit::function<void(FakeEndpoint&)> callback) {
    on_queue_requests_ = std::move(callback);
  }

 private:
  fit::function<void(FakeEndpoint&)> on_queue_requests_;
};

class FakeFunction
    : public fake_usb_endpoint::FakeUsbFidlProvider<ffunction::UsbFunction, FakeEndpoint> {
 public:
  using Base = fake_usb_endpoint::FakeUsbFidlProvider<ffunction::UsbFunction, FakeEndpoint>;

  FakeFunction(async_dispatcher_t* dispatcher) : Base(dispatcher), dispatcher_(dispatcher) {}

  void Configure(fidl::Request<ffunction::UsbFunction::Configure>& request,
                 fidl::internal::NaturalCompleter<ffunction::UsbFunction::Configure>::Sync&
                     completer) override {
    iface_ = std::move(request.iface());
    if (on_configure_) {
      on_configure_();
    }
    completer.Reply(fit::ok());
  }

  void AllocResources(
      fidl::Request<ffunction::UsbFunction::AllocResources>& request,
      fidl::internal::NaturalCompleter<ffunction::UsbFunction::AllocResources>::Sync& completer)
      override {
    uint8_t ep_addr = 1;
    for (auto& ep : request.endpoints()) {
      if (ep.endpoint().is_valid()) {
        fake_endpoint(ep_addr).Connect(dispatcher_, std::move(ep.endpoint()));
        ep_addr++;
      }
    }
    ffunction::UsbFunctionAllocResourcesResponse response;
    response.interface_nums() = std::vector<uint8_t>{0, 1};
    response.endpoint_addrs() = std::vector<uint8_t>{1, 2, 3};
    response.string_indices() = std::vector<uint8_t>{1, 2, 3};
    completer.Reply(fit::ok(std::move(response)));
  }

  void ConfigureEndpoint(
      fidl::Request<ffunction::UsbFunction::ConfigureEndpoint>& request,
      fidl::internal::NaturalCompleter<ffunction::UsbFunction::ConfigureEndpoint>::Sync& completer)
      override {
    config_ep_calls_++;
    completer.Reply(fit::ok());
  }

  void DisableEndpoint(
      fidl::Request<ffunction::UsbFunction::DisableEndpoint>& request,
      fidl::internal::NaturalCompleter<ffunction::UsbFunction::DisableEndpoint>::Sync& completer)
      override {
    disable_ep_calls_++;
    completer.Reply(fit::ok());
  }

  size_t ConfigEpCalls() const { return config_ep_calls_; }
  size_t DisableEpCalls() const { return disable_ep_calls_; }

  fidl::ClientEnd<ffunction::UsbFunctionInterface> TakeInterface() { return std::move(iface_); }

  void set_on_configure(fit::callback<void()> callback) { on_configure_ = std::move(callback); }

 private:
  fidl::ClientEnd<ffunction::UsbFunctionInterface> iface_;
  fit::callback<void()> on_configure_;
  size_t config_ep_calls_ = 0;
  size_t disable_ep_calls_ = 0;
  async_dispatcher_t* dispatcher_;
};

class FakeNetworkDeviceIfc : public fidl::testing::WireTestBase<fnetdev::NetworkDeviceIfc> {
 public:
  FakeNetworkDeviceIfc() = default;

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "FakeNetworkDeviceIfc not implemented:" << name.c_str();
    if (completer.is_reply_needed()) {
      completer.Close(ZX_ERR_NOT_SUPPORTED);
    }
  }

  void AddPort(fnetdev::wire::NetworkDeviceIfcAddPortRequest* request, fdf::Arena& arena,
               AddPortCompleter::Sync& completer) override {
    fdf::info("FakeNetworkDeviceIfc::AddPort called with id: {}", request->id);
    port_id_ = request->id;
    port_ = std::move(request->port);
    completer.buffer(arena).Reply(ZX_OK);
    if (on_add_port_) {
      on_add_port_();
    }
  }

  void CompleteRx(fnetdev::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena& arena,
                  CompleteRxCompleter::Sync& completer) override {
    for (auto& result : request->rx) {
      completed_rx_.push(fidl::ToNatural(result));
    }
    if (on_complete_rx_) {
      on_complete_rx_();
    }
  }

  void CompleteTx(fnetdev::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena& arena,
                  CompleteTxCompleter::Sync& completer) override {
    for (const auto& result : request->tx) {
      completed_tx_.push(result);
    }
    if (on_complete_tx_) {
      on_complete_tx_();
    }
  }

  void PortStatusChanged(fnetdev::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
                         fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) override {
    ASSERT_FALSE(completer.is_reply_needed());
    EXPECT_EQ(request->id, RndisFunction::kPortId);
    last_online_ =
        (request->new_status.flags() & fuchsia_hardware_network::wire::StatusFlags::kOnline) !=
        fuchsia_hardware_network::wire::StatusFlags{};
    if (on_port_status_changed_) {
      on_port_status_changed_();
    }
  }

  bool HasPort() { return port_.is_valid(); }

  std::optional<bool> last_online() { return last_online_; }

  fdf::ClientEnd<fnetdev::NetworkPort> TakePort() { return std::move(port_); }

  void set_on_add_port(fit::function<void()> callback) { on_add_port_ = std::move(callback); }
  void set_on_complete_tx(fit::function<void()> callback) { on_complete_tx_ = std::move(callback); }
  void set_on_complete_rx(fit::function<void()> callback) { on_complete_rx_ = std::move(callback); }
  void set_on_port_status_changed(fit::function<void()> callback) {
    on_port_status_changed_ = std::move(callback);
  }

  std::optional<fnetdev::wire::TxResult> PopCompleteTx() {
    if (completed_tx_.empty()) {
      return std::nullopt;
    }
    auto tx = completed_tx_.front();
    completed_tx_.pop();
    return tx;
  }
  std::optional<fnetdev::RxBuffer> PopCompleteRx() {
    if (completed_rx_.empty()) {
      return std::nullopt;
    }
    auto rx = completed_rx_.front();
    completed_rx_.pop();
    return rx;
  }

 private:
  uint8_t port_id_;
  fdf::ClientEnd<fnetdev::NetworkPort> port_;
  fit::function<void()> on_add_port_;
  fit::function<void()> on_complete_tx_;
  fit::function<void()> on_complete_rx_;
  fit::function<void()> on_port_status_changed_;
  std::queue<fnetdev::wire::TxResult> completed_tx_;
  std::queue<fnetdev::RxBuffer> completed_rx_;
  std::optional<bool> last_online_;
};

class RndisFunctionTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    device_server_.Initialize("default");
    zx_status_t status = device_server_.Serve(dispatcher, &to_driver_vfs);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    if (mac_address_.has_value()) {
      if (zx::result result =
              mac_address_metadata_server_.Serve(to_driver_vfs, dispatcher, mac_address_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    ffunction::UsbFunctionService::InstanceHandler handler({
        .device = usb_function_bindings_.CreateHandler(&fake_function_, dispatcher,
                                                       fidl::kIgnoreBindingClosure),
    });

    if (zx::result result =
            to_driver_vfs.AddService<ffunction::UsbFunctionService>(std::move(handler));
        result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  compat::DeviceServer device_server_;
  FakeFunction fake_function_{fdf::Dispatcher::GetCurrent()->async_dispatcher()};

  fidl::ServerBindingGroup<ffunction::UsbFunction> usb_function_bindings_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>
      mac_address_metadata_server_;
  FakeNetworkDeviceIfc fake_ifc_;
  std::optional<fuchsia_boot_metadata::MacAddressMetadata> mac_address_;
};

class FixtureConfig final {
 public:
  using DriverType = RndisFunction;
  using EnvironmentType = RndisFunctionTestEnvironment;
};

class RndisFunctionTest : public ::testing::Test {
 public:
  static constexpr fdf_arena_tag_t kArenaTag = 'TEST';
  static constexpr std::array<uint8_t, 6> kMacAddr = {0x01, 0x23, 0x34, 0x56, 0x67, 0x89};

  void SetUp() override {
    zx::result endpoints = fdf::CreateEndpoints<fnetdev::NetworkDeviceIfc>();
    ASSERT_OK(endpoints);
    libsync::Completion port_ready;
    libsync::Completion configure_done;
    driver_test_.RunInEnvironmentTypeContext(
        [server = std::move(endpoints->server), &port_ready,
         &configure_done](RndisFunctionTestEnvironment& env) mutable {
          env.mac_address_ =
              fuchsia_boot_metadata::MacAddressMetadata{{.mac_address = {{{.octets = kMacAddr}}}}};
          fdf::BindServer(fdf::Dispatcher::GetCurrent()->get(), std::move(server), &env.fake_ifc_);
          env.fake_ifc_.set_on_add_port([&port_ready]() { port_ready.Signal(); });
          env.fake_function_.set_on_configure([&configure_done]() { configure_done.Signal(); });
        });

    ASSERT_OK(driver_test_.StartDriver().status_value());

    zx::result connect_result = driver_test_.Connect<fnetdev::Service::NetworkDeviceImpl>();
    ASSERT_OK(connect_result);

    fdf::Arena arena(kArenaTag);
    net_impl_client_.Bind(std::move(connect_result.value()));
    fdf::WireUnownedResult init_result =
        net_impl_client_.buffer(arena)->Init(std::move(endpoints->client));
    ASSERT_OK(init_result.status());
    ASSERT_OK(init_result->s);

    port_ready.Wait();
    configure_done.Wait();

    driver_test_.RunInEnvironmentTypeContext([this](RndisFunctionTestEnvironment& env) {
      ASSERT_TRUE(env.fake_ifc_.HasPort());
      net_port_client_.Bind(env.fake_ifc_.TakePort());
      function_interface_client_.Bind(env.fake_function_.TakeInterface());
    });
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  void StartNetworkDevice() {
    fdf::Arena arena(kArenaTag);
    fdf::WireUnownedResult start_result = net_impl_client_.buffer(arena)->Start();
    ASSERT_OK(start_result.status());
    ASSERT_OK(start_result->s);
  }

  void WriteCommand(const void* data, size_t length) {
    const uint8_t* data_u8 = reinterpret_cast<const uint8_t*>(data);
    fidl::Result result = function_interface_client_->Control({{
        .setup = fdescriptor::UsbSetup{{
            .bm_request_type = USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
            .b_request = USB_CDC_SEND_ENCAPSULATED_COMMAND,
            .w_value = 0,
            .w_index = 0,
            .w_length = 0,
        }},
        .write = std::vector<uint8_t>(data_u8, data_u8 + length),
    }});
    ASSERT_OK(result);
    ASSERT_EQ(result->read().size(), 0ul);
  }

  void ReadResponse(void* data, size_t length) {
    fidl::Result result = function_interface_client_->Control({{
        .setup = fdescriptor::UsbSetup{{
            .bm_request_type = USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
            .b_request = USB_CDC_GET_ENCAPSULATED_RESPONSE,
            .w_value = 0,
            .w_index = 0,
            .w_length = static_cast<uint16_t>(length),
        }},
        .write = {},
    }});
    ASSERT_OK(result);
    ASSERT_LE(result->read().size(), length);
    memcpy(data, result->read().data(), result->read().size());
  }

  void QueryOid(uint32_t oid, void* data, size_t length, size_t* actual) {
    rndis_query query{
        .msg_type = RNDIS_QUERY_MSG,
        .msg_length = static_cast<uint32_t>(sizeof(rndis_query)),
        .request_id = 42,
        .oid = oid,
        .info_buffer_length = 0,
        .info_buffer_offset = 0,
        .reserved = 0,
    };
    WriteCommand(&query, sizeof(query));

    std::vector<uint8_t> buffer(sizeof(rndis_query_complete) + length);
    ReadResponse(buffer.data(), buffer.size());

    auto response = reinterpret_cast<rndis_query_complete*>(buffer.data());
    ASSERT_EQ(response->msg_type, RNDIS_QUERY_CMPLT);
    ASSERT_GE(response->msg_length, sizeof(rndis_query_complete));
    ASSERT_GE(response->request_id, 42u);
    ASSERT_EQ(response->status, static_cast<uint32_t>(RNDIS_STATUS_SUCCESS));

    size_t offset = response->info_buffer_offset + offsetof(rndis_query_complete, request_id);
    ASSERT_GE(offset, sizeof(rndis_query_complete));
    ASSERT_LE(offset + response->info_buffer_length, buffer.size());

    memcpy(data, buffer.data() + offset, response->info_buffer_length);
    *actual = response->info_buffer_length;
  }

  void SetPacketFilter() {
    struct Payload {
      rndis_set header;
      uint8_t data[RNDIS_SET_INFO_BUFFER_LENGTH];
    } __PACKED;
    Payload set = {};

    uint32_t filter = 0;
    set.header.msg_type = RNDIS_SET_MSG;
    set.header.msg_length = static_cast<uint32_t>(sizeof(rndis_set) + sizeof(filter));
    set.header.request_id = 42;
    set.header.oid = OID_GEN_CURRENT_PACKET_FILTER;
    set.header.info_buffer_length = static_cast<uint32_t>(sizeof(filter));
    set.header.info_buffer_offset = sizeof(rndis_set) - offsetof(rndis_set, request_id);
    memcpy(&set.data, &filter, sizeof(filter));
    WriteCommand(&set, sizeof(set));

    rndis_indicate_status status;
    ReadResponse(&status, sizeof(status));
    EXPECT_EQ(status.msg_type, static_cast<uint32_t>(RNDIS_INDICATE_STATUS_MSG));
    EXPECT_EQ(status.msg_length, sizeof(rndis_indicate_status));
    EXPECT_EQ(status.status, static_cast<uint32_t>(RNDIS_STATUS_MEDIA_CONNECT));

    rndis_set_complete response;
    ReadResponse(&response, sizeof(response));
    ASSERT_EQ(response.msg_type, static_cast<uint32_t>(RNDIS_SET_CMPLT));
    ASSERT_GE(response.msg_length, sizeof(rndis_set_complete));
    ASSERT_GE(response.request_id, 42u);
    ASSERT_EQ(response.status, static_cast<uint32_t>(RNDIS_STATUS_SUCCESS));
  }

  void ReadIndicateStatus(uint32_t expected_status) {
    rndis_indicate_status status;
    ReadResponse(&status, sizeof(status));
    ASSERT_EQ(status.msg_type, static_cast<uint32_t>(RNDIS_INDICATE_STATUS_MSG));
    ASSERT_EQ(status.msg_length, sizeof(rndis_indicate_status));
    ASSERT_EQ(status.status, expected_status);
  }

  void RunInFunctionContext(fit::callback<void(FakeFunction&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.fake_function_); });
  }

 protected:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fdf::WireSyncClient<fnetdev::NetworkDeviceImpl> net_impl_client_;
  fdf::WireSyncClient<fnetdev::NetworkPort> net_port_client_;
  fidl::SyncClient<ffunction::UsbFunctionInterface> function_interface_client_;
};

TEST_F(RndisFunctionTest, Configure) {
  RunInFunctionContext([](auto& function) {
    EXPECT_EQ(function.ConfigEpCalls(), 0u);
    EXPECT_EQ(function.DisableEpCalls(), 0u);
  });
  fidl::Result result = function_interface_client_->SetConfigured({{
      .configured = true,
      .speed = fdescriptor::UsbSpeed::kFull,
  }});
  ASSERT_OK(result);

  RunInFunctionContext([](auto& function) {
    EXPECT_EQ(function.ConfigEpCalls(), 3u);
    EXPECT_EQ(function.DisableEpCalls(), 0u);
  });

  result = function_interface_client_->SetConfigured({{
      .configured = false,
      .speed = fdescriptor::UsbSpeed::kFull,
  }});
  ASSERT_OK(result);

  RunInFunctionContext([](auto& function) {
    EXPECT_EQ(function.ConfigEpCalls(), 3u);
    EXPECT_EQ(function.DisableEpCalls(), 3u);
  });
}

TEST_F(RndisFunctionTest, NetworkDeviceGetInfo) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult result = net_impl_client_.buffer(arena)->GetInfo();
  ASSERT_TRUE(result.ok()) << result.error().FormatDescription();

  auto& info = result.value().info;
  EXPECT_EQ(info.tx_depth(), RndisFunction::kRequestPoolSize);
  EXPECT_EQ(info.rx_depth(), RndisFunction::kRequestPoolSize);

  fdf::WireUnownedResult mac_result = net_port_client_.buffer(arena)->GetMac();
  ASSERT_TRUE(mac_result.ok()) << mac_result.error().FormatDescription();
  ASSERT_TRUE(mac_result->mac_ifc.is_valid());
  fdf::WireSyncClient<fnetdev::MacAddr> mac_client(std::move(mac_result->mac_ifc));
  fdf::WireUnownedResult mac_addr_result = mac_client.buffer(arena)->GetAddress();
  ASSERT_TRUE(mac_addr_result.ok()) << mac_addr_result.error().FormatDescription();
  EXPECT_THAT(mac_addr_result.value().mac.octets, ::testing::ElementsAreArray(kMacAddr));
}

TEST_F(RndisFunctionTest, NetworkDeviceStartStop) {
  fdf::Arena arena(kArenaTag);
  auto start_result = net_impl_client_.buffer(arena)->Start();
  ASSERT_TRUE(start_result.ok());
  ASSERT_OK(start_result->s);

  // Set a packet filter to put the device online.
  SetPacketFilter();
  fdf::WireUnownedResult result = net_port_client_.buffer(arena)->GetStatus();
  ASSERT_TRUE(result.ok()) << result.error().FormatDescription();
  fuchsia_hardware_network::wire::PortStatus& status = result.value().status;
  EXPECT_EQ(status.flags(), fuchsia_hardware_network::wire::StatusFlags::kOnline);
  EXPECT_EQ(status.mtu(), RndisFunction::kMtu);

  auto stop_result = net_impl_client_.buffer(arena)->Stop();
  ASSERT_TRUE(stop_result.ok());
  ReadIndicateStatus(RNDIS_STATUS_MEDIA_DISCONNECT);
}

TEST_F(RndisFunctionTest, InvalidSizeCommand) {
  std::vector<uint8_t> invalid_data = {0xa, 0xb};
  WriteCommand(invalid_data.data(), invalid_data.size());

  std::vector<uint8_t> buffer(sizeof(rndis_indicate_status) + sizeof(rndis_diagnostic_info) +
                              invalid_data.size());
  ReadResponse(buffer.data(), buffer.size());
  auto status = reinterpret_cast<rndis_indicate_status*>(buffer.data());
  EXPECT_EQ(status->msg_type, static_cast<uint32_t>(RNDIS_INDICATE_STATUS_MSG));
  EXPECT_EQ(status->msg_length, buffer.size());
  EXPECT_EQ(status->status, static_cast<uint32_t>(RNDIS_STATUS_INVALID_DATA));
}

TEST_F(RndisFunctionTest, InitMessage) {
  rndis_init msg{
      .msg_type = RNDIS_INITIALIZE_MSG,
      .msg_length = sizeof(rndis_init),
      .request_id = 42,
      .major_version = RNDIS_MAJOR_VERSION,
      .minor_version = RNDIS_MINOR_VERSION,
      .max_xfer_size = RNDIS_MAX_XFER_SIZE,
  };
  WriteCommand(&msg, sizeof(msg));

  rndis_init_complete response;
  ReadResponse(&response, sizeof(response));

  EXPECT_EQ(response.msg_type, static_cast<uint32_t>(RNDIS_INITIALIZE_CMPLT));
  EXPECT_EQ(response.msg_length, sizeof(response));
  EXPECT_EQ(response.request_id, 42u);
  EXPECT_EQ(response.status, static_cast<uint32_t>(RNDIS_STATUS_SUCCESS));
  EXPECT_EQ(response.major_version, static_cast<uint32_t>(RNDIS_MAJOR_VERSION));
  EXPECT_EQ(response.minor_version, static_cast<uint32_t>(RNDIS_MINOR_VERSION));
  EXPECT_EQ(response.device_flags, static_cast<uint32_t>(RNDIS_DF_CONNECTIONLESS));
  EXPECT_EQ(response.medium, static_cast<uint32_t>(RNDIS_MEDIUM_802_3));
  EXPECT_EQ(response.max_packets_per_xfer, 1u);
  EXPECT_EQ(response.max_xfer_size, static_cast<uint32_t>(RNDIS_MAX_XFER_SIZE));
  EXPECT_EQ(response.packet_alignment, 0u);
  EXPECT_EQ(response.reserved0, 0u);
  EXPECT_EQ(response.reserved1, 0u);
}

TEST_F(RndisFunctionTest, Send) {
  fdf::Arena arena(kArenaTag);
  StartNetworkDevice();
  constexpr uint8_t kVmoId = 1;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  fdf::WireUnownedResult vmo_result =
      net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(vmo));
  ASSERT_TRUE(vmo_result.ok()) << vmo_result.error().FormatDescription();
  ASSERT_OK(vmo_result.value().s);

  SetPacketFilter();
  uint8_t bulk_in_addr = 0;
  driver_test_.RunInDriverContext(
      [&](RndisFunction& driver) { bulk_in_addr = driver.BulkInAddress(); });

  uint32_t transmit_ok, transmit_errors, transmit_no_buffer;
  size_t actual;
  QueryOid(OID_GEN_RCV_OK, &transmit_ok, sizeof(transmit_ok), &actual);
  QueryOid(OID_GEN_RCV_ERROR, &transmit_errors, sizeof(transmit_errors), &actual);
  QueryOid(OID_GEN_RCV_NO_BUFFER, &transmit_no_buffer, sizeof(transmit_no_buffer), &actual);
  EXPECT_EQ(transmit_ok, 0u);
  EXPECT_EQ(transmit_errors, 0u);
  EXPECT_EQ(transmit_no_buffer, 0u);

  constexpr uint64_t kRequestLength = 64;
  constexpr size_t kRequestCount = RndisFunction::kRequestPoolSize;

  // Fill the TX queue.

  libsync::Completion tx_enqueued;
  RunInFunctionContext([&](FakeFunction& function) {
    function.fake_endpoint(bulk_in_addr).set_on_queue_requests([&](FakeEndpoint& ep) {
      if (ep.pending_request_count() == kRequestCount) {
        tx_enqueued.Signal();
      }
    });
  });

  for (size_t i = 0; i != kRequestCount; ++i) {
    fnetdev::wire::BufferRegion region = {
        .vmo = kVmoId,
        .offset = 0,
        .length = kRequestLength,
    };
    fnetdev::wire::TxBuffer tx_buffer{
        .id = static_cast<uint32_t>(i),
        .data = fidl::VectorView<fnetdev::wire::BufferRegion>::FromExternal(&region, 1),
    };
    ASSERT_OK(net_impl_client_.buffer(arena)
                  ->QueueTx(fidl::VectorView<fnetdev::wire::TxBuffer>::FromExternal(&tx_buffer, 1))
                  .status());
  }

  tx_enqueued.Wait();

  QueryOid(OID_GEN_RCV_OK, &transmit_ok, sizeof(transmit_ok), &actual);
  QueryOid(OID_GEN_RCV_ERROR, &transmit_errors, sizeof(transmit_errors), &actual);
  QueryOid(OID_GEN_RCV_NO_BUFFER, &transmit_no_buffer, sizeof(transmit_no_buffer), &actual);
  EXPECT_EQ(transmit_ok, kRequestCount);
  EXPECT_EQ(transmit_errors, 0u);
  EXPECT_EQ(transmit_no_buffer, 0u);

  // One more packet should fail. The other packets haven't completed yet.
  libsync::Completion tx_completed;
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    env.fake_ifc_.set_on_complete_tx([&]() { tx_completed.Signal(); });
  });
  {
    fnetdev::wire::BufferRegion region = {
        .vmo = kVmoId,
        .offset = 0,
        .length = kRequestLength,
    };
    fnetdev::wire::TxBuffer tx_buffer{
        .id = kRequestCount,
        .data = fidl::VectorView<fnetdev::wire::BufferRegion>::FromExternal(&region, 1),
    };
    ASSERT_OK(net_impl_client_.buffer(arena)
                  ->QueueTx(fidl::VectorView<fnetdev::wire::TxBuffer>::FromExternal(&tx_buffer, 1))
                  .status());
  }
  tx_completed.Wait();
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    std::optional complete = env.fake_ifc_.PopCompleteTx();
    ASSERT_TRUE(complete.has_value());
    EXPECT_EQ(complete->id, kRequestCount);
    EXPECT_EQ(complete->status, ZX_ERR_NO_RESOURCES);
  });
  tx_completed.Reset();

  QueryOid(OID_GEN_RCV_OK, &transmit_ok, sizeof(transmit_ok), &actual);
  QueryOid(OID_GEN_RCV_ERROR, &transmit_errors, sizeof(transmit_errors), &actual);
  QueryOid(OID_GEN_RCV_NO_BUFFER, &transmit_no_buffer, sizeof(transmit_no_buffer), &actual);
  EXPECT_EQ(transmit_ok, 8u);
  EXPECT_EQ(transmit_errors, 0u);
  EXPECT_EQ(transmit_no_buffer, 1u);

  // Complete one more request and wait.
  RunInFunctionContext([&](FakeFunction& function) {
    function.fake_endpoint(bulk_in_addr).RequestComplete(ZX_OK, kRequestLength);
  });
  tx_completed.Wait();
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    std::optional complete = env.fake_ifc_.PopCompleteTx();
    ASSERT_TRUE(complete.has_value());
    EXPECT_EQ(complete->id, 0u);
    EXPECT_EQ(complete->status, ZX_OK);
  });

  // Now we can queue again.
  tx_enqueued.Reset();
  RunInFunctionContext([&](FakeFunction& function) {
    function.fake_endpoint(bulk_in_addr).set_on_queue_requests([&](FakeEndpoint& ep) {
      tx_enqueued.Signal();
    });
  });
  {
    fnetdev::wire::BufferRegion region = {
        .vmo = kVmoId,
        .offset = 0,
        .length = kRequestLength,
    };
    fnetdev::wire::TxBuffer tx_buffer{
        .id = kRequestCount + 1,
        .data = fidl::VectorView<fnetdev::wire::BufferRegion>::FromExternal(&region, 1),
    };
    ASSERT_OK(net_impl_client_.buffer(arena)
                  ->QueueTx(fidl::VectorView<fnetdev::wire::TxBuffer>::FromExternal(&tx_buffer, 1))
                  .status());
  }
  tx_enqueued.Wait();

  QueryOid(OID_GEN_RCV_OK, &transmit_ok, sizeof(transmit_ok), &actual);
  QueryOid(OID_GEN_RCV_ERROR, &transmit_errors, sizeof(transmit_errors), &actual);
  QueryOid(OID_GEN_RCV_NO_BUFFER, &transmit_no_buffer, sizeof(transmit_no_buffer), &actual);
  EXPECT_EQ(transmit_ok, kRequestCount + 1);
  EXPECT_EQ(transmit_errors, 0u);
  EXPECT_EQ(transmit_no_buffer, 1u);

  driver_test_.RunInEnvironmentTypeContext(
      [&](RndisFunctionTestEnvironment& env) { env.fake_ifc_.set_on_complete_tx(nullptr); });
}

TEST_F(RndisFunctionTest, Receive) {
  fdf::Arena arena(kArenaTag);
  StartNetworkDevice();

  constexpr uint8_t kVmoId = 1;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  zx::vmo duplicate;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
  fdf::WireUnownedResult vmo_result =
      net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(duplicate));
  ASSERT_TRUE(vmo_result.ok()) << vmo_result.error().FormatDescription();
  ASSERT_OK(vmo_result.value().s);

  SetPacketFilter();

  constexpr size_t kPayloadSize = 16;
  struct Payload {
    rndis_packet_header header;
    char data[kPayloadSize];
  };
  Payload payload = {};
  payload.header.msg_type = RNDIS_PACKET_MSG;
  payload.header.msg_length = sizeof(payload);
  payload.header.data_offset =
      sizeof(rndis_packet_header) - offsetof(rndis_packet_header, data_offset);
  payload.header.data_length = sizeof(payload.data);
  for (size_t i = 0; i != kPayloadSize; ++i) {
    payload.data[i] = static_cast<char>(i);
  }

  uint8_t bulk_out_addr = 0;
  driver_test_.RunInDriverContext(
      [&](RndisFunction& driver) { bulk_out_addr = driver.BulkOutAddress(); });

  libsync::Completion rx_completed;

  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    auto* payload_bytes = reinterpret_cast<uint8_t*>(&payload);
    env.fake_ifc_.set_on_complete_rx([&]() { rx_completed.Signal(); });
    env.fake_function_.fake_endpoint(bulk_out_addr)
        .RequestComplete(ZX_OK,
                         std::vector<uint8_t>(payload_bytes, payload_bytes + sizeof(payload)));
  });

  constexpr uint32_t kRxBufferId = 111;
  fnetdev::wire::RxSpaceBuffer rx_buffer = {
      .id = kRxBufferId,
      .region =
          {
              .vmo = kVmoId,
              .offset = 0,
              .length = 4096,
          },
  };
  fidl::OneWayStatus rx_result = net_impl_client_.buffer(arena)->QueueRxSpace(
      fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&rx_buffer, 1));
  ASSERT_TRUE(rx_result.ok()) << rx_result.error().FormatDescription();

  rx_completed.Wait();
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    std::optional rx = env.fake_ifc_.PopCompleteRx();
    ASSERT_TRUE(rx.has_value());
    EXPECT_EQ(rx->meta().port(), RndisFunction::kPortId);
    EXPECT_EQ(rx->meta().frame_type(), fuchsia_hardware_network::wire::FrameType::kEthernet);
    ASSERT_EQ(rx->data().size(), 1u);
    const fnetdev::RxBufferPart& rx_buffer = rx->data()[0];
    EXPECT_EQ(rx_buffer.id(), kRxBufferId);
    EXPECT_EQ(rx_buffer.offset(), 0u);
    EXPECT_EQ(rx_buffer.length(), kPayloadSize);
    std::array<char, kPayloadSize> read_buff;
    EXPECT_OK(vmo.read(read_buff.data(), 0, kPayloadSize));
    EXPECT_THAT(read_buff, ::testing::ElementsAreArray(payload.data));
  });
}

TEST_F(RndisFunctionTest, KeepAliveMessage) {
  rndis_header msg{
      .msg_type = RNDIS_KEEPALIVE_MSG,
      .msg_length = sizeof(rndis_header),
      .request_id = 42,
  };
  WriteCommand(&msg, sizeof(msg));

  rndis_header_complete response;
  ReadResponse(&response, sizeof(response));

  EXPECT_EQ(response.msg_type, static_cast<uint32_t>(RNDIS_KEEPALIVE_CMPLT));
  EXPECT_EQ(response.msg_length, sizeof(response));
  EXPECT_EQ(response.request_id, 42u);
  EXPECT_EQ(response.status, static_cast<uint32_t>(RNDIS_STATUS_SUCCESS));
}

TEST_F(RndisFunctionTest, Halt) {
  StartNetworkDevice();

  libsync::Completion port_status_changed;
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    env.fake_ifc_.set_on_port_status_changed([&]() { port_status_changed.Signal(); });
  });

  SetPacketFilter();
  port_status_changed.Wait();
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    ASSERT_TRUE(env.fake_ifc_.last_online().has_value());
    EXPECT_TRUE(env.fake_ifc_.last_online().value());
  });
  port_status_changed.Reset();

  rndis_header msg{
      .msg_type = RNDIS_HALT_MSG,
      .msg_length = sizeof(rndis_header),
      .request_id = 42,
  };
  WriteCommand(&msg, sizeof(msg));

  port_status_changed.Wait();
  driver_test_.RunInEnvironmentTypeContext([](RndisFunctionTestEnvironment& env) {
    EXPECT_EQ(env.fake_function_.DisableEpCalls(), 3u);
    ASSERT_TRUE(env.fake_ifc_.last_online().has_value());
    EXPECT_FALSE(env.fake_ifc_.last_online().value());
  });
  port_status_changed.Reset();
}

TEST_F(RndisFunctionTest, Inspect) {
  fdf::Arena arena(kArenaTag);
  StartNetworkDevice();

  constexpr uint8_t kVmoId = 1;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  zx::vmo duplicate;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
  fdf::WireUnownedResult vmo_result =
      net_impl_client_.buffer(arena)->PrepareVmo(kVmoId, std::move(duplicate));
  ASSERT_TRUE(vmo_result.ok()) << vmo_result.error().FormatDescription();
  ASSERT_OK(vmo_result.value().s);

  SetPacketFilter();

  uint8_t bulk_in_addr = 0;
  uint8_t bulk_out_addr = 0;
  driver_test_.RunInDriverContext([&](RndisFunction& driver) {
    bulk_in_addr = driver.BulkInAddress();
    bulk_out_addr = driver.BulkOutAddress();
  });

  // 1. Send (TX) 1 packet of size 64
  constexpr uint64_t kRequestLength = 64;
  fnetdev::wire::BufferRegion region = {
      .vmo = kVmoId,
      .offset = 0,
      .length = kRequestLength,
  };
  fnetdev::wire::TxBuffer tx_buffer{
      .id = 0,
      .data = fidl::VectorView<fnetdev::wire::BufferRegion>::FromExternal(&region, 1),
  };
  libsync::Completion tx_completed;
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    env.fake_ifc_.set_on_complete_tx([&]() { tx_completed.Signal(); });
  });
  ASSERT_OK(net_impl_client_.buffer(arena)
                ->QueueTx(fidl::VectorView<fnetdev::wire::TxBuffer>::FromExternal(&tx_buffer, 1))
                .status());

  RunInFunctionContext([&](FakeFunction& function) {
    function.fake_endpoint(bulk_in_addr).RequestComplete(ZX_OK, kRequestLength);
  });
  tx_completed.Wait();

  // 2. Receive (RX) 1 packet with payload size 16
  constexpr size_t kPayloadSize = 16;
  struct Payload {
    rndis_packet_header header;
    char data[kPayloadSize];
  };
  Payload payload = {};
  payload.header.msg_type = RNDIS_PACKET_MSG;
  payload.header.msg_length = sizeof(payload);
  payload.header.data_offset =
      sizeof(rndis_packet_header) - offsetof(rndis_packet_header, data_offset);
  payload.header.data_length = sizeof(payload.data);

  libsync::Completion rx_completed;
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    auto* payload_bytes = reinterpret_cast<uint8_t*>(&payload);
    env.fake_ifc_.set_on_complete_rx([&]() { rx_completed.Signal(); });
    env.fake_function_.fake_endpoint(bulk_out_addr)
        .RequestComplete(ZX_OK,
                         std::vector<uint8_t>(payload_bytes, payload_bytes + sizeof(payload)));
  });

  constexpr uint32_t kRxBufferId = 111;
  fnetdev::wire::RxSpaceBuffer rx_buffer = {
      .id = kRxBufferId,
      .region =
          {
              .vmo = kVmoId,
              .offset = 0,
              .length = 4096,
          },
  };
  fidl::OneWayStatus rx_result = net_impl_client_.buffer(arena)->QueueRxSpace(
      fidl::VectorView<fnetdev::wire::RxSpaceBuffer>::FromExternal(&rx_buffer, 1));
  ASSERT_TRUE(rx_result.ok()) << rx_result.error().FormatDescription();

  rx_completed.Wait();

  // 3. Trigger throughput and verify
  driver_test_.RunInDriverContext([](RndisFunction& driver) {
    driver.GetThroughputTrackerForTesting().MeasureForTesting(zx::sec(1));

    auto hierarchy = usb_inspect::ReadHierarchyFromInspector(driver.inspector().inspector());

    auto* root_node = hierarchy.GetByPath({"rndis-function"});
    ASSERT_NE(nullptr, root_node);

    auto* bulk_in = hierarchy.GetByPath({"rndis-function", "bulk_in"});
    ASSERT_NE(nullptr, bulk_in);
    // Expected TX: 64 bytes, tx_pending=0, max_rate=64.
    auto err_in =
        usb_inspect::VerifyEndpointInspect(bulk_in, 64, std::nullopt, 0, std::nullopt, 64, 0);
    EXPECT_TRUE(err_in.is_ok()) << err_in.error_value();

    auto* bulk_out = hierarchy.GetByPath({"rndis-function", "bulk_out"});
    ASSERT_NE(nullptr, bulk_out);
    // Expected RX: 60 bytes, rx_pending=8, max_rate=60.
    auto err_out =
        usb_inspect::VerifyEndpointInspect(bulk_out, std::nullopt, 60, std::nullopt, 8, 60, 0);
    EXPECT_TRUE(err_out.is_ok()) << err_out.error_value();

    auto* notification = hierarchy.GetByPath({"rndis-function", "notification"});
    ASSERT_NE(nullptr, notification);
    // Expected Interrupt TX: 0 bytes (no notifications sent), tx_pending=std::nullopt, max_rate=0.
    auto err_intr = usb_inspect::VerifyEndpointInspect(notification, 0, std::nullopt, std::nullopt,
                                                       std::nullopt, 0, 0);
    EXPECT_TRUE(err_intr.is_ok()) << err_intr.error_value();
  });

  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    env.fake_ifc_.set_on_complete_tx(nullptr);
    env.fake_ifc_.set_on_complete_rx(nullptr);
  });
  fidl::Result deconfig_result = function_interface_client_->SetConfigured({{
      .configured = false,
      .speed = fdescriptor::UsbSpeed::kFull,
  }});
  ASSERT_TRUE(deconfig_result.is_ok()) << deconfig_result.error_value().FormatDescription();
}

TEST_F(RndisFunctionTest, Reset) {
  StartNetworkDevice();
  libsync::Completion port_status_changed;
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    env.fake_ifc_.set_on_port_status_changed([&]() { port_status_changed.Signal(); });
  });

  SetPacketFilter();
  port_status_changed.Wait();
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    ASSERT_TRUE(env.fake_ifc_.last_online().has_value());
    EXPECT_TRUE(env.fake_ifc_.last_online().value());
  });
  port_status_changed.Reset();

  rndis_header msg{
      .msg_type = RNDIS_RESET_MSG,
      .msg_length = sizeof(rndis_header),
      .request_id = 42,
  };
  WriteCommand(&msg, sizeof(msg));

  port_status_changed.Wait();
  driver_test_.RunInEnvironmentTypeContext([&](RndisFunctionTestEnvironment& env) {
    ASSERT_TRUE(env.fake_ifc_.last_online().has_value());
    EXPECT_FALSE(env.fake_ifc_.last_online().value());
  });

  rndis_reset_complete response;
  ReadResponse(&response, sizeof(response));
  EXPECT_EQ(response.msg_type, static_cast<uint32_t>(RNDIS_RESET_CMPLT));
  EXPECT_EQ(response.msg_length, sizeof(rndis_reset_complete));
  EXPECT_EQ(response.status, static_cast<uint32_t>(RNDIS_STATUS_SUCCESS));
}

TEST_F(RndisFunctionTest, OidSupportedList) {
  uint32_t supported_oids[100];
  size_t actual;
  QueryOid(OID_GEN_SUPPORTED_LIST, &supported_oids, sizeof(supported_oids), &actual);
  ASSERT_GE(actual, sizeof(uint32_t));
  ASSERT_EQ(actual % sizeof(uint32_t), 0u);

  // Check that the list at least contains the list OID itself.
  bool contains_list_oid = false;
  for (size_t i = 0; i < actual / sizeof(uint32_t); ++i) {
    if (supported_oids[i] == OID_GEN_SUPPORTED_LIST) {
      contains_list_oid = true;
      break;
    }
  }
  EXPECT_TRUE(contains_list_oid);
}

TEST_F(RndisFunctionTest, OidHardwareStatus) {
  uint32_t hardware_status;
  size_t actual;
  QueryOid(OID_GEN_HARDWARE_STATUS, &hardware_status, sizeof(hardware_status), &actual);
  ASSERT_EQ(actual, sizeof(hardware_status));
  EXPECT_EQ(hardware_status, static_cast<uint32_t>(RNDIS_HW_STATUS_READY));
}

TEST_F(RndisFunctionTest, OidLinkSpeed) {
  ASSERT_OK(function_interface_client_->SetConfigured({{
      .configured = true,
      .speed = fdescriptor::UsbSpeed::kFull,
  }}));
  uint32_t speed;
  size_t actual;
  QueryOid(OID_GEN_LINK_SPEED, &speed, sizeof(speed), &actual);
  ASSERT_EQ(actual, sizeof(speed));
  EXPECT_EQ(speed, 120'000u);
}

TEST_F(RndisFunctionTest, OidMediaConnectStatus) {
  uint32_t status;
  size_t actual;
  QueryOid(OID_GEN_MEDIA_CONNECT_STATUS, &status, sizeof(status), &actual);
  ASSERT_EQ(actual, sizeof(status));
  EXPECT_EQ(status, static_cast<uint32_t>(RNDIS_STATUS_MEDIA_CONNECT));
}

TEST_F(RndisFunctionTest, OidPhysicalMedium) {
  uint32_t medium;
  size_t actual;
  QueryOid(OID_GEN_PHYSICAL_MEDIUM, &medium, sizeof(medium), &actual);
  ASSERT_EQ(actual, sizeof(medium));
  EXPECT_EQ(medium, static_cast<uint32_t>(RNDIS_MEDIUM_802_3));
}

TEST_F(RndisFunctionTest, OidMaximumSize) {
  uint32_t size;
  size_t actual;
  QueryOid(OID_GEN_MAXIMUM_TOTAL_SIZE, &size, sizeof(size), &actual);
  ASSERT_EQ(actual, sizeof(size));
  EXPECT_EQ(size, static_cast<uint32_t>(RNDIS_MAX_DATA_SIZE));
}

TEST_F(RndisFunctionTest, OidMacAddress) {
  std::array<uint8_t, RndisFunction::kEthMacSize> mac_addr;
  size_t actual;
  QueryOid(OID_802_3_PERMANENT_ADDRESS, mac_addr.data(), mac_addr.size(), &actual);
  ASSERT_EQ(actual, mac_addr.size());

  std::array<uint8_t, RndisFunction::kEthMacSize> expected = kMacAddr;
  expected[5] ^= 1;

  ASSERT_EQ(mac_addr, expected);
}

TEST_F(RndisFunctionTest, OidVendorDescription) {
  char description[100];
  size_t actual;
  QueryOid(OID_GEN_VENDOR_DESCRIPTION, &description, sizeof(description), &actual);
  EXPECT_STREQ(description, "Google");
}

}  // namespace
}  // namespace rndis_function
