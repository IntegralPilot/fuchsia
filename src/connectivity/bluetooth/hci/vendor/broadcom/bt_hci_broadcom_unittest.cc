// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bt_hci_broadcom.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.bluetooth/cpp/fidl.h>
#include <fidl/fuchsia.hardware.bluetooth/cpp/wire.h>
#include <fidl/fuchsia.power.broker/cpp/test_base.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/sync/cpp/completion.h>

#include <algorithm>

#include <gtest/gtest.h>

#include "fidl/fuchsia.hardware.bluetooth/cpp/markers.h"
#include "lib/driver/component/cpp/driver_base.h"
#include "lib/fidl/cpp/unified_messaging_declarations.h"
#include "lib/fidl/cpp/wire/unknown_interaction_handler.h"
#include "src/connectivity/bluetooth/hci/vendor/broadcom/bt_hci_broadcom_config.h"
#include "src/connectivity/bluetooth/hci/vendor/broadcom/packets.emb.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vmo_file.h"

namespace bt_hci_broadcom {

namespace {
namespace fhbt = fuchsia_hardware_bluetooth;

// Firmware binaries are a sequence of HCI commands containing the firmware as payloads. For
// testing, we use 1 HCI command with a 1 byte payload.
const std::vector<uint8_t> kFirmware = {
    0x01,
    0x02,  // arbitrary "firmware opcode"
    0x01,  // parameter_total_size
    0x03,  // payload
};
constexpr uint16_t kTestFirmwareOpCode = 0x0201;
const std::vector<std::string> kFirmwarePaths = {"BCM4345C5.hcd", "BCM4381A1.hcd"};

constexpr uint8_t kNoFastDownloadChipId = 173;
constexpr uint8_t kFastDownloadChipId = 174;

const std::array<uint8_t, 6> kMacAddress = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05};

const std::array<uint8_t, 6> kCommandCompleteEvent = {
    0x0e,        // command complete event code
    0x04,        // parameter_total_size
    0x01,        // num_hci_command_packets
    0x00, 0x00,  // command opcode (hardcoded for simplicity since this isn't checked by the driver)
    0x00,        // return_code (success)
};

std::vector<uint8_t> MakeReadVerboseConfigVersionInfoCommandCompleteEvent(uint8_t chip_id) {
  return {
      0x0e,  // command complete event code
      0x05,  // parameter_total_size
      0x01,  // num_hci_command_packets
      0x79,  // command opcode LSB (ReadVerboseConfigVersionInfo)
      0xfc,  // command opcode MSB (ReadVerboseConfigVersionInfo)
      0x00,  // status (success)
      chip_id,
  };
}

using fuchsia_power_system::LeaseToken;

constexpr zx::duration kDefaultHostIdleThreshold = zx::usec(12500);

class FakePowerBroker : public fidl::Server<fuchsia_power_broker::Topology>,
                        public fidl::Server<fuchsia_power_broker::Lessor>,
                        public fidl::Server<fuchsia_power_broker::LeaseControl>,
                        public fidl::testing::TestBase<fuchsia_power_broker::ElementControl> {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) {
    return to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_broker::Topology>(
        topology_bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         fidl::kIgnoreBindingClosure));
  }

  std::optional<uint8_t> lease_power_level() const { return lease_power_level_; }

  zx::unowned_event dependency_token() const { return dependency_token_.borrow(); }

  fidl::ServerEnd<fuchsia_power_broker::LeaseControl> TakeLeaseControlServerEnd() {
    return std::move(lease_control_server_end_);
  }

  void SatisfyLease() {
    ASSERT_TRUE(lease_control_server_end_.is_valid());
    lease_control_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                       std::move(lease_control_server_end_), this,
                                       fidl::kIgnoreBindingClosure);
  }

  bool IsLeaseControlClosed() {
    if (!lease_control_server_end_.is_valid()) {
      return true;
    }
    zx_signals_t observed{};
    auto result = lease_control_server_end_.channel().wait_one(
        ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &observed);
    return (result == ZX_OK && (observed & ZX_CHANNEL_PEER_CLOSED));
  }

  bool IsLeaseControlReadable() {
    if (!lease_control_server_end_.is_valid()) {
      return false;
    }
    zx_signals_t observed{};
    auto result = lease_control_server_end_.channel().wait_one(
        ZX_CHANNEL_READABLE, zx::time::infinite_past(), &observed);
    return (result == ZX_OK && (observed & ZX_CHANNEL_READABLE));
  }

  bool IsLeaseBound() {
    bool has_bindings = false;
    lease_control_bindings_.ForEachBinding([&](const auto& binding) { has_bindings = true; });
    return has_bindings;
  }

  // Verify that the lease has been released at this point (and reset lease power level)
  void ExpectLeaseReleased() {
    EXPECT_TRUE(lease_control_server_end_.is_valid());
    if (!lease_control_server_end_.is_valid()) {
      return;
    }
    zx_signals_t observed{};
    EXPECT_EQ(lease_control_server_end_.channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                           zx::time::infinite_past(), &observed),
              ZX_OK);
    EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);
    if (observed & ZX_CHANNEL_PEER_CLOSED) {
      lease_control_server_end_.reset();
      lease_power_level_.reset();
    }
  }

  fidl::ClientEnd<fuchsia_power_broker::ElementRunner> TakeElementRunnerClientEnd() {
    return std::move(element_runner_client_end_);
  }

  // fuchsia.power.broker/Topology
  void AddElement(fuchsia_power_broker::ElementSchema& req,
                  AddElementCompleter::Sync& completer) override {
    if (!req.lessor_channel() || !req.element_control() || !req.element_runner()) {
      completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
      return;
    }

    lessor_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                *std::move(req.lessor_channel()), this,
                                fidl::kIgnoreBindingClosure);
    element_control_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         *std::move(req.element_control()), this,
                                         fidl::kIgnoreBindingClosure);
    element_runner_client_end_ = *std::move(req.element_runner());

    completer.Reply(fit::success());
  }
  void Lease(
      fidl::Server<fuchsia_power_broker::Topology>::LeaseRequest& req,
      fidl::Server<fuchsia_power_broker::Topology>::LeaseCompleter::Sync& completer) override {
    completer.Reply(fit::success());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Topology> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  // fuchsia.power.broker/Lessor
  void Lease(fidl::Server<fuchsia_power_broker::Lessor>::LeaseRequest& request,
             fidl::Server<fuchsia_power_broker::Lessor>::LeaseCompleter::Sync& completer) override {
    EXPECT_FALSE(lease_power_level_);
    lease_power_level_ = request.level();

    auto [lease_control_client_end, lease_control_server_end] =
        fidl::Endpoints<fuchsia_power_broker::LeaseControl>::Create();
    lease_control_server_end_ = std::move(lease_control_server_end);
    completer.Reply(fit::ok(std::move(lease_control_client_end)));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Lessor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  // fuchsia.power.broker/LeaseControl
  void WatchStatus(WatchStatusRequest& request, WatchStatusCompleter::Sync& completer) override {
    completer.Reply(fuchsia_power_broker::LeaseStatus::kSatisfied);
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::LeaseControl> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  // fuchsia.power.broker/ElementControl
  void RegisterDependencyToken(RegisterDependencyTokenRequest& request,
                               RegisterDependencyTokenCompleter::Sync& completer) override {
    if (dependency_token_.is_valid()) {
      completer.Reply(
          fit::error(fuchsia_power_broker::RegisterDependencyTokenError::kAlreadyInUse));
      return;
    }

    dependency_token_ = std::move(request.token());
    completer.Reply(fit::ok());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementControl> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override { FAIL(); }

 private:
  fidl::ServerBindingGroup<fuchsia_power_broker::Topology> topology_bindings_;

  fidl::ServerBindingGroup<fuchsia_power_broker::Lessor> lessor_bindings_;
  fidl::ServerBindingGroup<fuchsia_power_broker::ElementControl> element_control_bindings_;
  fidl::ServerBindingGroup<fuchsia_power_broker::LeaseControl> lease_control_bindings_;
  fidl::ClientEnd<fuchsia_power_broker::ElementRunner> element_runner_client_end_;

  std::optional<uint8_t> lease_power_level_;
  fidl::ServerEnd<fuchsia_power_broker::LeaseControl> lease_control_server_end_;
  zx::event dependency_token_;
};

class FakePowerTokenProvider : public fidl::Server<fuchsia_hardware_power::PowerTokenProvider> {
 public:
  fuchsia_hardware_power::PowerTokenService::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_power::PowerTokenService::InstanceHandler({
        .token_provider = bindings_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
    });
  }

  void GetToken(GetTokenCompleter::Sync& completer) override {
    zx::event token;
    ASSERT_TRUE(zx::event::create(0, &token) == ZX_OK);
    completer.Reply(
        fit::success(fuchsia_hardware_power::PowerTokenProviderGetTokenResponse{std::move(token)}));
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power::PowerTokenProvider> md,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_power::PowerTokenProvider> bindings_;
};

class FakeTransportDevice : public fdf::WireServer<fuchsia_hardware_serialimpl::Device>,
                            public fidl::Server<fhbt::HciTransport>,
                            public fidl::Server<fhbt::Snoop> {
 public:
  explicit FakeTransportDevice() = default;

  fuchsia_hardware_serialimpl::Service::InstanceHandler GetSerialInstanceHandler() {
    return fuchsia_hardware_serialimpl::Service::InstanceHandler({
        .device = serial_binding_group_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                                      fidl::kIgnoreBindingClosure),
    });
  }
  fhbt::HciService::InstanceHandler GetHciInstanceHandler() {
    return fhbt::HciService::InstanceHandler({
        .hci_transport = hci_transport_binding_group_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
        .snoop = snoop_binding_group_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
    });
  }

  void SetCustomizedReply(std::vector<uint8_t> reply) {
    customized_reply_.emplace(std::move(reply));
  }

  // fhbt::HciTransport request handler implementations:
  void Send(SendRequest& request, SendCompleter::Sync& completer) override {
    uint16_t opcode = 0xFFFF;
    if (request.Which() == fhbt::SentPacket::Tag::kCommand) {
      // The command opcode is the first two bytes.
      std::vector<uint8_t>& packet = request.command().value();
      opcode = static_cast<uint16_t>(packet[1] << 8) | static_cast<uint16_t>(packet[0]);
      received_packets_.insert_or_assign(opcode, packet);

      if (opcode == static_cast<uint16_t>(BroadcomOpCode::SET_DOWNLOAD_CONFIG)) {
        auto view = MakeSetDownloadConfigCommandView(packet.data(), packet.size());
        if (view.Ok() && view.fast_download_mode().Read() == 0x01) {
          fast_download_mode_ = true;
        }
      }
    }

    if (opcode == static_cast<uint16_t>(BroadcomOpCode::LAUNCH_RAM) ||
        opcode == static_cast<uint16_t>(pw::bluetooth::emboss::OpCode::RESET)) {
      fast_download_mode_ = false;
    }

    std::vector<uint8_t> reply;

    if (fast_download_mode_ && opcode == kTestFirmwareOpCode) {
      // Suppress event for firmware in fast download mode (reply remains empty).
    } else if (opcode == static_cast<uint16_t>(BroadcomOpCode::READ_VERBOSE_CONFIG_VERSION_INFO)) {
      reply = MakeReadVerboseConfigVersionInfoCommandCompleteEvent(chip_id_);
    } else if (customized_reply_) {
      reply = *customized_reply_;
    } else {
      reply = std::vector<uint8_t>(kCommandCompleteEvent.data(),
                                   kCommandCompleteEvent.data() + kCommandCompleteEvent.size());
    }

    if (!reply.empty()) {
      hci_transport_binding_group_.ForEachBinding(
          [&](const fidl::ServerBinding<fhbt::HciTransport>& binding) {
            auto received_packet = fhbt::ReceivedPacket::WithEvent(reply);
            fit::result<fidl::OneWayError> result =
                fidl::SendEvent(binding)->OnReceive(received_packet);
            ASSERT_FALSE(result.is_error());
          });
    }
    completer.Reply();
  }

  void SendEvent(std::vector<uint8_t> event_data) {
    hci_transport_binding_group_.ForEachBinding(
        [&](const fidl::ServerBinding<fhbt::HciTransport>& binding) {
          auto event = fhbt::ReceivedPacket::WithEvent(event_data);
          fit::result<fidl::OneWayError> result = fidl::SendEvent(binding)->OnReceive(event);
          ASSERT_FALSE(result.is_error());
        });
  }
  void AckReceive(AckReceiveCompleter::Sync& completer) override {}
  void ConfigureSco(
      fidl::Server<fhbt::HciTransport>::ConfigureScoRequest& request,
      fidl::Server<fhbt::HciTransport>::ConfigureScoCompleter::Sync& completer) override {}
  void handle_unknown_method(::fidl::UnknownMethodMetadata<fhbt::HciTransport> metadata,
                             ::fidl::UnknownMethodCompleter::Sync& completer) override {
    ZX_PANIC("Unknown method in HciTransport requests");
  }

  void SetSerialPid(uint16_t serial_pid) { serial_pid_ = serial_pid; }
  void SetChipId(uint8_t chip_id) { chip_id_ = chip_id; }

  bool HasReceivedOpCode(uint16_t opcode) const { return received_packets_.contains(opcode); }

  std::optional<const std::vector<uint8_t>> LastPacketByOpCode(uint16_t opcode) const {
    auto it = received_packets_.find(opcode);
    if (it == received_packets_.end()) {
      return {};
    }
    return it->second;
  }

  // fuchsia_hardware_serialimpl::Device FIDL request handler implementation.
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override {
    fuchsia_hardware_serial::wire::SerialPortInfo info = {
        .serial_class = fuchsia_hardware_serial::Class::kBluetoothHci,
        .serial_pid = serial_pid_,
    };

    completer.buffer(arena).ReplySuccess(info);
  }
  void Config(ConfigRequestView request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }
  void Enable(EnableRequestView request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }
  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override {
    fidl::VectorView<uint8_t> data;
    completer.buffer(arena).ReplySuccess(data);
  }
  void Write(WriteRequestView request, fdf::Arena& arena,
             WriteCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }
  void CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) override {}

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    ZX_PANIC("Unknown method in Serial requests");
  }

  // fidl::Server<fhbt::Snoop> overrides:
  void AcknowledgePackets(AcknowledgePacketsRequest& request,
                          AcknowledgePacketsCompleter::Sync& completer) override {}
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::Snoop> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  std::optional<std::vector<uint8_t>> customized_reply_;
  uint16_t serial_pid_ = PDEV_PID_BCM43458;
  uint8_t chip_id_ = kNoFastDownloadChipId;
  bool fast_download_mode_ = false;
  // The last command received for each opcode is stored.
  std::unordered_map<uint16_t, std::vector<uint8_t>> received_packets_;

  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> serial_binding_group_;
  fidl::ServerBindingGroup<fhbt::HciTransport> hci_transport_binding_group_;
  fidl::ServerBindingGroup<fhbt::Snoop> snoop_binding_group_;
};

class NoOpEventHandler final : public fidl::WireSyncEventHandler<fhbt::HciTransport> {
 public:
  void OnReceive(fidl::WireEvent<fhbt::HciTransport::OnReceive>* event) override {}
  void handle_unknown_event(fidl::UnknownEventMetadata<fhbt::HciTransport> metadata) override {}
};

class TestEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    // Add our package data dir
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    zx_status_t status =
        fdio_open3("/pkg/data/", static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                   server.TakeChannel().release());
    if (status != ZX_OK) {
      return zx::error(status);
    }
    zx::result result = to_driver_vfs.AddDirectoryAt(std::move(client), "pkg", "data");
    if (result.is_error()) {
      return result.take_error();
    }

    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    // Serve our firmware directory locally
    auto dir_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    firmware_server_.SetDispatcher(dispatcher);
    ZX_ASSERT(firmware_server_.ServeDirectory(firmware_dir_, std::move(dir_endpoints.server)) ==
              ZX_OK);
    // Attach the firmware directory endpoint to "pkg/lib"
    ZX_ASSERT(to_driver_vfs.component()
                  .AddDirectoryAt(std::move(dir_endpoints.client), "pkg/lib", "firmware")
                  .is_ok());

    // Add the services that the fake parent driver exposes to the incoming directory of the driver
    // under test.
    result = to_driver_vfs.AddService<fuchsia_hardware_serialimpl::Service>(
        transport_device_.GetSerialInstanceHandler());
    EXPECT_TRUE(result.is_ok());

    EXPECT_EQ(fake_power_broker_.Serve(to_driver_vfs).status_value(), ZX_OK);

    // Serve (fake) power_token_provider.
    result = to_driver_vfs.AddService<fuchsia_hardware_power::PowerTokenService>(
        std::move(fake_power_token_provider_.GetInstanceHandler()), "default");
    if (result.is_error()) {
      return result.take_error();
    }

    result = to_driver_vfs.AddService<fhbt::HciService>(transport_device_.GetHciInstanceHandler());
    EXPECT_TRUE(result.is_ok());

    if (mac_address_.has_value()) {
      zx::result result =
          mac_address_metadata_server_.Serve(to_driver_vfs, dispatcher, mac_address_.value());
      if (result.is_error()) {
        return result.take_error();
      }
    }
    return zx::ok();
  }

  void AddFirmwareFile(const std::vector<uint8_t>& firmware) {
    // Create vmo for firmware file.
    zx::vmo vmo;
    zx::vmo::create(4096, 0, &vmo);
    vmo.write(firmware.data(), 0, firmware.size());
    vmo.set_prop_content_size(firmware.size());

    //  Create firmware file, and add it to the "firmware" directory we added under pkg/lib.
    fbl::RefPtr<fs::VmoFile> firmware_file =
        fbl::MakeRefCounted<fs::VmoFile>(std::move(vmo), firmware.size());
    for (const auto& path : kFirmwarePaths) {
      ZX_ASSERT(firmware_dir_->AddEntry(path, firmware_file) == ZX_OK);
    }
  }

  zx_status_t SetMacAddressMetadata(std::array<uint8_t, 6> mac_address_octets) {
    mac_address_.emplace(
        fuchsia_boot_metadata::MacAddressMetadata({.mac_address{mac_address_octets}}));
    return ZX_OK;
  }

  FakePowerBroker& fake_power_broker() { return fake_power_broker_; }

  FakeTransportDevice transport_device_;

 private:
  fbl::RefPtr<fs::PseudoDir> firmware_dir_ = fbl::MakeRefCounted<fs::PseudoDir>();
  fs::SynchronousVfs firmware_server_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>
      mac_address_metadata_server_;
  FakePowerBroker fake_power_broker_;
  FakePowerTokenProvider fake_power_token_provider_;
  std::optional<fuchsia_boot_metadata::MacAddressMetadata> mac_address_;
};

class FixtureConfig final {
 public:
  using DriverType = BtHciBroadcom;
  using EnvironmentType = TestEnvironment;
};

class BtHciBroadcomTest : public ::gtest::TestLoopFixture {
 public:
  BtHciBroadcomTest() = default;

  void SetUp() override { SetUp(/* enable_suspend=*/false); }

  void SetUp(bool enable_suspend) { enable_suspend_ = enable_suspend; }

  zx::result<> StartDriver() {
    auto result = driver_test().StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs& args) {
      bt_hci_broadcom_config::Config config;
      config.enable_suspend() = enable_suspend_;
      args.config(config.ToVmo());
    });
    if (result.is_ok()) {
      // We can't set the dispatcher in the constructor because the driver is initialized by
      // the test harness in the blocking StartDriver call above, which would deadlock if the test
      // dispatcher were used. The driver framework currently does not support fake time so this
      // appears to be the best way to inject the test dispatcher.
      driver_test().RunInDriverContext(
          [this](BtHciBroadcom& driver) { driver.set_test_dispatcher(dispatcher()); });
    }
    return result;
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 protected:
  void SetFirmware(const std::vector<uint8_t> firmware = kFirmware) {
    driver_test().RunInEnvironmentTypeContext(
        [&](TestEnvironment& env) { env.AddFirmwareFile(firmware); });
  }

  void SetMacAddressMetadata(std::array<uint8_t, 6> mac_address_octets = kMacAddress) {
    ASSERT_EQ(ZX_OK,
              driver_test().RunInEnvironmentTypeContext<zx_status_t>([&](TestEnvironment& env) {
                return env.SetMacAddressMetadata(std::move(mac_address_octets));
              }));
  }

  void OpenVendor() {
    // Connect to Vendor protocol through the Service.
    zx::result connect_result = driver_test().Connect<fhbt::Service::Vendor>();
    ASSERT_EQ(ZX_OK, connect_result.status_value());

    // Bind the channel to a Vendor client end.
    vendor_client_.Bind(std::move(connect_result.value()));

    // Verify features & ensure driver responds to requests.
    fidl::WireResult<fhbt::Vendor::GetFeatures> features = vendor_client_->GetFeatures();
    ASSERT_TRUE(features.ok());
    EXPECT_TRUE(features.value().acl_priority_command());
  }

  void OpenVendorWithHciTransportClient() {
    // Connect to Vendor protocol through the Service.
    zx::result connect_result = driver_test().Connect<fhbt::Service::Vendor>();
    ASSERT_EQ(ZX_OK, connect_result.status_value());

    fidl::ClientEnd<fhbt::HciTransport> hci_transport_end(connect_result.value().TakeChannel());
    hci_transport_client_.Bind(std::move(hci_transport_end));
  }

  void OpenHciTransportClient() {
    auto result = vendor_client_->OpenHciTransport();
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_error());

    auto response = *result.value();
    hci_transport_client_.Bind(std::move(response->channel));
  }

  const fidl::WireSyncClient<fhbt::Vendor>& vendor_client() { return vendor_client_; }
  const fidl::WireSyncClient<fhbt::HciTransport>& hci_transport_client() {
    return hci_transport_client_;
  }

  uint64_t GetCoreDumpCount() {
    return driver_test().RunInDriverContext<uint64_t>([](BtHciBroadcom& driver) {
      auto vmo = driver.inspector().inspector().DuplicateVmo();
      auto hierarchy_res = inspect::ReadFromVmo(std::move(vmo));
      if (hierarchy_res.is_error())
        return static_cast<uint64_t>(0);
      auto hierarchy = std::move(hierarchy_res.value());
      const auto* prop =
          hierarchy.node().get_property<inspect::UintPropertyValue>("core_dump_count");
      return prop ? prop->value() : static_cast<uint64_t>(0);
    });
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;

  fidl::WireSyncClient<fhbt::Vendor> vendor_client_;
  fidl::WireSyncClient<fhbt::HciTransport> hci_transport_client_;

  bool enable_suspend_ = false;
};

class BtHciBroadcomInitializedTest : public BtHciBroadcomTest {
 public:
  void SetUp() override { SetUp(/* enable_suspend=*/false); }
  void SetUp(bool enable_suspend) {
    BtHciBroadcomTest::SetUp(enable_suspend);
    SetFirmware();
    SetMacAddressMetadata();
    ASSERT_TRUE(StartDriver().is_ok());
    OpenVendor();
  }
};

class BtHciBroadcomInitializedWithPowerTest : public BtHciBroadcomInitializedTest {
 public:
  void SetUp() override { BtHciBroadcomInitializedTest::SetUp(/* enable_suspend=*/true); }
};

TEST_F(BtHciBroadcomInitializedTest, Lifecycle) {}

TEST_F(BtHciBroadcomInitializedTest, OpenSnoop) {
  ::fidl::WireResult<::fuchsia_hardware_bluetooth::Vendor::OpenSnoop> result =
      vendor_client()->OpenSnoop();
  ASSERT_TRUE(result.ok());
  ASSERT_FALSE(result->is_error());
}

TEST_F(BtHciBroadcomInitializedTest, GetCrashParameters) {
  auto result = vendor_client()->GetCrashParameters();
  ASSERT_TRUE(result.ok());
  ASSERT_FALSE(result->is_error());

  auto params = result->value();
  ASSERT_TRUE(params->has_crash_events());
  EXPECT_EQ(params->crash_events().size(), 1u);
  EXPECT_EQ(params->crash_events()[0].size(), 2u);
  EXPECT_EQ(params->crash_events()[0][0], 0x1B);
  EXPECT_EQ(params->crash_events()[0][1], 0x03);

  ASSERT_TRUE(params->has_program_name());
  EXPECT_EQ(params->program_name().get(), std::string_view("bt-hci-broadcom"));

  ASSERT_TRUE(params->has_crash_signature());
  EXPECT_EQ(params->crash_signature().get(), std::string_view("bt-hci-broadcom-core-dump"));
}

TEST_F(BtHciBroadcomInitializedTest, HciTransportOpenTwice) {
  // Should be able to open two copies of HciTransport.
  auto result = vendor_client()->OpenHciTransport();
  ASSERT_TRUE(result.ok());
  ASSERT_FALSE(result->is_error());

  auto result_second = vendor_client()->OpenHciTransport();
  ASSERT_TRUE(result_second.ok());
  ASSERT_FALSE(result_second->is_error());
}

TEST_F(BtHciBroadcomTest, ReportLoadFirmwareError) {
  // Ensure reading metadata succeeds.
  SetMacAddressMetadata();

  // No firmware has been set, so load_firmware() should fail during initialization.
  ASSERT_EQ(StartDriver().status_value(), ZX_ERR_NOT_FOUND);
}

TEST_F(BtHciBroadcomTest, TooSmallFirmwareBuffer) {
  // Ensure reading metadata succeeds.
  SetMacAddressMetadata();

  SetFirmware(std::vector<uint8_t>{0x00});
  ASSERT_EQ(StartDriver().status_value(), ZX_ERR_INTERNAL);
}

TEST_F(BtHciBroadcomTest, ControllerReturnsEventSmallerThanEventHeader) {
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    env.transport_device_.SetCustomizedReply(
        std::vector<uint8_t>(kCommandCompleteEvent.data(), kCommandCompleteEvent.data() + 1));
  });

  SetFirmware();
  SetMacAddressMetadata();
  ASSERT_NE(StartDriver().status_value(), ZX_OK);
}

TEST_F(BtHciBroadcomTest, ControllerReturnsEventSmallerThanCommandComplete) {
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    env.transport_device_.SetCustomizedReply(std::vector<uint8_t>(
        kCommandCompleteEvent.data(),
        kCommandCompleteEvent.data() + pw::bluetooth::emboss::EventHeader::MaxSizeInBytes()));
  });

  SetFirmware();
  SetMacAddressMetadata();
  ASSERT_FALSE(StartDriver().is_ok());
}

TEST_F(BtHciBroadcomTest, ControllerFailsToInitializeWhenMissingBdAddr) {
  // Don't set mac address metadata causing an initialization failure on the driver.
  //  Respond to ReadBdaddr command with a command complete (which doesn't include the bdaddr).
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    env.transport_device_.SetCustomizedReply(std::vector<uint8_t>(
        kCommandCompleteEvent.data(), kCommandCompleteEvent.data() + kCommandCompleteEvent.size()));
  });

  // Ensure loading the firmware succeeds.
  SetFirmware();

  // Initialization should fail as missing the MAC address is a fatal error.
  ASSERT_TRUE(StartDriver().is_error());
}

// Tests that the driver sends the vendor-specific baud rate setup command during initialization.
TEST_F(BtHciBroadcomTest, SendsSetBaudRateDuringInitialization) {
  SetMacAddressMetadata();
  SetFirmware();

  ASSERT_TRUE(StartDriver().is_ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    auto packet = env.transport_device_.LastPacketByOpCode(
        static_cast<uint16_t>(BroadcomOpCode::SET_BAUD_RATE));
    ASSERT_TRUE(packet.has_value());

    auto view = MakeSetBaudRateCommandView(packet->data(), packet->size());
    ASSERT_TRUE(view.Ok());
    ASSERT_EQ(view.unused().Read(), 0);
    ASSERT_EQ(view.baud_rate().Read(), kTargetBaudRate);
  });
}

TEST_F(BtHciBroadcomTest, SendsPowerCapWhenNeeded) {
  SetMacAddressMetadata();
  //  Respond to SetInfo command with a controller needing PowerCap
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.transport_device_.SetSerialPid(PDEV_PID_BCM4381A1); });
  // Ensure loading the firmware succeeds.
  SetFirmware();

  // Initialization should succeed
  ASSERT_TRUE(StartDriver().is_ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    auto packet = env.transport_device_.LastPacketByOpCode(
        static_cast<uint16_t>(BroadcomOpCode::SET_POWER_CAP));
    ASSERT_TRUE(packet.has_value());

    const std::vector<uint8_t> kExpectedBytes = {
        0x00, 0xFF, 0x0F,             // Header: opcode 0xFF00, param size 15
        0x01,                         // sub_opcode
        0x02, 0x00,                   // cmd_format_opcode (little-endian)
        72,   60,   28,               // chain_0 limits
        72,   60,   28,               // chain_1 limits
        72,   60,   28,   72, 60, 28  // beamforming_cap
    };
    EXPECT_EQ(*packet, kExpectedBytes);
  });
}

TEST_F(BtHciBroadcomTest, EnablesLowPowerMode) {
  SetMacAddressMetadata();
  //  Respond to SetInfo command with a controller where LowPowerMode is enabled
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.transport_device_.SetSerialPid(PDEV_PID_BCM4381A1); });
  // Ensure loading the firmware succeeds.
  SetFirmware();

  // Initialization should succeed
  ASSERT_TRUE(StartDriver().is_ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    auto packet = env.transport_device_.LastPacketByOpCode(
        static_cast<uint16_t>(BroadcomOpCode::WRITE_SLEEP_MODE));
    ASSERT_TRUE(packet.has_value());
    // We should have calculated the sleep ticks correctly - this is 62.5ms for the device
    // and 12.5ms for the host in 12.5ms increments.
    auto sleep_cmd = MakeWriteSleepModeCmdView(packet->data(), packet->size());
    ASSERT_TRUE(sleep_cmd.Ok());
    ASSERT_EQ(sleep_cmd.IntrinsicSizeInBytes().Read(), 15);
    ASSERT_EQ(sleep_cmd.parameter_size().Read(), 12);
    ASSERT_EQ(sleep_cmd.mode().Read(), SleepMode::UART);
    ASSERT_EQ(sleep_cmd.idle_threshold_device().Read(), 5);
    ASSERT_EQ(sleep_cmd.idle_threshold_host().Read(), 1);
  });
}

TEST_F(BtHciBroadcomTest, FastDownloadSupportedChipId) {
  SetMacAddressMetadata();
  SetFirmware();
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.transport_device_.SetChipId(kFastDownloadChipId); });
  ASSERT_TRUE(StartDriver().is_ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    ASSERT_TRUE(env.transport_device_.HasReceivedOpCode(
        static_cast<uint16_t>(BroadcomOpCode::SET_DOWNLOAD_CONFIG)));
    auto fw_packet = env.transport_device_.LastPacketByOpCode(kTestFirmwareOpCode);
    ASSERT_TRUE(fw_packet.has_value());
    EXPECT_EQ(*fw_packet, kFirmware);
  });
}

TEST_F(BtHciBroadcomTest, FastDownloadNotSupportedChipId) {
  SetMacAddressMetadata();
  SetFirmware();
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.transport_device_.SetChipId(kNoFastDownloadChipId); });
  ASSERT_TRUE(StartDriver().is_ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    ASSERT_FALSE(env.transport_device_.HasReceivedOpCode(
        static_cast<uint16_t>(BroadcomOpCode::SET_DOWNLOAD_CONFIG)));
    auto fw_packet = env.transport_device_.LastPacketByOpCode(kTestFirmwareOpCode);
    ASSERT_TRUE(fw_packet.has_value());
    EXPECT_EQ(*fw_packet, kFirmware);
  });
}

TEST_F(BtHciBroadcomTest, VendorProtocolUnknownMethod) {
  SetFirmware();
  SetMacAddressMetadata();
  ASSERT_TRUE(StartDriver().is_ok());

  OpenVendorWithHciTransportClient();

  fidl::Arena arena;
  std::vector<uint8_t> packet = {1};
  auto packet_view = fidl::VectorView<uint8_t>::FromExternal(packet);
  auto result = hci_transport_client()->Send(fhbt::wire::SentPacket::WithAcl(arena, packet_view));

  ASSERT_EQ(result.status(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(BtHciBroadcomInitializedTest, EncodeSetAclPrioritySuccessWithParametersHighSink) {
  std::array<uint8_t, SetAclPriorityCommand::MaxSizeInBytes()> result_buffer;
  fidl::Arena arena;
  auto builder = fhbt::wire::VendorSetAclPriorityParams::Builder(arena);
  builder.connection_handle(0xFF00);
  builder.priority(fhbt::wire::VendorAclPriority::kHigh);
  builder.direction(fhbt::wire::VendorAclDirection::kSink);

  auto command = fhbt::wire::VendorCommand::WithSetAclPriority(arena, builder.Build());
  auto result = vendor_client()->EncodeCommand(command);
  ASSERT_TRUE(result.ok());
  ASSERT_FALSE(result->is_error());

  std::copy(result->value()->encoded.begin(), result->value()->encoded.end(),
            result_buffer.begin());
  const std::array<uint8_t, SetAclPriorityCommand::MaxSizeInBytes()> kExpectedBuffer = {
      0x1A,
      0xFD,  // OpCode
      0x04,  // size
      0x00,
      0xFF,  // handle
      0x01,  // priority (High)
      0x01,  // direction (Sink)
  };
  EXPECT_EQ(result_buffer, kExpectedBuffer);
}

TEST_F(BtHciBroadcomInitializedTest, EncodeSetAclPrioritySuccessWithParametersNormalSource) {
  std::array<uint8_t, SetAclPriorityCommand::MaxSizeInBytes()> result_buffer;
  fidl::Arena arena;
  auto builder = fhbt::wire::VendorSetAclPriorityParams::Builder(arena);
  builder.connection_handle(0xFF00);
  builder.priority(fhbt::wire::VendorAclPriority::kNormal);
  builder.direction(fhbt::wire::VendorAclDirection::kSource);

  auto command = fhbt::wire::VendorCommand::WithSetAclPriority(arena, builder.Build());
  auto result = vendor_client()->EncodeCommand(command);
  ASSERT_TRUE(result.ok());
  ASSERT_FALSE(result->is_error());

  std::copy(result->value()->encoded.begin(), result->value()->encoded.end(),
            result_buffer.begin());
  const std::array<uint8_t, SetAclPriorityCommand::MaxSizeInBytes()> kExpectedBuffer = {
      0x1A,
      0xFD,  // OpCode
      0x04,  // size
      0x00,
      0xFF,  // handle
      0x00,  // priority (Normal)
      0x00,  // direction (Source)
  };
  EXPECT_EQ(result_buffer, kExpectedBuffer);
}

TEST_F(BtHciBroadcomInitializedTest, HciTransportPassthrough) {
  OpenHciTransportClient();

  const std::vector<uint8_t> kExpectedBuffer = {
      0x07,
      0x05,  // OpCode
      0x03,  // size
      0x00,
      0x00,  // Handle (ignored)
      0x00,  // Clock (own clock)
  };

  const std::vector<uint8_t> kExpectedResponse = {
      0x0E,                    // Cmd Complete
      0x0B,                    // 12 bytes
      0x05,                    // HCI Command Packets
      0x05, 0x07,              // Opcode
      0x00,                    // Success
      0x00, 0x00,              // Handle (reserved)
      0x12, 0x34, 0x56, 0x78,  // Clock value
      0x00, 0x00,              // Accuracy
  };

  driver_test().RunInEnvironmentTypeContext(
      [&](TestEnvironment& env) { env.transport_device_.SetCustomizedReply(kExpectedResponse); });

  fidl::Arena arena;
  auto result =
      hci_transport_client()->Send(fhbt::wire::SentPacket::WithCommand(arena, kExpectedBuffer));

  ASSERT_EQ(result.status(), ZX_OK);

  driver_test().RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    auto packet = env.transport_device_.LastPacketByOpCode(0x0507);
    ASSERT_TRUE(packet.has_value());
    EXPECT_EQ(packet, kExpectedBuffer);
  });

  class EventHandler final : public fidl::WireSyncEventHandler<fhbt::HciTransport> {
   public:
    EventHandler() = default;

    void SetExpected(const std::vector<uint8_t>& expected) { expected_ = expected; }

    void OnReceive(fidl::WireEvent<fhbt::HciTransport::OnReceive>* event) override {
      auto response = event->event();
      // Should have relayed the response from the underlying transport.
      std::vector<uint8_t> data(response.begin(), response.end());
      EXPECT_EQ(data, expected_);
    }

    void handle_unknown_event(fidl::UnknownEventMetadata<fhbt::HciTransport> metadata) override {
      ASSERT_TRUE(false);
    }

   private:
    std::vector<uint8_t> expected_;
  };

  EventHandler event_handler;
  event_handler.SetExpected(kExpectedResponse);
  fidl::Status status = hci_transport_client().HandleOneEvent(event_handler);
  EXPECT_TRUE(status.ok());
}

TEST_F(BtHciBroadcomInitializedTest, HciTransportPassthroughCoreDumpCooldown) {
  OpenHciTransportClient();

  EXPECT_EQ(GetCoreDumpCount(), 0ull);

  const std::vector<uint8_t> kCoreDumpEvent = {0xFF, 0x02, 0x1B, 0x03};

  driver_test().RunInEnvironmentTypeContext(
      [&](TestEnvironment& env) { env.transport_device_.SendEvent(kCoreDumpEvent); });

  NoOpEventHandler event_handler;
  // Wait for the event to be forwarded to ensure the background driver thread has finished
  // processing it before checking Inspect metrics.
  fidl::Status status = hci_transport_client().HandleOneEvent(event_handler);
  EXPECT_TRUE(status.ok());

  EXPECT_EQ(GetCoreDumpCount(), 1ull);

  // Send another dump event.
  driver_test().RunInEnvironmentTypeContext(
      [&](TestEnvironment& env) { env.transport_device_.SendEvent(kCoreDumpEvent); });

  status = hci_transport_client().HandleOneEvent(event_handler);
  EXPECT_TRUE(status.ok());

  EXPECT_EQ(GetCoreDumpCount(), 1ull);  // Cooldown! Still 1.

  // Advance time past the default cooldown (20 minutes) in the driver context.
  driver_test().RunInDriverContext<void>(
      [this](BtHciBroadcom& driver) { RunLoopFor(zx::min(21)); });

  // Send another dump event.
  driver_test().RunInEnvironmentTypeContext(
      [&](TestEnvironment& env) { env.transport_device_.SendEvent(kCoreDumpEvent); });

  status = hci_transport_client().HandleOneEvent(event_handler);
  EXPECT_TRUE(status.ok());

  EXPECT_EQ(GetCoreDumpCount(), 2ull);  // Cooldown expired! Now 2.
}

TEST_F(BtHciBroadcomInitializedWithPowerTest, InitPowerManagement) {
  // Should have acquired a Boot lease as part of startup
  std::optional<uint8_t> lease_power_level = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::optional<uint8_t>(TestEnvironment&)>(
          [](TestEnvironment& env) { return env.fake_power_broker().lease_power_level(); }));
  ASSERT_TRUE(lease_power_level);
  EXPECT_EQ(*lease_power_level, BtHciBroadcom::kBoot);

  // But after startup firmware load, the lease should be dropped already.
  fidl::ServerEnd<fuchsia_power_broker::LeaseControl> lease_control_server_end =
      driver_test().RunInEnvironmentTypeContext(
          fit::callback<fidl::ServerEnd<fuchsia_power_broker::LeaseControl>(TestEnvironment&)>(
              [](TestEnvironment& env) {
                return env.fake_power_broker().TakeLeaseControlServerEnd();
              }));
  EXPECT_TRUE(lease_control_server_end.is_valid());

  zx_signals_t observed{};
  EXPECT_EQ(lease_control_server_end.channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                        zx::time::infinite_past(), &observed),
            ZX_OK);
  EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);

  // SetLevel should be kOff, and respond as fine.
  // Do the initial SetLevel call to make sure that the element responds.
  fidl::ClientEnd element_runner_client_end = driver_test().RunInEnvironmentTypeContext(
      fit::callback<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>(TestEnvironment&)>(
          [](TestEnvironment& env) {
            return env.fake_power_broker().TakeElementRunnerClientEnd();
          }));
  fidl::Client<fuchsia_power_broker::ElementRunner> element_runner(
      std::move(element_runner_client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  element_runner->SetLevel(BtHciBroadcom::kOff)
      .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        if (result.is_error()) {
          fdf::warn("Result: {}", result.error_value().status_string());
        }
        EXPECT_TRUE(result.is_ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();
  driver_test().runtime().ResetQuit();
}

TEST_F(BtHciBroadcomInitializedWithPowerTest, ActivityAcquiresAndExtendsLease) {
  // Should have acquired a Boot lease as part of startup
  std::optional<uint8_t> lease_power_level = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::optional<uint8_t>(TestEnvironment&)>(
          [](TestEnvironment& env) { return env.fake_power_broker().lease_power_level(); }));
  ASSERT_TRUE(lease_power_level);
  EXPECT_EQ(*lease_power_level, BtHciBroadcom::kBoot);

  fdf::info("Checking that boot lease has been dropped");
  // But after startup firmware load, the lease should be dropped already.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.fake_power_broker().ExpectLeaseReleased(); });

  OpenHciTransportClient();
  fdf::info("Sending a packet through, should get a power lease");

  fidl::Arena arena;
  auto result = hci_transport_client()->Send(
      fhbt::wire::SentPacket::WithCommand(arena, std::vector<uint8_t>{0x1A, 0xFD}));

  ASSERT_EQ(result.status(), ZX_OK);

  fdf::info("waiting for power lease");
  // Should acquire an On lease
  driver_test().runtime().RunUntil([&]() {
    lease_power_level = driver_test().RunInEnvironmentTypeContext(
        fit::callback<std::optional<uint8_t>(TestEnvironment&)>(
            [](TestEnvironment& env) { return env.fake_power_broker().lease_power_level(); }));
    return lease_power_level.has_value();
  });

  EXPECT_EQ(*lease_power_level, BtHciBroadcom::kOn);

  // Get the lease control server end to monitor closure.
  auto lease_control_server_end = driver_test().RunInEnvironmentTypeContext(
      fit::callback<fidl::ServerEnd<fuchsia_power_broker::LeaseControl>(TestEnvironment&)>(
          [](TestEnvironment& env) {
            return env.fake_power_broker().TakeLeaseControlServerEnd();
          }));
  EXPECT_TRUE(lease_control_server_end.is_valid());

  // Wait for some time less than timeout (timeout is 2 * kDefaultHostIdleThreshold).
  // Let's wait kDefaultHostIdleThreshold.
  driver_test().RunInDriverContext<void>(
      [this](BtHciBroadcom& driver) { RunLoopFor(kDefaultHostIdleThreshold); });

  // Verify lease is STILL ACTIVE (not closed).
  bool closed = driver_test().RunInEnvironmentTypeContext<bool>([&](TestEnvironment& env) {
    zx_signals_t observed{};
    auto wait_result = lease_control_server_end.channel().wait_one(
        ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &observed);
    return (wait_result == ZX_OK && (observed & ZX_CHANNEL_PEER_CLOSED));
  });
  EXPECT_FALSE(closed);

  fdf::info("Sending another packet to extend lease");
  auto result2 = hci_transport_client()->Send(
      fhbt::wire::SentPacket::WithCommand(arena, std::vector<uint8_t>{0x1A, 0xFD}));
  ASSERT_EQ(result2.status(), ZX_OK);

  // Wait another kDefaultHostIdleThreshold. Total time since first packet is 2 *
  // kDefaultHostIdleThreshold. Total time since second packet is kDefaultHostIdleThreshold (should
  // not expire).
  driver_test().RunInDriverContext<void>(
      [this](BtHciBroadcom& driver) { RunLoopFor(kDefaultHostIdleThreshold); });

  // Verify lease is STILL ACTIVE.
  closed = driver_test().RunInEnvironmentTypeContext<bool>([&](TestEnvironment& env) {
    zx_signals_t observed{};
    auto wait_result = lease_control_server_end.channel().wait_one(
        ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &observed);
    return (wait_result == ZX_OK && (observed & ZX_CHANNEL_PEER_CLOSED));
  });
  EXPECT_FALSE(closed);

  // Wait another 2 * kDefaultHostIdleThreshold. Total time since second packet is 3 *
  // kDefaultHostIdleThreshold (should expire).
  driver_test().RunInDriverContext<void>(
      [this](BtHciBroadcom& driver) { RunLoopFor(2 * kDefaultHostIdleThreshold); });

  // Verify lease IS DROPPED.
  driver_test().runtime().RunUntil([&]() {
    return !driver_test().RunInEnvironmentTypeContext<bool>([&](TestEnvironment& env) {
      zx_signals_t observed{};
      auto wait_result = lease_control_server_end.channel().wait_one(
          ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &observed);
      if (wait_result == ZX_ERR_TIMED_OUT) {
        return false;
      }
      EXPECT_EQ(wait_result, ZX_OK);
      return (observed & ZX_CHANNEL_PEER_CLOSED) != 0;
    });
  });
}

TEST_F(BtHciBroadcomInitializedWithPowerTest, LeasePendingVeryLong) {
  // Should have acquired a Boot lease as part of startup
  std::optional<uint8_t> lease_power_level = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::optional<uint8_t>(TestEnvironment&)>(
          [](TestEnvironment& env) { return env.fake_power_broker().lease_power_level(); }));
  ASSERT_TRUE(lease_power_level);
  EXPECT_EQ(*lease_power_level, BtHciBroadcom::kBoot);

  fdf::info("Checking that boot lease has been dropped");
  // But after startup firmware load, the lease should be dropped already.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.fake_power_broker().ExpectLeaseReleased(); });

  OpenHciTransportClient();
  fdf::info("Sending a packet through, should get a power lease");

  fidl::Arena arena;
  auto result = hci_transport_client()->Send(
      fhbt::wire::SentPacket::WithCommand(arena, std::vector<uint8_t>{0x1A, 0xFD}));

  ASSERT_EQ(result.status(), ZX_OK);

  // Wait for the lease request to be processed by FakePowerBroker.
  driver_test().runtime().RunUntil([&]() {
    return driver_test().RunInEnvironmentTypeContext<bool>([](TestEnvironment& env) {
      return env.fake_power_broker().lease_power_level().has_value();
    });
  });

  // Wait for longer than the timeout (2 * kDefaultHostIdleThreshold).
  // Let's wait 3 * kDefaultHostIdleThreshold.
  // The lease should NOT be dropped yet because it's pending.
  driver_test().RunInDriverContext<void>(
      [this](BtHciBroadcom& driver) { RunLoopFor(3 * kDefaultHostIdleThreshold); });

  // Verify lease is STILL ACTIVE (not closed).
  bool closed = driver_test().RunInEnvironmentTypeContext<bool>(
      [](TestEnvironment& env) { return env.fake_power_broker().IsLeaseControlClosed(); });
  EXPECT_FALSE(closed);

  // Verify that WatchStatus hasn't been satisfied yet (it should be readable).
  bool readable = driver_test().RunInEnvironmentTypeContext<bool>(
      [](TestEnvironment& env) { return env.fake_power_broker().IsLeaseControlReadable(); });
  EXPECT_TRUE(readable);

  fdf::info("Satisfying lease after timeout");
  // Now satisfy the lease.
  driver_test().RunInEnvironmentTypeContext(
      [](TestEnvironment& env) { env.fake_power_broker().SatisfyLease(); });

  // Now it should be satisfied, and the driver should schedule a drop.
  // Wait for it to be dropped (binding removed).
  driver_test().runtime().RunUntil([&]() {
    return driver_test().RunInEnvironmentTypeContext<bool>(
        [](TestEnvironment& env) { return !env.fake_power_broker().IsLeaseBound(); });
  });
}

}  // namespace

}  // namespace bt_hci_broadcom
