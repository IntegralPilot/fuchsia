// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_BROADCOM_BT_HCI_BROADCOM_H_
#define SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_BROADCOM_BT_HCI_BROADCOM_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.bluetooth/cpp/fidl.h>
#include <fidl/fuchsia.hardware.bluetooth/cpp/wire.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/fidl.h>
#include <fidl/fuchsia.io/cpp/wire_test_base.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/power/cpp/element-description-builder.h>
#include <lib/driver/power/cpp/power-support.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>

namespace bt_hci_broadcom {

constexpr uint32_t kTargetBaudRate = 2000000;
constexpr uint32_t kDefaultBaudRate = 115200;

class HciEventHandler
    : public fidl::WireSyncEventHandler<fuchsia_hardware_bluetooth::HciTransport> {
 public:
  explicit HciEventHandler(fit::function<void(std::vector<uint8_t>&)> on_receive_callback);

  void OnReceive(fuchsia_hardware_bluetooth::wire::ReceivedPacket*) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata) override {}

 private:
  fit::function<void(std::vector<uint8_t>&)> on_receive_callback_;
};

enum class ActivityType : uint8_t {
  kSendPacket,
  kReceivePacket,
};

class BtHciBroadcom final : public fdf::DriverBase2,
                            public fidl::WireServer<fuchsia_power_broker::ElementRunner>,
                            public fidl::WireServer<fuchsia_hardware_bluetooth::Vendor> {
 public:
  inspect::ComponentInspector& inspector() { return *component_inspector_; }

  BtHciBroadcom();

  void set_test_dispatcher(async_dispatcher_t* dispatcher) { dispatcher_ = dispatcher; }

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;

  enum PowerLevel : uint8_t {
    kOff = 0,
    kBoot,
    kOn,
    kPowerLevelCount,
  };

 private:
  static constexpr size_t kMacAddrLen = 6;
  static constexpr zx::duration kDefaultHostIdleThreshold = zx::usec(12500);
  static constexpr zx::duration kDefaultDevIdleThreshold = zx::usec(62500);

  static const std::unordered_map<uint16_t, std::string> kFirmwareMap;

  // fuchsia_hardware_bluetooth::Vendor protocol interface implementations.
  void GetFeatures(GetFeaturesCompleter::Sync& completer) override;
  void EncodeCommand(EncodeCommandRequestView request,
                     EncodeCommandCompleter::Sync& completer) override;
  void OpenHci(OpenHciCompleter::Sync& completer) override;
  void OpenHciTransport(OpenHciTransportCompleter::Sync& completer) override;
  void OpenSnoop(OpenSnoopCompleter::Sync& completer) override;
  void GetCrashParameters(GetCrashParametersCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::Vendor> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fuchsia.power.broker/ElementRunner implementation
  void SetLevel(fuchsia_power_broker::wire::ElementRunnerSetLevelRequest* request,
                SetLevelCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Truly private, internal helper methods:
  zx_status_t ConnectToHciTransportFidlProtocol();
  zx_status_t ConnectToSerialFidlProtocol();

  static void EncodeSetAclPriorityCommand(
      fuchsia_hardware_bluetooth::wire::VendorSetAclPriorityParams params, void* out_buffer);

  void OnReceivePacket(std::vector<uint8_t>& packet);
  template <typename CmdView>
  fpromise::promise<std::vector<uint8_t>, zx_status_t> SendCommand(CmdView view);

  fpromise::promise<std::vector<uint8_t>, zx_status_t> SendHciReset();
  fpromise::promise<std::vector<uint8_t>, zx_status_t> SendCommand(const void* command,
                                                                   size_t length);

  // Waits for the next event from |hci_transport_client_|'s fidl channel.
  fpromise::promise<std::vector<uint8_t>, zx_status_t> ReadEvent();

  fpromise::promise<void, zx_status_t> SetBaudRate(uint32_t baud_rate);

  fpromise::promise<void, zx_status_t> SetBdaddr(const std::array<uint8_t, kMacAddrLen>& bdaddr);

  fpromise::promise<void, zx_status_t> SetDefaultPowerCaps();

  // Enable Low Power Mode
  // The controller may sleep after host_idle_threshold duration passes with no
  // outgoing data and device_idle_threshold with no incoming data.
  // Wake pins can prevent this.
  fpromise::promise<void, zx_status_t> EnableLowPowerMode(zx::duration host_idle_threshold,
                                                          zx::duration device_idle_threshold);

  fpromise::promise<void, zx_status_t> DisableLowPowerMode();

  zx::result<> InitPowerManagement();
  zx::result<fdf_power::ElementDesc> ApplyPowerConfiguration(
      std::vector<fdf_power::PowerElementConfiguration> element_configs);
  fpromise::promise<void, zx_status_t> AssertLevel(PowerLevel requested_level);
  fpromise::promise<void, zx_status_t> AcquirePowerElementLease();
  void HandleWakeLeaseTimeout();

  // Called when there is some transport activity, used to manage the transport and controller power
  // states.
  void NoteActivity(ActivityType activity);

  // Called when a core dump HCI event is received.
  void NoteCoreDump();

  fpromise::promise<void, zx_status_t> LoadFirmware(bool fast_download);

  // Adds a client which passes through the given client, managing power states.
  fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport> AddHciTransportClient(
      fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport> upstream_client_end);

  // Used by firmware loading
  zx_status_t SendCommandSync(const void* command, size_t length);
  zx_status_t SendCommandWithoutEvent(const void* command, size_t length);
  zx::result<std::vector<uint8_t>> ReadEventSync();

  zx_status_t SendVmoAsCommands(zx::vmo vmo, size_t size, bool fast_download);

  fpromise::promise<void, zx_status_t> Initialize();

  fpromise::promise<void, zx_status_t> OnInitializeComplete(zx_status_t status);

  void CompleteStart(zx_status_t status);

  zx_status_t Bind();

  uint32_t serial_pid_;
  // true if underlying transport is UART
  bool is_uart_ = false;

  std::optional<fdf::StartCompleter> start_completer_;

  std::optional<async::Executor> executor_;

  std::vector<uint8_t> event_receive_buffer_;
  HciEventHandler hci_event_handler_;
  fidl::WireSyncClient<fuchsia_hardware_bluetooth::HciTransport> hci_transport_client_;
  fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport> hci_transport_client_end_;

  std::vector<fidl::ServerBindingRef<fuchsia_hardware_bluetooth::HciTransport>> active_clients_;

  async_dispatcher_t* dispatcher_ = nullptr;

  fdf::WireSyncClient<fuchsia_hardware_serialimpl::Device> serial_client_;

  std::shared_ptr<fdf::Namespace> incoming_;

  PowerLevel power_level_ = PowerLevel::kBoot;
  async::TaskClosureMethod<BtHciBroadcom, &BtHciBroadcom::HandleWakeLeaseTimeout> drop_level_task_{
      this};

  zx::event assertive_token_;
  fidl::ClientEnd<fuchsia_power_broker::ElementControl> element_control_client_end_;
  fidl::ClientEnd<fuchsia_power_broker::Lessor> element_lessor_client_;
  std::optional<fidl::ServerBinding<fuchsia_power_broker::ElementRunner>>
      element_runner_server_binding_;

  // A lease on our power element is kept when we are asserting our level.
  // It's kept at Boot level until we are done initializing the controller, then
  // asserted at On when we are transmitting or expecting to receive.
  std::optional<fidl::WireClient<fuchsia_power_broker::LeaseControl>> level_lease_client_;

  std::optional<inspect::ComponentInspector> component_inspector_;
  inspect::UintProperty core_dump_count_;
  std::optional<zx::time> last_core_dump_time_;

  fidl::ServerBindingGroup<fuchsia_hardware_bluetooth::Vendor> vendor_binding_group_;
};

}  // namespace bt_hci_broadcom

#endif  // SRC_CONNECTIVITY_BLUETOOTH_HCI_VENDOR_BROADCOM_BT_HCI_BROADCOM_H_
