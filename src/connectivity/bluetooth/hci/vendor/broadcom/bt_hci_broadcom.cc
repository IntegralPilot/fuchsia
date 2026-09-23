// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bt_hci_broadcom.h"

#include <assert.h>
#include <endian.h>
#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <inttypes.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/time.h>
#include <lib/async/default.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include "fidl/fuchsia.hardware.bluetooth/cpp/markers.h"
#include "lib/fidl/cpp/channel.h"
#include "lib/fit/function.h"
#include "lib/fpromise/promise.h"
#include "src/connectivity/bluetooth/hci/vendor/broadcom/bt_hci_broadcom_config.h"
#include "src/connectivity/bluetooth/hci/vendor/broadcom/packets.emb.h"
#include "tools/power_config/lib/cpp/power_config.h"

#include <pw_bluetooth/hci_events.emb.h>

namespace bt_hci_broadcom {
namespace {

constexpr size_t kMaxCommandPacketPayloadSize = 255;
constexpr size_t kMaxHciCommandSize =
    CommandHeader::IntrinsicSizeInBytes() + kMaxCommandPacketPayloadSize;

constexpr uint8_t kDefaultBrPowerCap = 72;
constexpr uint8_t kDefaultEdrPowerCap = 60;
constexpr uint8_t kDefaultBlePowerCap = 28;

template <typename Container>
SetPowerCapCommandView MakeDefaultPowerCapCommand(Container* container) {
  auto view = MakeSetPowerCapCommandView(container);
  ZX_ASSERT(view.IsComplete());
  view.header().opcode().Write(BroadcomOpCode::SET_POWER_CAP);
  view.header().parameter_total_size().Write(SetPowerCapCommand::parameter_size());
  view.sub_opcode().Write(SetPowerCapSubOpCode::SET);
  view.cmd_format_opcode().Write(SetPowerCapCmdFormatOpCode::FORMAT_2);
  view.chain_0_power_limit_br().Write(kDefaultBrPowerCap);
  view.chain_0_power_limit_edr().Write(kDefaultEdrPowerCap);
  view.chain_0_power_limit_ble().Write(kDefaultBlePowerCap);
  view.chain_1_power_limit_br().Write(kDefaultBrPowerCap);
  view.chain_1_power_limit_edr().Write(kDefaultEdrPowerCap);
  view.chain_1_power_limit_ble().Write(kDefaultBlePowerCap);
  view.beamforming_cap()[0].Write(kDefaultBrPowerCap);
  view.beamforming_cap()[1].Write(kDefaultEdrPowerCap);
  view.beamforming_cap()[2].Write(kDefaultBlePowerCap);
  view.beamforming_cap()[3].Write(kDefaultBrPowerCap);
  view.beamforming_cap()[4].Write(kDefaultEdrPowerCap);
  view.beamforming_cap()[5].Write(kDefaultBlePowerCap);
  ZX_ASSERT(view.Ok());
  return view;
}

template <typename Container>
WriteSleepModeCmdView DisableLowPowerModeCmd(Container* container) {
  auto view = MakeWriteSleepModeCmdView(container);
  ZX_ASSERT(view.IsComplete());
  view.header().opcode().Write(BroadcomOpCode::WRITE_SLEEP_MODE);
  view.header().parameter_total_size().Write(WriteSleepModeCmd::parameter_size());
  view.mode().Write(SleepMode::DISABLED);
  view.idle_threshold_host().Write(0);
  view.idle_threshold_device().Write(0);
  view.bt_wake_polarity().Write(0);
  view.host_wake_polarity().Write(0);
  view.sleep_during_sco().Write(0);
  view.combine_sleep_and_lpm().Write(0);
  view.tri_state_uart_before_sleep().Write(0);
  for (auto usb_flag : view.usb_flags()) {
    usb_flag.Write(0);
  }
  view.pulsed_host_wake().Write(0);
  ZX_ASSERT(view.Ok());
  return view;
}

template <typename Container>
WriteSleepModeCmdView EnableLowPowerModeCmd(Container* container, zx::duration host_idle_threshold,
                                            zx::duration device_idle_threshold) {
  uint8_t host_idle_units = static_cast<uint8_t>(host_idle_threshold.to_nsecs() / 12500000);
  uint8_t device_idle_units = static_cast<uint8_t>(device_idle_threshold.to_nsecs() / 12500000);
  auto view = MakeWriteSleepModeCmdView(container);
  ZX_ASSERT(view.IsComplete());
  view.header().opcode().Write(BroadcomOpCode::WRITE_SLEEP_MODE);
  view.header().parameter_total_size().Write(WriteSleepModeCmd::parameter_size());
  view.mode().Write(SleepMode::UART);
  view.idle_threshold_host().Write(host_idle_units);
  view.idle_threshold_device().Write(device_idle_units);
  view.bt_wake_polarity().Write(1);
  view.host_wake_polarity().Write(1);
  view.sleep_during_sco().Write(1);
  view.combine_sleep_and_lpm().Write(1);
  view.tri_state_uart_before_sleep().Write(0);
  for (auto usb_flag : view.usb_flags()) {
    usb_flag.Write(0);
  }
  view.pulsed_host_wake().Write(0);
  ZX_ASSERT(view.Ok());
  return view;
}

namespace fhbt = fuchsia_hardware_bluetooth;
namespace fhsi = fuchsia_hardware_serialimpl::wire;

// Chips with a chip ID greater than or equal to this value support the "Fast Download"
// feature for firmware loading.
constexpr uint8_t kFastDownloadChipIdMin = 174;

constexpr zx::duration kFirmwareDownloadDelay = zx::msec(50);

// Hardcoded. Better to parameterize on chipset. Broadcom chips need a few hundred msec delay after
// firmware load.
constexpr zx::duration kBaudRateSwitchDelay = zx::msec(200);

constexpr zx::duration kCoreDumpCooldown = zx::min(20);

constexpr uint8_t kVendorSpecificEventCode = 0xFF;

// 0x1B = DBFW subevent code, 0x03 = the dump type is "core dump"
constexpr std::array<uint8_t, 2> kCrashVendorSubeventPrefix = {0x1B, 0x03};
constexpr char kCrashProgramName[] = "bt-hci-broadcom";
constexpr char kCrashSignature[] = "bt-hci-broadcom-core-dump";
constexpr char kCoreDumpCountInspectPropertyName[] = "core_dump_count";

}  // namespace

const std::unordered_map<uint16_t, std::string> BtHciBroadcom::kFirmwareMap = {
    {PDEV_PID_BCM43458, "BCM4345C5.hcd"},
    {PDEV_PID_BCM4359, "BCM4359C0.hcd"},
    {PDEV_PID_BCM4381A1, "BCM4381A1.hcd"}};

HciEventHandler::HciEventHandler(fit::function<void(std::vector<uint8_t>&)> on_receive_callback)
    : on_receive_callback_(std::move(on_receive_callback)) {}

void HciEventHandler::OnReceive(fhbt::wire::ReceivedPacket* packet) {
  if (!on_receive_callback_) {
    fdf::error("No receive callback has been set.");
    return;
  }
  // Ignore packets if they are not event packets during initialization.
  if (packet->Which() != fhbt::wire::ReceivedPacket::Tag::kEvent) {
    fdf::error("Received non event packet: {}", static_cast<int>(packet->Which()));
    return;
  }
  std::vector<uint8_t> buffer(packet->event().begin(), packet->event().end());
  on_receive_callback_(buffer);
}

class HciTransportPassthroughImpl : public fidl::Server<fhbt::HciTransport>,
                                    public fidl::AsyncEventHandler<fhbt::HciTransport> {
 public:
  using ActivityCallback = fit::function<void(ActivityType)>;
  using CoreDumpCallback = fit::function<void()>;

  explicit HciTransportPassthroughImpl(fidl::ClientEnd<fhbt::HciTransport> upstream_client_end,
                                       ActivityCallback activity_cb, CoreDumpCallback core_dump_cb,
                                       async_dispatcher_t* dispatcher)
      : activity_cb_(std::move(activity_cb)),
        core_dump_cb_(std::move(core_dump_cb)),
        upstream_client_(std::move(upstream_client_end), dispatcher, this) {}

  static fidl::ServerBindingRef<fhbt::HciTransport> BindServer(
      async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_hardware_bluetooth::HciTransport> server_end,
      fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport> upstream_client_end,
      ActivityCallback activity_cb, CoreDumpCallback core_dump_cb) {
    std::unique_ptr impl = std::make_unique<HciTransportPassthroughImpl>(
        std::move(upstream_client_end), std::move(activity_cb), std::move(core_dump_cb),
        dispatcher);
    HciTransportPassthroughImpl* impl_ptr = impl.get();

    fidl::ServerBindingRef binding_ref =
        fidl::BindServer(dispatcher, std::move(server_end), std::move(impl),
                         std::mem_fn(&HciTransportPassthroughImpl::OnUnbound));
    impl_ptr->binding_ref_.emplace(binding_ref);
    return binding_ref;
  }

  void Send(SendRequest& request, SendCompleter::Sync& completed) override {
    activity_cb_(ActivityType::kSendPacket);
    upstream_client_->Send(request).Then(
        [completer = completed.ToAsync()](auto result) mutable { completer.Reply(); });
  }

  void AckReceive(AckReceiveCompleter::Sync& completer) override {
    auto result = upstream_client_->AckReceive();
    if (result.is_error()) {
      fdf::warn("Failed to ack to upstream");
    }
  }

  void OnReceive(fidl::Event<fhbt::HciTransport::OnReceive>& event) override {
    activity_cb_(ActivityType::kReceivePacket);

    // Check if it is a core dump event.
    if (event.Which() == fhbt::ReceivedPacket::Tag::kEvent) {
      const std::vector<uint8_t>& bytes = event.event().value();
      if (bytes.size() >= 4 && bytes[0] == kVendorSpecificEventCode &&
          bytes[2] == kCrashVendorSubeventPrefix[0] && bytes[3] == kCrashVendorSubeventPrefix[1]) {
        core_dump_cb_();
      }
    }

    if (!binding_ref_.has_value()) {
      fdf::warn("OnReceive with no server?!?");
    }
    fit::result result = fidl::SendEvent(*binding_ref_)->OnReceive(event);
    if (result.is_error()) {
      fdf::warn("Failed to send OnReceive to client");
    }
  }

  void ConfigureSco(ConfigureScoRequest& request, ConfigureScoCompleter::Sync& completer) override {
    auto result = upstream_client_->ConfigureSco(std::move(request));
    if (result.is_error()) {
      fdf::warn("ConfigureSco failed");
    }
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fhbt::HciTransport> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method in HciTransport client, closing with ZX_ERR_NOT_SUPPORTED");
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void handle_unknown_event(fidl::UnknownEventMetadata<fhbt::HciTransport> metadata) override {
    fdf::error("Unknown event in upstream HciTransport protocol, ignoring");
  }

  void OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<fhbt::HciTransport> server_end) {
    if (info.is_user_initiated()) {
      fdf::info("Shutting down HciTransport");
    } else if (info.is_peer_closed()) {
      fdf::info("HciTransport Client closed");
    } else {
      fdf::warn("HciTransport Server error: {}", info.status_string());
    }
    // Upstream client end should be dropped when the server is deallocated.
  }

 private:
  ActivityCallback activity_cb_;
  CoreDumpCallback core_dump_cb_;
  fidl::Client<fhbt::HciTransport> upstream_client_;

  std::optional<fidl::ServerBindingRef<fuchsia_hardware_bluetooth::HciTransport>> binding_ref_;
};

BtHciBroadcom::BtHciBroadcom()
    : DriverBase2("bt-hci-broadcom"),
      hci_event_handler_([this](std::vector<uint8_t>& packet) { OnReceivePacket(packet); }) {}

void BtHciBroadcom::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
  // BT_HOST_WAKE and BT_DEV_WAKE, when they are available, are used to

  dispatcher_ = dispatcher();
  component_inspector_ = context.CreateInspector(this);
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx_status_t status = ConnectToHciTransportFidlProtocol();
  if (status != ZX_OK) {
    completer(zx::error(status));
    return;
  }
  status = ConnectToSerialFidlProtocol();
  if (status == ZX_OK) {
    is_uart_ = true;
  }

  fdf::Arena arena('INFO');
  auto result = serial_client_.buffer(arena)->GetInfo();
  if (!result.ok()) {
    fdf::error("Read failed FIDL error: {}", result.status_string());
    completer(zx::error(result.status()));
    return;
  }

  if (result->is_error()) {
    fdf::error("Read failed : {}", zx_status_get_string(result->error_value()));
    completer(zx::error(result->error_value()));
    return;
  }

  serial_pid_ = result.value()->info.serial_pid;

  if (serial_pid_ == PDEV_PID_BCM4381A1) {
    // BCM4381 board requires flow control by default.
    fdf::Arena config_arena('CONF');
    const uint32_t flags = fhsi::kSerialDataBits8 | fhsi::kSerialStopBits1 |
                           fhsi::kSerialParityNone | fhsi::kSerialFlowCtrlCtsRts;
    fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Config> result =
        serial_client_.buffer(config_arena)->Config(kDefaultBaudRate, flags);
    if (!result.ok()) {
      fdf::error("Initial UART configuration failed, FIDL error: {}",
                 zx_status_get_string(result.status()));
      completer(zx::error(result.status()));
      return;
    }
    if (result->is_error()) {
      fdf::error("Initial UART configuration failed, domain error: {}",
                 zx_status_get_string(result->error_value()));
      completer(zx::error(result->error_value()));
      return;
    }
  }

  const auto config = context.take_config<bt_hci_broadcom_config::Config>();

  if (config.enable_suspend()) {
    zx::result<> power_init_result = InitPowerManagement();
    if (power_init_result.is_ok()) {
      fdf::info("Initialized power management");
    } else {
      fdf::error("Failed to initialize power management: {}", power_init_result);
      CompleteStart(power_init_result.error_value());
      return;
    }
  }

  core_dump_count_ = component_inspector_->root().CreateUint(kCoreDumpCountInspectPropertyName, 0);

  // Continue initialization through the fpromise executor.
  start_completer_.emplace(std::move(completer));
  executor_.emplace(dispatcher());
  executor_->schedule_task(Initialize().then([this](fpromise::result<void, zx_status_t>& result) {
    if (result.is_ok()) {
      CompleteStart(ZX_OK);
    } else {
      CompleteStart(result.take_error());
    }
  }));
}

void BtHciBroadcom::Stop(fdf::StopCompleter completer) { completer(zx::ok()); }

void BtHciBroadcom::GetFeatures(GetFeaturesCompleter::Sync& completer) {
  fidl::Arena arena;
  auto builder = fhbt::wire::VendorFeatures::Builder(arena);
  builder.acl_priority_command(true);
  builder.android_vendor_extensions(fhbt::wire::AndroidVendorSupport::Builder(arena).Build());
  completer.Reply(builder.Build());
}

void BtHciBroadcom::EncodeCommand(EncodeCommandRequestView request,
                                  EncodeCommandCompleter::Sync& completer) {
  uint8_t data_buffer[SetAclPriorityCommand::MaxSizeInBytes()];
  switch (request->Which()) {
    case fhbt::wire::VendorCommand::Tag::kSetAclPriority: {
      EncodeSetAclPriorityCommand(request->set_acl_priority(), data_buffer);
      auto encoded_cmd = fidl::VectorView<uint8_t>::FromExternal(
          data_buffer, SetAclPriorityCommand::MaxSizeInBytes());
      completer.ReplySuccess(encoded_cmd);
      return;
    }
    default: {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
  }
}

void BtHciBroadcom::OpenHci(OpenHciCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport> BtHciBroadcom::AddHciTransportClient(
    fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport> upstream_client_end) {
  auto [client_end, server_end] = fidl::Endpoints<fhbt::HciTransport>::Create();
  auto binding_ref = HciTransportPassthroughImpl::BindServer(
      executor_->dispatcher(), std::move(server_end), std::move(upstream_client_end),
      fit::bind_member<&BtHciBroadcom::NoteActivity>(this),
      fit::bind_member<&BtHciBroadcom::NoteCoreDump>(this));

  active_clients_.push_back(binding_ref);
  return std::move(client_end);
}

void BtHciBroadcom::OpenHciTransport(OpenHciTransportCompleter::Sync& completer) {
  fidl::ClientEnd<fhbt::HciTransport> client_end;
  if (hci_transport_client_end_.is_valid()) {
    client_end = std::move(hci_transport_client_end_);
  } else {
    // We need a new client end, because we already gave away the initialization one.
    zx::result<fidl::ClientEnd<fhbt::HciTransport>> client_end_result =
        incoming_->Connect<fhbt::HciService::HciTransport>();
    if (client_end_result.is_error()) {
      fdf::error("Connect to fhbt::HciTransport protocol failed: {}", client_end_result);
      completer.ReplyError(client_end_result.status_value());
      return;
    }
    client_end = std::move(*client_end_result);
  }
  fidl::ClientEnd<fhbt::HciTransport> passthrough_client =
      AddHciTransportClient(std::move(client_end));
  completer.ReplySuccess(std::move(passthrough_client));
}

void BtHciBroadcom::OpenSnoop(OpenSnoopCompleter::Sync& completer) {
  zx::result<fidl::ClientEnd<fhbt::Snoop>> client_end =
      incoming_->Connect<fhbt::HciService::Snoop>();
  if (client_end.is_error()) {
    fdf::error("Connect to Snoop protocol failed: {}", client_end);
    completer.ReplyError(client_end.status_value());
    return;
  }
  completer.ReplySuccess(std::move(*client_end));
}

void BtHciBroadcom::GetCrashParameters(GetCrashParametersCompleter::Sync& completer) {
  fidl::Arena arena;
  auto builder = fhbt::wire::VendorCrashParameters::Builder(arena);

  auto inner_view = fidl::VectorView<uint8_t>::FromExternal(
      const_cast<uint8_t*>(kCrashVendorSubeventPrefix.data()), kCrashVendorSubeventPrefix.size());
  std::array<fidl::VectorView<uint8_t>, 1> crash_events_array = {inner_view};

  builder.crash_events(fidl::VectorView<fidl::VectorView<uint8_t>>::FromExternal(
      crash_events_array.data(), crash_events_array.size()));
  builder.program_name(kCrashProgramName);
  builder.crash_signature(kCrashSignature);
  completer.ReplySuccess(builder.Build());
}

void BtHciBroadcom::handle_unknown_method(fidl::UnknownMethodMetadata<fhbt::Vendor> metadata,
                                          fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method in Vendor protocol, closing with ZX_ERR_NOT_SUPPORTED");
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t BtHciBroadcom::ConnectToHciTransportFidlProtocol() {
  zx::result<fidl::ClientEnd<fhbt::HciTransport>> client_end =
      incoming_->Connect<fhbt::HciService::HciTransport>();
  if (client_end.is_error()) {
    fdf::error("Connect to fhbt::HciTransport protocol failed: {}", client_end);
    return client_end.status_value();
  }

  hci_transport_client_ = fidl::WireSyncClient(*std::move(client_end));

  return ZX_OK;
}

zx_status_t BtHciBroadcom::ConnectToSerialFidlProtocol() {
  zx::result<fdf::ClientEnd<fuchsia_hardware_serialimpl::Device>> client_end =
      incoming_->Connect<fuchsia_hardware_serialimpl::Service::Device>();
  if (client_end.is_error()) {
    fdf::error("Connect to fuchsia_hardware_serialimpl::Device protocol failed: {}", client_end);
    return client_end.status_value();
  }

  serial_client_ = fdf::WireSyncClient(*std::move(client_end));
  return ZX_OK;
}

void BtHciBroadcom::EncodeSetAclPriorityCommand(fhbt::wire::VendorSetAclPriorityParams params,
                                                void* out_buffer) {
  if (!params.has_connection_handle() || !params.has_priority() || !params.has_direction()) {
    fdf::error("The command cannot be encoded because the following fields are missing: {} {} {}",
               params.has_connection_handle() ? "" : "connection_handle",
               params.has_priority() ? "" : "priority", params.has_direction() ? "" : "direction");
    return;
  }

  auto view = MakeSetAclPriorityCommandView(static_cast<uint8_t*>(out_buffer),
                                            SetAclPriorityCommand::MaxSizeInBytes());
  view.header().opcode().Write(BroadcomOpCode::SET_ACL_PRIORITY);
  view.header().parameter_total_size().Write(SetAclPriorityCommand::parameter_size());
  view.connection_handle().Write(params.connection_handle());
  view.priority().Write((params.priority() == fhbt::VendorAclPriority::kNormal)
                            ? AclPriority::NORMAL
                            : AclPriority::HIGH);
  view.direction().Write((params.direction() == fhbt::VendorAclDirection::kSource)
                             ? AclDirection::SOURCE
                             : AclDirection::SINK);
}

void BtHciBroadcom::OnReceivePacket(std::vector<uint8_t>& packet) {
  event_receive_buffer_ = packet;
  auto result = hci_transport_client_->AckReceive();
  if (result.status() != ZX_OK) {
    fdf::error("Failed to ack receive: {}", result.status_string());
  }
}

template <typename CmdView>
fpromise::promise<std::vector<uint8_t>, zx_status_t> BtHciBroadcom::SendCommand(CmdView view) {
  ZX_ASSERT(view.Ok());
  auto storage = view.BackingStorage();
  return SendCommand(storage.data(), view.IntrinsicSizeInBytes().Read());
}

fpromise::promise<std::vector<uint8_t>, zx_status_t> BtHciBroadcom::SendCommand(const void* command,
                                                                                size_t length) {
  // send HCI command
  fidl::Arena arena;
  auto command_vec = std::vector<uint8_t>(static_cast<const uint8_t*>(command),
                                          static_cast<const uint8_t*>(command) + length);
  auto command_view = fidl::VectorView<uint8_t>::FromExternal(command_vec);
  auto result =
      hci_transport_client_->Send(fhbt::wire::SentPacket::WithCommand(arena, command_view));
  if (result.status() != ZX_OK) {
    fdf::error("Failed to send command: {}", result.status_string());
    return fpromise::make_result_promise<std::vector<uint8_t>, zx_status_t>(
        fpromise::error(result.status()));
  }

  return ReadEvent();
}

fpromise::promise<std::vector<uint8_t>, zx_status_t> BtHciBroadcom::ReadEvent() {
  zx::result<std::vector<uint8_t>> result = ReadEventSync();
  if (result.is_error()) {
    fdf::error("Failed to read event");
    return fpromise::make_result_promise<std::vector<uint8_t>, zx_status_t>(
        fpromise::error(result.status_value()));
  }

  return fpromise::make_result_promise<std::vector<uint8_t>, zx_status_t>(
      fpromise::ok(std::move(result.value())));
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::SetBaudRate(uint32_t baud_rate) {
  std::array<std::byte, SetBaudRateCommand::MaxSizeInBytes()> storage;
  auto view = MakeSetBaudRateCommandView(&storage);
  view.header().opcode().Write(BroadcomOpCode::SET_BAUD_RATE);
  view.header().parameter_total_size().Write(SetBaudRateCommand::parameter_size());
  view.unused().Write(0);
  view.baud_rate().Write(baud_rate);

  return SendCommand(view).and_then(
      [this, baud_rate](const std::vector<uint8_t>&) -> fpromise::result<void, zx_status_t> {
        fdf::Arena arena('CONF');
        fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Config> result =
            serial_client_.buffer(arena)->Config(baud_rate, fhsi::kSerialSetBaudRateOnly);
        if (!result.ok()) {
          return fpromise::error(result.status());
        }
        if (result->is_error()) {
          return fpromise::error(result->error_value());
        }
        return fpromise::ok();
      });
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::EnableLowPowerMode(
    zx::duration host_idle_threshold, zx::duration device_idle_threshold) {
  if (serial_pid_ != PDEV_PID_BCM4381A1) {
    fdf::info("skipping low power settings on non-4381");
    return fpromise::make_promise(
        []() { return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok()); });
  }
  // These are in 12.5ms increments.

  std::array<std::byte, WriteSleepModeCmd::MaxSizeInBytes()> storage;
  return SendCommand(EnableLowPowerModeCmd(&storage, host_idle_threshold, device_idle_threshold))
      .and_then([](const std::vector<uint8_t>& cmd_complete) {
        auto view = pw::bluetooth::emboss::MakeSimpleCommandCompleteEventView(cmd_complete.data(),
                                                                              cmd_complete.size());
        if (view.Ok()) {
          if (view.status().Read() == pw::bluetooth::emboss::StatusCode::SUCCESS) {
            fdf::info("set low power mode settings");
          } else {
            fdf::warn("failed to set low power mode: 0x{:02x}",
                      static_cast<uint8_t>(view.status().Read()));
          }
        } else {
          fdf::warn("LowPowerMode CmdComplete is too small or invalid: {}", cmd_complete.size());
        }
      });
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::DisableLowPowerMode() {
  if (serial_pid_ != PDEV_PID_BCM4381A1) {
    fdf::info("skipping low power settings on non-4381");
    return fpromise::make_promise(
        []() { return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok()); });
  }
  std::array<std::byte, WriteSleepModeCmd::MaxSizeInBytes()> storage;
  return SendCommand(DisableLowPowerModeCmd(&storage))
      .and_then([](const std::vector<uint8_t>& cmd_complete) {
        auto view = pw::bluetooth::emboss::MakeSimpleCommandCompleteEventView(cmd_complete.data(),
                                                                              cmd_complete.size());
        if (view.Ok()) {
          if (view.status().Read() != pw::bluetooth::emboss::StatusCode::SUCCESS) {
            fdf::warn("failed to disable low power mode: 0x{:02x}",
                      static_cast<uint8_t>(view.status().Read()));
          }
        } else {
          fdf::warn("LowPowerMode CmdComplete is too small or invalid: {}", cmd_complete.size());
        }
      });
}

zx::result<> BtHciBroadcom::InitPowerManagement() {
  zx::result open_result = incoming_->Open<fuchsia_io::File>("/pkg/data/broadcom_power.fidl",
                                                             fuchsia_io::Flags::kPermReadBytes);
  if (!open_result.is_ok() || !open_result->is_valid()) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  zx::result<fuchsia_hardware_power::ComponentPowerConfiguration> load_result =
      power_config::Load(std::move(open_result.value()));
  if (load_result.is_error()) {
    fdf::error("Loading Power config failed: {}", load_result);
    return load_result.take_error();
  }

  std::vector<fdf_power::PowerElementConfiguration> element_configs;
  for (const fuchsia_hardware_power::PowerElementConfiguration& element_config :
       load_result.value().power_elements()) {
    auto converted = fdf_power::PowerElementConfiguration::FromFidl(element_config);
    if (converted.is_error()) {
      fdf::error("Converting power element config failed: {}", converted);
      return converted.take_error();
    }
    element_configs.push_back(converted.value());
  }

  zx::result<fdf_power::ElementDesc> element_desc =
      ApplyPowerConfiguration(std::move(element_configs));
  if (element_desc.is_error()) {
    return element_desc.take_error();
  }

  assertive_token_ = std::move(element_desc->assertive_token);
  element_control_client_end_ = *std::move(element_desc->element_control_client);
  element_runner_server_binding_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         std::move(element_desc->element_runner_server.value()),
                                         this, fidl::kIgnoreBindingClosure);
  element_lessor_client_ = std::move(*element_desc->lessor_client);

  fidl::WireResult lease = fidl::WireCall(element_lessor_client_)->Lease(PowerLevel::kBoot);
  if (!lease.ok()) {
    fdf::error("Call to Lease failed: {}", lease.error().FormatDescription());
    return zx::error(lease.error().status());
  }
  if (lease->is_error()) {
    fdf::error("Failed to acquire lease: {}", fdf_power::LeaseErrorToString(lease->error_value()));
    return fdf_power::LeaseErrorToZxError(lease->error_value());
  }
  level_lease_client_.emplace(std::move(lease->value()->lease_control), dispatcher());
  return zx::ok();
}

zx::result<fdf_power::ElementDesc> BtHciBroadcom::ApplyPowerConfiguration(
    std::vector<fdf_power::PowerElementConfiguration> element_configs) {
  // One for the power element
  constexpr size_t kExpectedPowerElementConfigs = 1;

  if (element_configs.size() != kExpectedPowerElementConfigs) {
    fdf::error("Unexpected number of power element configs: {} != {}", element_configs.size(),
               kExpectedPowerElementConfigs);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fit::result<fdf_power::Error, std::vector<fdf_power::ElementDesc>> result =
      fdf_power::ApplyPowerConfiguration(*incoming_, element_configs,
                                         /*use_element_runner=*/true);
  if (result.is_error()) {
    fdf::info("Failed to apply power config: {}", fdf_power::ErrorToString(result.error_value()));
    return fdf_power::ErrorToZxError(result.error_value());
  }
  if (result->size() != 1) {
    fdf::error("Unexpected element desc count {}", result->size());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fdf_power::ElementDesc& element_desc = result->at(0);
  fdf::info("Power element applied: \"{}\"", element_desc.element_config.element.name);

  if (element_desc.element_config.element.levels.size() != PowerLevel::kPowerLevelCount) {
    fdf::error("Got {} power levels, expected {}",
               element_desc.element_config.element.levels.size(),
               static_cast<uint32_t>(PowerLevel::kPowerLevelCount));
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(std::move(element_desc));
}

void BtHciBroadcom::SetLevel(fuchsia_power_broker::wire::ElementRunnerSetLevelRequest* request,
                             SetLevelCompleter::Sync& completer) {
  fdf::debug("SetLevel {} ?-> {} ", static_cast<uint32_t>(power_level_),
             static_cast<uint32_t>(request->level));

  if (power_level_ == request->level) {
    completer.Reply();
    return;
  }

  if (power_level_ == PowerLevel::kBoot) {
    if (request->level == PowerLevel::kOff) {
      fdf::debug("Initial powerlevel off request (but we are trying to boot), ignoring..");
      completer.Reply();
      return;
    }
    // We don't expect another transition within Boot mode, until we drop the Boot lease at the end
    // of initialization.
    fdf::warn("Within boot mode, got unexpected SetLevel({}) - ignoring..",
              static_cast<int>(request->level));
  }

  // The only two transitions we expect are from OFF -> ON, and ON -> OFF.
  // These are caused by our self-lease (in AcquirePowerElementLease) and signal that
  // dependent power elements are at the correct level when ON.
  // Log any other transitions.
  if ((power_level_ == PowerLevel::kOff && request->level != PowerLevel::kOn) ||
      (power_level_ == PowerLevel::kOn && request->level != PowerLevel::kOff)) {
    fdf::warn("Got unexpected SetLevel Transition: {} -> {}", static_cast<int>(power_level_),
              static_cast<int>(request->level));
  }

  completer.Reply();
}

void BtHciBroadcom::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unexpected ElementRunner method ordinal {:#018x}", metadata.method_ordinal);
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::AssertLevel(PowerLevel requested_level) {
  if (!element_lessor_client_.is_valid()) {
    // We are not using power framework
    return fpromise::make_promise(
        []() { return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok()); });
  }
  if (requested_level == power_level_) {
    // We don't need to change our power level, but we may need to bump the timeout.
    if (requested_level != PowerLevel::kOff) {
      drop_level_task_.Cancel();
      drop_level_task_.PostDelayed(dispatcher(), 2 * kDefaultHostIdleThreshold);
    }
    return fpromise::make_promise(
        []() { return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok()); });
  }
  switch (requested_level) {
    case PowerLevel::kOn:
    case PowerLevel::kBoot:
      return AcquirePowerElementLease();
    case PowerLevel::kOff:
      if (level_lease_client_) {
        // We are not asserting anymore, release any lease we have.
        auto result = level_lease_client_->UnbindMaybeGetEndpoint();
        if (result.is_error()) {
          fdf::error("Tried to unbind when we have a pending call?!");
        }
        level_lease_client_.reset();
      } else {
        fdf::warn("Would have unbound, but we don't have a valid cilent");
      }
      break;
    default:
      fdf::error("Unexpected level {}", static_cast<uint32_t>(requested_level));
      return fpromise::make_error_promise(ZX_ERR_INVALID_ARGS);
  }

  power_level_ = requested_level;
  return fpromise::make_promise(
      []() { return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok()); });
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::AcquirePowerElementLease() {
  if (level_lease_client_) {
    fdf::debug("Not acquiring a lease due to already having a lease");
    return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
  }

  // Request dependent power nodes rise to kPowerLevelOn
  fidl::WireResult lease = fidl::WireCall(element_lessor_client_)->Lease(kOn);
  if (!lease.ok()) {
    fdf::error("Call to Lease failed: {}", lease.error().FormatDescription());
    return fpromise::make_result_promise<void, zx_status_t>(fpromise::error(ZX_ERR_IO));
  }
  if (lease->is_error()) {
    fdf::error("Failed to acquire lease: {}", fdf_power::LeaseErrorToString(lease->error_value()));
    return fpromise::make_result_promise<void, zx_status_t>(fpromise::error(ZX_ERR_IO));
  }

  level_lease_client_.emplace(std::move(lease->value()->lease_control), dispatcher());

  fpromise::bridge<void, zx_status_t> bridge;
  (*level_lease_client_)
      ->WatchStatus(fuchsia_power_broker::wire::LeaseStatus::kPending)
      .Then([this, completer = std::move(bridge.completer)](auto& lease_satisfied_result) mutable {
        if (!lease_satisfied_result.ok()) {
          fdf::error("Call to Lease WatchStatus failed: {}",
                     lease_satisfied_result.error().FormatDescription());
          completer.complete_error(ZX_ERR_INTERNAL);
          return;
        }
        if (lease_satisfied_result->status != fuchsia_power_broker::LeaseStatus::kSatisfied) {
          fdf::error("Call to Lease WatchStatus did not result in kSatisfied!?");
          completer.complete_error(ZX_ERR_BAD_STATE);
          return;
        }
        fdf::debug("Lease is satisfied");
        power_level_ = PowerLevel::kOn;
        // Schedule when we should drop the lease.
        drop_level_task_.Cancel();
        drop_level_task_.PostDelayed(dispatcher(), 2 * kDefaultHostIdleThreshold);
        completer.complete_ok();
        return;
      });
  return bridge.consumer.promise();
}

void BtHciBroadcom::HandleWakeLeaseTimeout() {
  executor_->schedule_task(AssertLevel(PowerLevel::kOff));
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::SetBdaddr(
    const std::array<uint8_t, kMacAddrLen>& bdaddr) {
  std::array<std::byte, SetBdaddrCommand::MaxSizeInBytes()> storage;
  auto view = MakeSetBdaddrCommandView(&storage);
  view.header().opcode().Write(BroadcomOpCode::SET_BD_ADDR);
  view.header().parameter_total_size().Write(SetBdaddrCommand::parameter_size());
  std::byte* raw_addr_storage = view.bdaddr().BackingStorage().data();
  for (size_t i = 0; i < kMacAddrLen; ++i) {
    raw_addr_storage[i] = static_cast<std::byte>(bdaddr[kMacAddrLen - 1 - i]);
  }

  return SendCommand(view).and_then([](const std::vector<uint8_t>&) {});
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::SetDefaultPowerCaps() {
  if (serial_pid_ != PDEV_PID_BCM4381A1) {
    return fpromise::make_promise(
        []() { return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok()); });
  }
  std::array<std::byte, SetPowerCapCommand::MaxSizeInBytes()> storage;
  return SendCommand(MakeDefaultPowerCapCommand(&storage))
      .and_then([](std::vector<uint8_t>& cmd_complete) {
        auto view = pw::bluetooth::emboss::MakeSimpleCommandCompleteEventView(cmd_complete.data(),
                                                                              cmd_complete.size());
        if (view.Ok()) {
          if (view.status().Read() == pw::bluetooth::emboss::StatusCode::SUCCESS) {
            fdf::info("set default power caps");
          } else {
            fdf::warn("failed to set default power caps: 0x{:02x}",
                      static_cast<uint8_t>(view.status().Read()));
          }
        }
      });
}

void BtHciBroadcom::NoteActivity(ActivityType activity) {
  executor_->schedule_task(AssertLevel(PowerLevel::kOn));
}

void BtHciBroadcom::NoteCoreDump() {
  zx::time now = async::Now(dispatcher_);
  if (!last_core_dump_time_.has_value() || (now - *last_core_dump_time_) >= kCoreDumpCooldown) {
    core_dump_count_.Add(1);
    last_core_dump_time_ = now;
  }
}

constexpr auto kOpenFlags = fuchsia_io::Flags::kPermReadBytes | fuchsia_io::Flags::kProtocolFile;

fpromise::promise<void, zx_status_t> BtHciBroadcom::LoadFirmware(bool fast_download) {
  zx::vmo fw_vmo;
  size_t fw_size;

  // If there's no firmware for this PID, we don't expect the bind to happen without a
  // corresponding entry in the firmware table. Please double-check the PID value and add an entry
  // to the firmware table if it's valid.
  ZX_ASSERT_MSG(kFirmwareMap.find(serial_pid_) != kFirmwareMap.end(), "no mapping for PID: %u",
                serial_pid_);

  std::string full_filename = "/pkg/lib/firmware/";
  full_filename.append(kFirmwareMap.at(serial_pid_));

  auto client = incoming_->Open<fuchsia_io::File>(full_filename.c_str(), kOpenFlags);
  if (client.is_error()) {
    fdf::warn("Open firmware file failed: {}", zx_status_get_string(client.error_value()));
    return fpromise::make_error_promise(client.error_value());
  }

  fidl::WireResult backing_memory_result =
      fidl::WireCall(*client)->GetBackingMemory(fuchsia_io::wire::VmoFlags::kRead);
  if (!backing_memory_result.ok()) {
    if (backing_memory_result.is_peer_closed()) {
      fdf::warn("Failed to get backing memory: Peer closed");
      return fpromise::make_error_promise(ZX_ERR_NOT_FOUND);
    }
    fdf::warn("Failed to get backing memory: {}",
              zx_status_get_string(backing_memory_result.status()));
    return fpromise::make_error_promise(backing_memory_result.status());
  }

  const auto* backing_memory = backing_memory_result.Unwrap();
  if (backing_memory->is_error()) {
    fdf::warn("Failed to get backing memory: {}",
              zx_status_get_string(backing_memory->error_value()));
    return fpromise::make_error_promise(backing_memory->error_value());
  }

  zx::vmo& backing_vmo = backing_memory->value()->vmo;
  if (zx_status_t status = backing_vmo.get_prop_content_size(&fw_size); status != ZX_OK) {
    fdf::warn("Failed to get vmo size: {}", zx_status_get_string(status));
    return fpromise::make_error_promise(status);
  }
  fw_vmo.reset(backing_vmo.release());

  fpromise::promise<std::vector<uint8_t>, zx_status_t> download_cmd_promise;
  if (fast_download) {
    std::array<std::byte, SetDownloadConfigCommand::MaxSizeInBytes()> storage;
    auto view = MakeSetDownloadConfigCommandView(&storage);
    view.header().opcode().Write(BroadcomOpCode::SET_DOWNLOAD_CONFIG);
    view.header().parameter_total_size().Write(SetDownloadConfigCommand::parameter_size());
    view.command_version().Write(0x00);
    view.fast_download_mode().Write(0x01);
    download_cmd_promise =
        SendCommand(view)
            .and_then([this](const std::vector<uint8_t>& /*event*/) {
              std::array<std::byte, StartFirmwareDownloadCommand::MaxSizeInBytes()> storage;
              auto view = MakeStartFirmwareDownloadCommandView(&storage);
              view.header().opcode().Write(BroadcomOpCode::START_FIRMWARE_DOWNLOAD);
              view.header().parameter_total_size().Write(
                  StartFirmwareDownloadCommand::parameter_size());
              return SendCommand(view);
            })
            .box();
  } else {
    std::array<std::byte, StartFirmwareDownloadCommand::MaxSizeInBytes()> storage;
    auto view = MakeStartFirmwareDownloadCommandView(&storage);
    view.header().opcode().Write(BroadcomOpCode::START_FIRMWARE_DOWNLOAD);
    view.header().parameter_total_size().Write(StartFirmwareDownloadCommand::parameter_size());
    download_cmd_promise = SendCommand(view);
  }

  return download_cmd_promise
      .or_else([](zx_status_t& status) -> fpromise::result<std::vector<uint8_t>, zx_status_t> {
        fdf::error("could not load firmware file");
        return fpromise::error(status);
      })
      .and_then([this](std::vector<uint8_t>& /*event*/) mutable {
        // give time for placing firmware in download mode
        return executor_->MakeDelayedPromise(zx::duration(kFirmwareDownloadDelay))
            .then([](fpromise::result<>& /*result*/) {
              return fpromise::result<void, zx_status_t>(fpromise::ok());
            });
      })
      .and_then([this, fw_vmo = std::move(fw_vmo), fw_size,
                 fast_download]() mutable -> fpromise::result<void, zx_status_t> {
        zx::time start_time = async::Now(dispatcher_);

        // The firmware is a sequence of HCI commands containing the firmware data as payloads.
        zx_status_t status = SendVmoAsCommands(std::move(fw_vmo), fw_size, fast_download);
        if (status != ZX_OK) {
          return fpromise::error(status);
        }

        zx::duration firmware_duration = async::Now(dispatcher_) - start_time;
        FDF_LOG(INFO, "Transferred firmware (duration: %" PRId64 " ms, fast: %d)",
                firmware_duration.to_msecs(), fast_download);
        return fpromise::ok();
      })
      .and_then([this]() -> fpromise::promise<void, zx_status_t> {
        if (is_uart_) {
          // firmware switched us back to 115200. switch back to kTargetBaudRate.
          fdf::Arena arena('CONF');
          fdf::WireUnownedResult<fuchsia_hardware_serialimpl::Device::Config> result =
              serial_client_.buffer(arena)->Config(kDefaultBaudRate, fhsi::kSerialSetBaudRateOnly);
          if (!result.ok()) {
            return fpromise::make_result_promise(fpromise::error(result.status()));
          }
          if (result->is_error()) {
            return fpromise::make_result_promise(fpromise::error(result->error_value()));
          }

          return executor_->MakeDelayedPromise(kBaudRateSwitchDelay)
              .then(
                  [this](fpromise::result<>& /*result*/) { return SetBaudRate(kTargetBaudRate); });
        }
        return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
      })
      .and_then([]() { fdf::info("firmware loaded"); });
}

zx_status_t BtHciBroadcom::SendCommandSync(const void* command, size_t length) {
  zx_status_t status = SendCommandWithoutEvent(command, length);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "Failed to send command: %s", zx_status_get_string(status));
    return status;
  }

  return ReadEventSync().status_value();
}

zx_status_t BtHciBroadcom::SendCommandWithoutEvent(const void* command, size_t length) {
  fidl::Arena arena;
  auto command_vec = std::vector<uint8_t>(static_cast<const uint8_t*>(command),
                                          static_cast<const uint8_t*>(command) + length);
  auto command_view = fidl::VectorView<uint8_t>::FromExternal(command_vec);

  auto result =
      hci_transport_client_->Send(fhbt::wire::SentPacket::WithCommand(arena, command_view));
  if (result.status() != ZX_OK) {
    fdf::error("Failed to send command: {}", result.status_string());
  }
  return result.status();
}

zx::result<std::vector<uint8_t>> BtHciBroadcom::ReadEventSync() {
  fidl::Status result = hci_transport_client_.HandleOneEvent(hci_event_handler_);
  if (result.status() != ZX_OK) {
    fdf::error("Failed to get event packet: {}", zx_status_get_string(result.status()));
    return zx::error(result.status());
  }

  // Read result will be stored in |event_receive_buffer_|.
  std::vector<uint8_t> packet_bytes = std::move(event_receive_buffer_);
  // Copy out the data from buffer and clear the buffer.
  event_receive_buffer_.clear();

  auto view = pw::bluetooth::emboss::MakeSimpleCommandCompleteEventView(packet_bytes.data(),
                                                                        packet_bytes.size());
  if (!view.Ok()) {
    fdf::error("command channel read too short or invalid: {}", packet_bytes.size());
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (view.command_complete().header().event_code().Read() !=
      pw::bluetooth::emboss::EventCode::COMMAND_COMPLETE) {
    fdf::error("did not receive command complete");
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (view.status().Read() != pw::bluetooth::emboss::StatusCode::SUCCESS) {
    fdf::error("got command complete error 0x{:02x}", static_cast<uint8_t>(view.status().Read()));
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(packet_bytes));
}

zx_status_t BtHciBroadcom::SendVmoAsCommands(zx::vmo vmo, size_t size, bool fast_download) {
  size_t offset = 0;

  while (offset < size) {
    uint8_t buffer[kMaxHciCommandSize];

    size_t remaining = size - offset;
    size_t read_amount = (remaining > sizeof(buffer) ? sizeof(buffer) : remaining);

    if (read_amount < CommandHeader::IntrinsicSizeInBytes()) {
      fdf::error("short HCI command in firmware download");
      return ZX_ERR_INTERNAL;
    }

    zx_status_t status = vmo.read(buffer, offset, read_amount);
    if (status != ZX_OK) {
      return status;
    }

    auto header_view = MakeCommandHeaderView(buffer, CommandHeader::IntrinsicSizeInBytes());
    size_t length =
        header_view.parameter_total_size().Read() + CommandHeader::IntrinsicSizeInBytes();
    if (read_amount < length) {
      fdf::error("short HCI command in firmware download");
      return ZX_ERR_INTERNAL;
    }

    offset += length;
    if (fast_download) {
      if (zx_status_t status = SendCommandWithoutEvent(buffer, length); status != ZX_OK) {
        fdf::error("SendCommand failed in firmware download: {}", zx_status_get_string(status));
        return status;
      }

      // In Fast Download mode, only the Launch RAM command returns an event.
      if (header_view.opcode().Read() == BroadcomOpCode::LAUNCH_RAM) {
        if (zx::result<std::vector<uint8_t>> res = ReadEventSync(); res.is_error()) {
          fdf::error("Failed to read event for Launch RAM command: {}",
                     zx_status_get_string(res.error_value()));
          return res.error_value();
        }
      }
    } else {
      if (zx_status_t status = SendCommandSync(buffer, length); status != ZX_OK) {
        fdf::error("SendCommand failed in firmware download: {}", zx_status_get_string(status));
        return status;
      }
    }
  }

  return ZX_OK;
}

fpromise::promise<std::vector<uint8_t>, zx_status_t> BtHciBroadcom::SendHciReset() {
  std::array<std::byte, pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes()> storage;
  auto view = pw::bluetooth::emboss::MakeCommandHeaderView(&storage);
  view.opcode().Write(pw::bluetooth::emboss::OpCode::RESET);
  view.parameter_total_size().Write(0);
  return SendCommand(view);
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::Initialize() {
  fdf::debug("sending initial reset command");
  return SendHciReset()
      .and_then([this](const std::vector<uint8_t>&) -> fpromise::promise<void, zx_status_t> {
        if (is_uart_) {
          fdf::debug("setting baud rate to {}", kTargetBaudRate);
          // switch baud rate to TARGET_BAUD_RATE
          return SetBaudRate(kTargetBaudRate);
        }
        return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
      })
      .and_then([this]() {
        fdf::debug("sending read verbose config version info command");
        std::array<std::byte, CommandHeader::MaxSizeInBytes()> storage;
        auto view = MakeCommandHeaderView(&storage);
        view.opcode().Write(BroadcomOpCode::READ_VERBOSE_CONFIG_VERSION_INFO);
        view.parameter_total_size().Write(0);
        return SendCommand(view);
      })
      .then([this](fpromise::result<std::vector<uint8_t>, zx_status_t>& result) {
        bool fast_download_supported = false;
        if (result.is_error()) {
          fdf::error("Read verbose config command failed: {}",
                     zx_status_get_string(result.error()));
        } else {
          const auto& cmd_complete = result.value();
          auto event = MakeReadVerboseConfigVersionInfoCommandCompleteEventView(
              cmd_complete.data(), cmd_complete.size());
          if (event.Ok() && event.status().Read() == pw::bluetooth::emboss::StatusCode::SUCCESS &&
              static_cast<uint16_t>(event.command_complete().command_opcode().Read()) ==
                  static_cast<uint16_t>(BroadcomOpCode::READ_VERBOSE_CONFIG_VERSION_INFO)) {
            uint8_t chip_id = event.chip_id().Read();
            fdf::info("Chip ID: {}", chip_id);
            if (chip_id >= kFastDownloadChipIdMin) {
              fast_download_supported = true;
            }
          } else if (!event.Ok()) {
            fdf::error("Read verbose config failed: response too short or invalid");
          } else {
            fdf::error("Read verbose config failed: {}",
                       static_cast<uint8_t>(event.status().Read()));
          }
        }
        fdf::debug("loading firmware");
        return LoadFirmware(fast_download_supported);
      })
      .and_then([this]() {
        fdf::debug("sending reset command");
        return SendHciReset();
      })
      .and_then([this](std::vector<uint8_t>&) -> fpromise::promise<void, zx_status_t> {
        fdf::debug("Getting mac address");
        zx::result metadata =
            fdf_metadata::GetMetadata<fuchsia_boot_metadata::MacAddressMetadata>(incoming_);
        if (metadata.is_error()) {
          fdf::error("Error reading metadata: {}", metadata.status_string());
          return fpromise::make_error_promise(ZX_ERR_INTERNAL);
        }

        if (!metadata.value().mac_address().has_value()) {
          fdf::error("Mac address metadata missing mac address");
          return fpromise::make_error_promise(ZX_ERR_INTERNAL);
        }
        const auto& octets = metadata.value().mac_address().value().octets();
        fdf::info("Got mac address {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", octets[0], octets[1],
                  octets[2], octets[3], octets[4], octets[5]);

        // send Set BDADDR command
        return SetBdaddr(octets);
      })
      .and_then([this]() { return SetDefaultPowerCaps(); })
      .and_then([this]() {
        return EnableLowPowerMode(kDefaultHostIdleThreshold, kDefaultDevIdleThreshold);
      })
      .then([this](fpromise::result<void, zx_status_t>& result) {
        zx_status_t status = result.is_ok() ? ZX_OK : result.error();
        return OnInitializeComplete(status);
      });
}

fpromise::promise<void, zx_status_t> BtHciBroadcom::OnInitializeComplete(zx_status_t status) {
  // We're done with the HciTransport client end. Allow the HciTransport clients we vend to use it.
  hci_transport_client_end_ = hci_transport_client_.TakeClientEnd();

  if (status != ZX_OK) {
    fdf::error("device initialization failed: {}", zx_status_get_string(status));
    return fpromise::make_error_promise(status);
  }

  fhbt::Service::InstanceHandler handler({
      .vendor =
          vendor_binding_group_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
  zx::result add_service_result = outgoing()->AddService<fhbt::Service>(std::move(handler));
  if (add_service_result.is_error()) {
    fdf::error("Failed to add vendor service: {}", add_service_result.status_string());
    return fpromise::make_error_promise(add_service_result.error_value());
  }

  // We are done booting, we can drop our boot power needs.
  fdf::debug("dropping boot power lease");
  executor_->schedule_task(AssertLevel(PowerLevel::kOff));
  fdf::info("initialization completed successfully.");
  return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
}

void BtHciBroadcom::CompleteStart(zx_status_t status) {
  if (start_completer_.has_value()) {
    start_completer_.value()(zx::make_result(status));
    start_completer_.reset();
  } else {
    fdf::error("CompleteStart called without start_completer_.");
  }
}

}  // namespace bt_hci_broadcom

FUCHSIA_DRIVER_EXPORT2(bt_hci_broadcom::BtHciBroadcom);
