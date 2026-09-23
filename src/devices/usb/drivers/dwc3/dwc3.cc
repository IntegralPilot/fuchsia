// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc3/dwc3.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clock/cpp/fidl.h>
#include <fidl/fuchsia.hardware.interconnect/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.reset/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.dci/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/natural_ostream.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.policy/cpp/common_types_format.h>
#include <fidl/fuchsia.hardware.usb.policy/cpp/fidl.h>
#include <fidl/fuchsia.hardware.vreg/cpp/fidl.h>
#include <fidl/fuchsia.inspect/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sync/completion.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <sstream>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/designware/platform/cpp/bind.h>
#include <fbl/auto_lock.h>
#include <hwreg/bitfields.h>

#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"

namespace dwc3 {

namespace fclock = fuchsia_hardware_clock;
namespace fdci = fuchsia_hardware_usb_dci;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace fhi = fuchsia_hardware_interconnect;
namespace fpdev = fuchsia_hardware_platform_device;
namespace fphy = fuchsia_hardware_usb_phy;
namespace fpolicy = fuchsia_hardware_usb_policy;
namespace freset = fuchsia_hardware_reset;
namespace fvreg = fuchsia_hardware_vreg;

namespace {

class QualcommExtension final : public PlatformExtension {
  enum class BusPath : uint8_t { kUsbDdr, kUsbIpa, kDdrUsb };
  enum class State : uint8_t { kNone, kNominal, kSvs, kMin };

  class InterconnectHandler {
   public:
    explicit InterconnectHandler(
        std::unordered_map<BusPath, fidl::ClientEnd<fhi::Path>> interconnect_clients) {
      for (auto& [path, client_end] : interconnect_clients) {
        interconnect_clients_.emplace(
            path, fidl::Client<fhi::Path>(std::move(client_end),
                                          fdf::Dispatcher::GetCurrent()->async_dispatcher()));
      }
    }

    void SetBandwidth(BusPath path, uint32_t average, uint32_t peak,
                      fit::callback<void()> callback) {
      interconnect_clients_.at(path)
          ->SetBandwidth({{.average_bandwidth_bps = average, .peak_bandwidth_bps = peak}})
          .Then([path = path, callback = std::move(callback)](
                    fidl::Result<fhi::Path::SetBandwidth>& result) mutable {
            if (result.is_error()) {
              fdf::error("Failed to set bandwidth for path {}: {}", static_cast<uint8_t>(path),
                         result.error_value().FormatDescription());
            }
            callback();
          });
    }

   private:
    std::unordered_map<BusPath, fidl::Client<fhi::Path>> interconnect_clients_;
  };

 public:
  class HsPhyCtrl : public hwreg::RegisterBase<HsPhyCtrl, uint32_t> {
   public:
    DEF_BIT(20, utmi_otg_vbus_valid);

    static auto Get() { return hwreg::RegisterAddr<HsPhyCtrl>(0xf'8810); }
  };

  static std::unique_ptr<QualcommExtension> Create(Dwc3* parent, const fdf::MmioView& mmio);

  QualcommExtension(const fdf::MmioView& mmio,
                    std::unordered_map<BusPath, fidl::ClientEnd<fhi::Path>> interconnect_clients,
                    std::unordered_map<std::string, fidl::ClientEnd<fclock::Clock>> clock_clients,
                    fidl::ClientEnd<freset::Reset> reset_client,
                    fidl::ClientEnd<fvreg::Vreg> regulator_client,
                    fdf::Dispatcher interconnect_dispatcher)
      : mmio_(mmio),
        clock_clients_{std::move(clock_clients)},
        reset_client_(std::move(reset_client)),
        regulator_client_(std::move(regulator_client)),
        interconnect_dispatcher_(std::move(interconnect_dispatcher)),
        interconnect_handler_(interconnect_dispatcher_.async_dispatcher(), std::in_place,
                              std::move(interconnect_clients)) {}

  // PlatformExtension interface implementation.
  zx::result<> Start() override {
    TRACE_DURATION("dwc3", "QualcommExtension::Start");
    return PowerOn(true);
  }
  zx::result<> Suspend() override {
    TRACE_DURATION("dwc3", "QualcommExtension::Suspend");
    HsPhyCtrl::Get().ReadFrom(&mmio_).set_utmi_otg_vbus_valid(false).WriteTo(&mmio_);
    return PowerOff();
  }
  zx::result<> Resume() override {
    TRACE_DURATION("dwc3", "QualcommExtension::Resume");
    if (zx::result<> result = PowerOn(false); result.is_error()) {
      return result;
    }
    HsPhyCtrl::Get().ReadFrom(&mmio_).set_utmi_otg_vbus_valid(true).WriteTo(&mmio_);
    return zx::ok();
  }

 private:
  zx::result<> PowerOn(bool driver_start) {
    TRACE_DURATION("dwc3", "QualcommExtension::PowerOn", "driver_start", driver_start);
    if (power_on_) {
      return zx::ok();
    }

    if (zx::result<> result = VoteBandwidth(State::kSvs); result.is_error()) {
      return result.take_error();
    }

    if (zx::result<> result = VoteVoltage(true); result.is_error()) {
      return result.take_error();
    }

    // Only reset after being powered down, not during driver start.
    if (!driver_start) {
      if (fidl::Result result = fidl::Call(reset_client_)->ToggleWithTimeout(zx::usec(1500).get());
          result.is_error()) {
        fdf::error("Failed to toggle reset {}", result.error_value().FormatDescription().c_str());
        return zx::error(result.error_value().is_domain_error()
                             ? result.error_value().domain_error()
                             : result.error_value().framework_error().status());
      }
    }

    if (zx::result<> result = VoteClocks(true); result.is_error()) {
      return result;
    }

    fdf::info("Qualcomm extension: dwc3 core powered on");

    power_on_ = true;
    return zx::ok();
  }

  zx::result<> PowerOff() {
    TRACE_DURATION("dwc3", "QualcommExtension::PowerOff");
    if (!power_on_) {
      return zx::ok();
    }

    if (zx::result<> result = VoteClocks(false); result.is_error()) {
      return result.take_error();
    }

    if (zx::result<> result = VoteVoltage(false); result.is_error()) {
      return result.take_error();
    }

    if (zx::result<> result = VoteBandwidth(State::kNone); result.is_error()) {
      return result.take_error();
    }

    fdf::info("Qualcomm extension: dwc3 core powered off");

    power_on_ = false;
    return zx::ok();
  }

  zx::result<> VoteBandwidth(State state);
  void SetInterconnectBandwidths(State state);
  zx::result<> VoteVoltage(bool on);
  zx::result<> VoteClocks(bool on);

  State state_ = State::kNone;
  fdf::MmioView mmio_;
  std::unordered_map<std::string, fidl::ClientEnd<fclock::Clock>> clock_clients_;
  fidl::ClientEnd<freset::Reset> reset_client_;
  fidl::ClientEnd<fvreg::Vreg> regulator_client_;
  fdf::Dispatcher interconnect_dispatcher_;
  async_patterns::DispatcherBound<InterconnectHandler> interconnect_handler_;
  bool power_on_{false};
};

std::unique_ptr<QualcommExtension> QualcommExtension::Create(Dwc3* parent,
                                                             const fdf::MmioView& mmio) {
  TRACE_DURATION("dwc3", "QualcommExtension::Create");
  // Get all resources.
  static const std::unordered_map<BusPath, const std::string> kBusPathNames{
      {BusPath::kUsbDdr, "usb-ddr"}, {BusPath::kUsbIpa, "usb-ipa"}, {BusPath::kDdrUsb, "ddr-usb"}};

  static const std::vector<std::string> kClockNames{"core-clk", "iface-clk", "bus-aggr-clk",
                                                    "xo",       "sleep-clk", "utmi-clk"};

  if (mmio.get_size() < HsPhyCtrl::Get().addr() + sizeof(HsPhyCtrl::ValueType)) {
    fdf::info("MMIO is too small to be qualcomm chipset");
    return nullptr;
  }

  std::unordered_map<BusPath, fidl::ClientEnd<fhi::Path>> interconnect_clients;
  for (const auto& [path, node_name] : kBusPathNames) {
    auto interconnect_result = parent->incoming()->OpenService<fhi::PathService>(node_name);
    if (interconnect_result.is_error()) {
      fdf::info("Failed to open interconnect service {}, assuming not qualcomm chipset", node_name);
      return nullptr;
    }
    auto interconnect_client = interconnect_result->connect_path();
    if (interconnect_client.is_error()) {
      fdf::info("Failed to connect to interconnect {}, assuming not qualcomm chipset", node_name);
      return nullptr;
    }
    interconnect_clients[path] = std::move(*interconnect_client);
  }

  std::unordered_map<std::string, fidl::ClientEnd<fclock::Clock>> clock_clients;
  for (const auto& name : kClockNames) {
    auto clock_result = parent->incoming()->OpenService<fclock::Service>(name);
    if (clock_result.is_error()) {
      fdf::info("Failed to open clock service {}, assuming not qualcomm chipset", name);
      return nullptr;
    }
    auto clock_client = clock_result->connect_clock();
    if (clock_client.is_error()) {
      fdf::info("Failed to connect to clock {}, assuming not qualcomm chipset", name);
      return nullptr;
    }
    clock_clients[name] = std::move(*clock_client);
  }

  auto reset_result = parent->incoming()->OpenService<freset::Service>("core_reset");
  if (reset_result.is_error()) {
    fdf::info("Failed to open reset service, assuming not qualcomm chipset");
    return nullptr;
  }
  auto reset_client = reset_result->connect_reset();
  if (reset_client.is_error()) {
    fdf::info("Failed to connect to reset, assuming not qualcomm chipset");
    return nullptr;
  }

  auto regulator_result = parent->incoming()->OpenService<fvreg::Service>("dwc3-regulator");
  if (regulator_result.is_error()) {
    fdf::info("Failed to open regulator service, assuming not qualcomm chipset");
    return nullptr;
  }
  auto regulator_client = regulator_result->connect_vreg();
  if (regulator_client.is_error()) {
    fdf::info("Failed to connect to regulator, assuming not qualcomm chipset");
    return nullptr;
  }

  // Verify that the regulator is reachable before creating the platform extension.
  // A domain error (e.g. ZX_ERR_NOT_SUPPORTED from GDSC) indicates that the regulator
  // driver is present and responded. Only a framework error (such as ZX_ERR_PEER_CLOSED)
  // indicates the service is not present.
  if (fidl::Result result = fidl::Call(*regulator_client)->GetVoltageStep();
      result.is_error() && result.error_value().is_framework_error()) {
    fdf::info("Failed to query regulator, assuming not qualcomm chipset: {}", result.error_value());
    return nullptr;
  }

  auto dispatcher =
      fdf::SynchronizedDispatcher::Create(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls,
                                          "dwc3-interconnect", [](fdf_dispatcher_t*) {});
  if (dispatcher.is_error()) {
    fdf::error("Failed to create interconnect dispatcher: {}", dispatcher);
    return nullptr;
  }

  return std::make_unique<QualcommExtension>(mmio, std::move(interconnect_clients),
                                             std::move(clock_clients), std::move(*reset_client),
                                             std::move(*regulator_client), std::move(*dispatcher));
}

zx::result<> QualcommExtension::VoteBandwidth(State state) {
  TRACE_DURATION("dwc3", "QualcommExtension::VoteBandwidth", "state", static_cast<uint8_t>(state));
  if (state_ == state) {
    // Already in the correct state
    return zx::ok();
  }
  state_ = state;
  SetInterconnectBandwidths(state_);
  return zx::ok();
}

void QualcommExtension::SetInterconnectBandwidths(State state) {
  TRACE_DURATION("dwc3", "QualcommExtension::SetInterconnectBandwidths", "state",
                 static_cast<uint8_t>(state));
  static const std::unordered_map<State, std::unordered_map<BusPath, std::pair<uint32_t, uint32_t>>>
      kVoteMap = {
          {
              State::kNone,
              {
                  {BusPath::kUsbDdr, {0, 0}},
                  {BusPath::kUsbIpa, {0, 0}},
                  {BusPath::kDdrUsb, {0, 0}},
              },
          },
          {
              State::kNominal,
              {
                  {BusPath::kUsbDdr, {1'000'000, 1'250'000}},
                  {BusPath::kUsbIpa, {0, 2'400'000}},
                  {BusPath::kDdrUsb, {0, 40'000'000}},
              },
          },
          {
              State::kSvs,
              {
                  {BusPath::kUsbDdr, {240'000'000, 700'000'000}},
                  {BusPath::kUsbIpa, {0, 2'400'000}},
                  {BusPath::kDdrUsb, {0, 40'000'000}},
              },
          },
          {
              State::kMin,
              {
                  {BusPath::kUsbDdr, {1'000, 1'000}},
                  {BusPath::kUsbIpa, {1'000, 1'000}},
                  {BusPath::kDdrUsb, {1'000, 1'000}},
              },
          },
  };

  const auto& votes = kVoteMap.at(state);
  sync_completion_t completion;
  auto count = std::make_shared<std::atomic_size_t>(votes.size());

  for (const auto& [path, vote] : votes) {
    const auto& [average, peak] = vote;
    interconnect_handler_.AsyncCall(&InterconnectHandler::SetBandwidth, path, average, peak,
                                    [count, &completion]() {
                                      if (count->fetch_sub(1) == 1) {
                                        sync_completion_signal(&completion);
                                      }
                                    });
  }

  sync_completion_wait(&completion, ZX_TIME_INFINITE);
}

zx::result<> QualcommExtension::VoteVoltage(bool on) {
  TRACE_DURATION("dwc3", "QualcommExtension::VoteVoltage", "on", on);
  if (on) {
    fidl::Result enable = fidl::Call(regulator_client_)->Enable();
    if (enable.is_error()) {
      fdf::error("failed to enable regulator: {}", enable.error_value());
      return zx::error(enable.error_value().is_domain_error()
                           ? enable.error_value().domain_error()
                           : enable.error_value().framework_error().status());
    }
  } else {
    fidl::Result disable = fidl::Call(regulator_client_)->Disable();
    if (disable.is_error()) {
      fdf::error("failed to disable regulator: {}", disable.error_value());
      return zx::error(disable.error_value().is_domain_error()
                           ? disable.error_value().domain_error()
                           : disable.error_value().framework_error().status());
    }
  }

  return zx::ok();
}

zx::result<> QualcommExtension::VoteClocks(bool on) {
  TRACE_DURATION("dwc3", "QualcommExtension::VoteClocks", "on", on);
  constexpr std::array<std::string, 6> kClockNames{
      "xo", "sleep-clk", "iface-clk", "core-clk", "utmi-clk", "bus-aggr-clk",
  };

  if (on) {
    for (const std::string& name : kClockNames) {
      fidl::Result enable = fidl::Call(clock_clients_.at(name))->Enable();
      if (enable.is_error()) {
        fdf::error("could not enable clk {}: {}", name, enable.error_value());
        return zx::error(enable.error_value().is_domain_error()
                             ? enable.error_value().domain_error()
                             : enable.error_value().framework_error().status());
      }
    }
  } else {
    // Disable clocks in the opposite order.
    for (const std::string& name : kClockNames | std::views::reverse) {
      fidl::Result disable = fidl::Call(clock_clients_.at(name))->Disable();
      if (disable.is_error()) {
        fdf::error("could not disable clk {}: {}", name, disable.error_value());
        return zx::error(disable.error_value().is_domain_error()
                             ? disable.error_value().domain_error()
                             : disable.error_value().framework_error().status());
      }
    }
  }

  return zx::ok();
}

constexpr uint32_t BandwidthForSpeed(fdescriptor::UsbSpeed speed) {
  switch (speed) {
    case fdescriptor::UsbSpeed::kLow:
    case fdescriptor::UsbSpeed::kFull:
      return 2'000'000;
    case fdescriptor::UsbSpeed::kHigh:
      return 40'000'000;
    case fdescriptor::UsbSpeed::kSuper:
      return 400'000'000;
    case fdescriptor::UsbSpeed::kEnhancedSuper:
      return 1'000'000'000;
    case fdescriptor::UsbSpeed::kUndefined:
    default:
      return 0;
  }
}

class IrisExtension final : public PlatformExtension {
 public:
  // Phase 1 Iris default speed is High-Speed (40 MB/s).
  static constexpr fdescriptor::UsbSpeed kDefaultSpeed = fdescriptor::UsbSpeed::kHigh;
  static constexpr uint32_t kDefaultBandwidthBps = BandwidthForSpeed(kDefaultSpeed);

  static std::unique_ptr<IrisExtension> Create(Dwc3* parent) {
    auto interconnect_result =
        parent->incoming()->Connect<fhi::PathService::Path>("usb-interconnect");
    if (interconnect_result.is_error()) {
      return nullptr;
    }
    fidl::SyncClient<fhi::Path> client(std::move(interconnect_result.value()));
    // Probe interconnect path to ensure it is actually available before creating IrisExtension.
    fhi::BandwidthRequest request{{
        .average_bandwidth_bps = kDefaultBandwidthBps,
        .peak_bandwidth_bps = kDefaultBandwidthBps,
        .tag = 'USB ',
    }};
    if (auto result = client->SetBandwidth(request); result.is_error()) {
      fdf::info("Failed to set bandwidth on usb-interconnect, assuming not Iris platform: {}",
                result.error_value().FormatDescription());
      return nullptr;
    }
    return std::make_unique<IrisExtension>(std::move(client));
  }

  explicit IrisExtension(fidl::SyncClient<fhi::Path> interconnect_client)
      : interconnect_client_(std::move(interconnect_client)) {}

  zx::result<> Start() override {
    TRACE_DURATION("dwc3", "IrisExtension::Start");
    return VoteBandwidth(BandwidthForSpeed(speed_));
  }

  zx::result<> Suspend() override {
    TRACE_DURATION("dwc3", "IrisExtension::Suspend");
    return VoteBandwidth(0);
  }

  zx::result<> Resume() override {
    TRACE_DURATION("dwc3", "IrisExtension::Resume");
    return VoteBandwidth(BandwidthForSpeed(speed_));
  }

  zx::result<> SetConnectionSpeed(fdescriptor::UsbSpeed speed) override {
    TRACE_DURATION("dwc3", "IrisExtension::SetConnectionSpeed", "speed",
                   static_cast<uint32_t>(speed));
    speed_ = speed;
    return VoteBandwidth(BandwidthForSpeed(speed_));
  }

  bool PowersDownCoreOnDisconnect() const override { return false; }

 private:
  zx::result<> VoteBandwidth(uint32_t bandwidth) {
    if (!interconnect_client_.is_valid()) {
      return zx::ok();
    }
    fhi::BandwidthRequest request{{
        .average_bandwidth_bps = bandwidth,
        .peak_bandwidth_bps = bandwidth,
        .tag = 'USB ',
    }};
    if (auto result = interconnect_client_->SetBandwidth(request); result.is_error()) {
      fdf::error("SetBandwidth ({} bps) failed on interconnect: {}", bandwidth,
                 result.error_value().FormatDescription());
      return zx::error(result.error_value().is_domain_error()
                           ? result.error_value().domain_error()
                           : result.error_value().framework_error().status());
    }
    return zx::ok();
  }

  fdescriptor::UsbSpeed speed_{kDefaultSpeed};
  fidl::SyncClient<fhi::Path> interconnect_client_;
};

}  // namespace

zx::eventpair Dwc3::AcquireWakeLease() {
  TRACE_DURATION("dwc3", "AcquireWakeLease");
  if (!config_ || !config_->enable_suspend()) {
    return {};
  }

  auto sag_client = incoming_->Connect<fuchsia_power_system::ActivityGovernor>();
  if (sag_client.is_error()) {
    fdf::warn("Failed to connect to SystemActivityGovernor: {}.", sag_client.status_string());
    return {};
  }

  zx::eventpair client_end, server_end;
  zx_status_t status = zx::eventpair::create(0, &client_end, &server_end);
  if (status != ZX_OK) {
    fdf::error("Failed to create eventpair: {}", zx_status_get_string(status));
    return {};
  }

  auto result =
      fidl::Call(sag_client.value())
          ->AcquireWakeLeaseWithToken({{.name = "dwc3", .server_token = std::move(server_end)}});
  if (result.is_ok()) {
    return client_end;
  }

  if (result.error_value().is_framework_error()) {
    fdf::warn("Failed to acquire wake lease: {}", result.error_value());
    return {};
  }

  switch (result.error_value().domain_error()) {
    case fuchsia_power_system::AcquireWakeLeaseError::kInternal:
      fdf::warn("Failed to acquire wake lease: Internal");
      break;
    case fuchsia_power_system::AcquireWakeLeaseError::kInvalidName:
      fdf::warn("Failed to acquire wake lease: Invalid name");
      break;
    default:
      ZX_PANIC("Unknown AcquireWakeLeaseError value: %u",
               static_cast<uint32_t>(result.error_value().domain_error()));
      break;
  }

  return {};
}

zx::result<> Dwc3::Start(fdf::DriverContext context) {
  TRACE_DURATION("dwc3", "Dwc3::Start");
  config_ = context.take_config<dwc3_config::Config>();
  inspector_ = context.CreateInspector(this);
  incoming_ = context.take_incoming();

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  power_element_runner_ = context.take_power_element_runner();
#endif

  if (zx::result result = InitializeSuspend(dispatcher(), *incoming_, "dwc3"); result.is_error()) {
    fdf::error("Failed to initialize suspend: {}", result.status_string());
    return result.take_error();
  }

  auto phy_result = incoming()->Connect<fphy::Service::Device>("dwc3-phy");
  if (phy_result.is_ok()) {
    phy_ = fidl::SyncClient<fuchsia_hardware_usb_phy::UsbPhy>(std::move(phy_result.value()));
  }

  // Set up Inspect data.
  metrics_.Init();
  dwc3_root_ = inspector().root().CreateLazyNode("dwc3", [this] {
    return fpromise::make_ok_promise(this->metrics_.RecordMetrics(get_mmio(), this));
  });

  if (zx_status_t status = AcquirePDevResources(); status != ZX_OK) {
    fdf::error("AcquirePDevResources: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  // Platform extensions require platform-specific hardware mocks. Gating this on
  // bypass_platform_extension allows generic DWC3 tests to skip platform mocks.
  if (!config_->bypass_platform_extension()) {
    std::unique_ptr<PlatformExtension> extension =
        QualcommExtension::Create(this, get_mmio()->View(0));
    if (!extension) {
      extension = IrisExtension::Create(this);
    }
    if (extension) {
      if (zx::result result = extension->Start(); result.is_error()) {
        fdf::error("Failed platform extension start: {}", result);
        return result.take_error();
      }
      platform_extension_ = std::move(extension);
    }
  }

  auto watcher_result = incoming()->Connect<fphy::ConnectionWatcherService::Watcher>("dwc3-phy");
  if (watcher_result.is_ok()) {
    connection_watcher_ = fidl::Client<fuchsia_hardware_usb_phy::ConnectionWatcher>(
        std::move(watcher_result.value()), dispatcher());
  }

  // Start the hanging-get call loop.
  if (connection_watcher_.is_valid()) {
    connection_watcher_->WatchConnectStatusChanged({AcquireWakeLease()})
        .Then(fit::bind_member<&Dwc3::OnConnectStatusChanged>(this));
  }

  if (zx_status_t status = Init(); status != ZX_OK) {
    return zx::error(status);
  }

  auto dci_handler = dci_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure);

  auto serve_dci_result =
      outgoing()->AddService<fdci::UsbDciService>(fdci::UsbDciService::InstanceHandler({
          .device = std::move(dci_handler),
      }));

  if (serve_dci_result.is_error()) {
    fdf::error("Failed to add UsbDci service: {}", serve_dci_result.status_value());
    return serve_dci_result.take_error();
  }

  auto policy_handler =
      policy_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure);

  auto serve_policy_result =
      outgoing()->AddService<fpolicy::Service>(fpolicy::Service::InstanceHandler({
          .controller = std::move(policy_handler),
      }));

  if (serve_policy_result.is_error()) {
    fdf::error("Failed to add UsbPolicy service: {}", serve_policy_result.status_value());
    return serve_policy_result.take_error();
  }

  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_designware_platform::BIND_PLATFORM_DEV_VID_DESIGNWARE),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                         bind_fuchsia_designware_platform::BIND_PLATFORM_DEV_DID_DWC3),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.dci.UsbDciService"),
  };

  std::vector offers = {
      fdf::MakeOffer2<fdci::UsbDciService>(),
      fdf::MakeOffer2<fpolicy::Service>(),
  };
  std::optional mac_address_offer = mac_address_metadata_server_.CreateOffer();
  if (mac_address_offer.has_value()) {
    offers.push_back(std::move(mac_address_offer.value()));
  }
  std::optional serial_number_offer = serial_number_metadata_server_.CreateOffer();
  if (serial_number_offer.has_value()) {
    offers.push_back(std::move(serial_number_offer.value()));
  }
  std::optional usb_phy_offer = usb_phy_metadata_server_.CreateOffer();
  if (usb_phy_offer.has_value()) {
    offers.push_back(std::move(usb_phy_offer.value()));
  }

  auto child = AddChild("dwc3", properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child.status_value());
    return child.take_error();
  }
  child_ = fidl::SyncClient<fuchsia_driver_framework::NodeController>(std::move(*child));

  return zx::ok();
}

namespace {

// Helper to find a value in dictionary.
const std::optional<std::reference_wrapper<const fuchsia_driver_metadata::DictionaryValue>>
FindMetadataValue(const fuchsia_driver_metadata::Dictionary& dict, std::string_view key) {
  if (!dict.entries().has_value()) {
    return std::nullopt;
  }
  for (const fuchsia_driver_metadata::DictionaryEntry& entry : *dict.entries()) {
    if (entry.key() == key) {
      return entry.value();
    }
  }
  return std::nullopt;
}

// Helper to get an uint32 value.
zx::result<uint32_t> GetUint32(const fuchsia_driver_metadata::Dictionary& dict,
                               std::string_view key) {
  std::optional maybe_val = FindMetadataValue(dict, key);
  if (!maybe_val) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const auto& val = maybe_val->get();
  if (!val.int64().has_value()) {
    fdf::error("Metadata key '{}' is not an integer", key);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (val.int64().value() > std::numeric_limits<uint32_t>::max() || val.int64().value() < 0) {
    fdf::error("Metadata key '{}' value {} out of range for uint32", key, val.int64().value());
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  return zx::ok(static_cast<uint32_t>(val.int64().value()));
}

// Helper to get a boolean value or check property presence.
zx::result<bool> GetBool(const fuchsia_driver_metadata::Dictionary& dict, std::string_view key) {
  std::optional maybe_val = FindMetadataValue(dict, key);
  if (!maybe_val) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const auto& val = maybe_val->get();
  if (val.boolean().has_value()) {
    return zx::ok(val.boolean().value());
  }
  if (val.int64().has_value()) {
    return zx::ok(val.int64().value() != 0);
  }
  if (val.int64_vec().has_value()) {
    if (val.int64_vec().value().empty()) {
      return zx::ok(true);
    }
    return zx::ok(val.int64_vec().value().front() != 0);
  }
  fdf::error("Metadata key '{}' is not a valid boolean", key);
  return zx::error(ZX_ERR_INVALID_ARGS);
}

}  // namespace

zx::result<> Dwc3::LoadMetadata(fdf::PDev& pdev) {
  zx::result meta = pdev.GetFidlMetadata<fuchsia_driver_metadata::Dictionary>(
      "fuchsia.driver.metadata.Dictionary");
  if (meta.is_error()) {
    if (meta.error_value() == ZX_ERR_NOT_FOUND) {
      return zx::ok();
    }
    fdf::error("Failed to retrieve driver metadata: {}", meta.status_string());
    return meta.take_error();
  }

  zx::result interrupt_moderation_us_res = GetUint32(*meta, "interrupt-moderation-us");
  if (interrupt_moderation_us_res.is_error() &&
      interrupt_moderation_us_res.error_value() != ZX_ERR_NOT_FOUND) {
    return interrupt_moderation_us_res.take_error();
  }
  if (interrupt_moderation_us_res.is_ok()) {
    uint32_t value = interrupt_moderation_us_res.value();
    uint32_t imodi = value * 4;  // units of 250ns.
    if (imodi < value || imodi > std::numeric_limits<uint16_t>::max()) {
      fdf::error(
          "Metadata key 'interrupt-moderation-us' value {} out of range for 16-bit IMODI register",
          interrupt_moderation_us_res.value());
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    interrupt_moderation_ = imodi;
  } else {
    interrupt_moderation_ = 0;
  }

  zx::result fladj_res = GetUint32(*meta, "quirk-frame-length-adjustment");
  if (fladj_res.is_error() && fladj_res.error_value() == ZX_ERR_NOT_FOUND) {
    fladj_res = GetUint32(*meta, "snps,quirk-frame-length-adjustment");
  }
  if (fladj_res.is_error() && fladj_res.error_value() != ZX_ERR_NOT_FOUND) {
    return fladj_res.take_error();
  }
  if (fladj_res.is_ok()) {
    fladj_ = fladj_res.value();
    if (*fladj_ > 0x3fu) {
      fdf::info(
          "quirk-frame-length-adjustment value {:#x} exceeds maximum of 0x3f and will be masked",
          *fladj_);
    }
  } else {
    fladj_ = std::nullopt;
  }

  zx::result lpm_sel_res = GetBool(*meta, "gfladj-refclk-lpm-sel-quirk");
  if (lpm_sel_res.is_error() && lpm_sel_res.error_value() == ZX_ERR_NOT_FOUND) {
    lpm_sel_res = GetBool(*meta, "snps,gfladj-refclk-lpm-sel-quirk");
  }
  if (lpm_sel_res.is_error() && lpm_sel_res.error_value() != ZX_ERR_NOT_FOUND) {
    return lpm_sel_res.take_error();
  }
  fladj_refclk_lpm_sel_ = lpm_sel_res.value_or(false);

  return zx::ok();
}

zx_status_t Dwc3::AcquirePDevResources() {
  TRACE_DURATION("dwc3", "Dwc3::AcquirePDevResources");
  auto pdev_result = incoming()->OpenService<fpdev::Service>("pdev");
  if (pdev_result.is_error()) {
    fdf::error("incoming()->OpenService<fpdev::Service>(): {}", pdev_result.status_value());
    return pdev_result.error_value();
  }
  auto pdev_client_end = pdev_result->connect_device();
  if (pdev_client_end.is_error()) {
    fdf::error("pdev_result->connect_device(): {}", pdev_client_end.status_value());
    return pdev_client_end.error_value();
  }
  pdev_ = fdf::PDev{std::move(pdev_client_end.value())};

  if (zx::result result = LoadMetadata(pdev_); result.is_error()) {
    return result.status_value();
  }

  if (zx::result result =
          usb_phy_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev_);
      result.is_error()) {
    fdf::error("Failed to forward usb-phy metadata: {}", result);
    return result.status_value();
  }

  if (zx::result result = mac_address_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(),
                                                                       incoming(), "pdev");
      result.is_error()) {
    fdf::error("Failed to forward mac address metadata: {}", result);
    return result.status_value();
  }

  if (zx::result result = serial_number_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(),
                                                                         incoming(), "pdev");
      result.is_error()) {
    fdf::error("Failed to forward serial number metadata: {}", result);
    return result.status_value();
  }

  auto mmio = pdev_.MapMmio(0);
  if (mmio.is_error()) {
    fdf::error("MapMmio failed: {}", mmio);
    return mmio.error_value();
  }
  mmio_ = std::move(*mmio);

  auto bti = pdev_.GetBti(0);
  if (bti.is_error()) {
    fdf::error("GetBti failed: {}", bti);
    return bti.error_value();
  }
  bti_ = std::move(*bti);

  auto irq = pdev_.GetInterrupt(0);
  if (irq.is_error()) {
    fdf::error("GetInterrupt failed: {}", irq);
    return irq.error_value();
  }
  irq_ = std::move(*irq);

  return ZX_OK;
}

zx_status_t Dwc3::Init() {
  TRACE_DURATION("dwc3", "Dwc3::Init");
  // Start by identifying our hardware and making sure that we recognize it, and
  // it is a version that we know we can support.  Then, reset the hardware so
  // that we know it is in a good state.
  // Now that we have our registers, check to make sure that we are running on
  // a version of the hardware that we support.
  if (zx_status_t status = CheckHwVersion(); status != ZX_OK) {
    fdf::error("CheckHwVersion failed: {}", zx_status_get_string(status));
    return status;
  }

  // Now that we have our registers, reset the hardware.  This will ensure that
  // we are starting from a known state moving forward.
  if (zx_status_t status = ResetHw(); status != ZX_OK) {
    fdf::error("HW Reset Failed: {}", zx_status_get_string(status));
    return status;
  }

  // Finally, figure out the number of endpoints that this version of the
  // controller supports.
  auto hwparams3 = GHWPARAMS3::Get().ReadFrom(get_mmio());
  uint32_t total_eps = hwparams3.DWC_USB31_NUM_EPS();
  uint32_t num_in_eps = hwparams3.DWC_USB31_NUM_IN_EPS();

  if (total_eps < num_in_eps || total_eps < 2 || num_in_eps == 0 || (total_eps - num_in_eps) == 0 ||
      num_in_eps > 16 || (total_eps - num_in_eps) > 16 || total_eps > 32) {
    fdf::error("dwc3: Invalid hardware parameters: total_eps={}, num_in_eps={}", total_eps,
               num_in_eps);
    return ZX_ERR_NOT_SUPPORTED;
  }

  num_in_eps_ = num_in_eps;
  num_out_eps_ = total_eps - num_in_eps;

  // Cache hardware info while the core is powered on and mapped.
  {
    auto* mmio = get_mmio();
    std::vector<fuchsia_hardware_usb_dci::EndpointInfo> endpoints;
    endpoints.reserve(num_in_eps_ + num_out_eps_ - 2);

    uint32_t mdwidth = GHWPARAMS0::Get().ReadFrom(mmio).DWC_USB31_MDWIDTH();
    uint32_t ram_width_bytes = 4 << mdwidth;

    for (uint32_t i = 1; i < num_in_eps_; i++) {
      fuchsia_hardware_usb_dci::EndpointInfo ep_info;
      ep_info.ep_address(static_cast<uint8_t>(0x80 | i));

      uint32_t depth = GTXFIFOSIZ::Get(i).ReadFrom(mmio).TXFDEP();
      uint16_t max_ps = static_cast<uint16_t>(depth * ram_width_bytes);

      fuchsia_hardware_usb_dci::SupportedEndpointInfo bulk_info;
      bulk_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
      bulk_info.max_packet_size_limit(max_ps);

      fuchsia_hardware_usb_dci::SupportedEndpointInfo intr_info;
      intr_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
      intr_info.max_packet_size_limit(max_ps);

      ep_info.supported_types(std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo>{
          std::move(bulk_info),
          std::move(intr_info),
      });
      endpoints.push_back(std::move(ep_info));
    }

    for (uint32_t i = 1; i < num_out_eps_; i++) {
      fuchsia_hardware_usb_dci::EndpointInfo ep_info;
      ep_info.ep_address(static_cast<uint8_t>(i));

      fuchsia_hardware_usb_dci::SupportedEndpointInfo bulk_info;
      bulk_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
      bulk_info.max_packet_size_limit(1024);

      fuchsia_hardware_usb_dci::SupportedEndpointInfo intr_info;
      intr_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
      intr_info.max_packet_size_limit(1024);

      ep_info.supported_types(std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo>{
          std::move(bulk_info),
          std::move(intr_info),
      });
      endpoints.push_back(std::move(ep_info));
    }

    dci_hardware_info_.endpoints(std::move(endpoints));
    dci_hardware_info_.supports_dynamic_ep_sizing(false);
  }

  uint32_t ep_count = total_eps;
  if (ep_count < (kUserEndpointStartNum + 1)) {
    fdf::error("HW supports only {} physical endpoints, but at least {} are needed to operate.",
               ep_count, (kUserEndpointStartNum + 1));
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Now go ahead and allocate the user endpoint storage and uep servers.
  user_endpoints_.Init(ep_count - kUserEndpointStartNum, bti_, this);

  // Now that we have our BTI, and have reset our hardware, we can go ahead and
  // release the quarantine on any pages which may have been previously pinned
  // by this BTI.
  if (zx_status_t status = bti_.release_quarantine(); status != ZX_OK) {
    fdf::error("Release quarantine failed: {}", zx_status_get_string(status));
    return status;
  }

  // If something goes wrong after this point, make sure to release any of our
  // allocated dma buffers.
  auto cleanup = fit::defer([this]() { ReleaseResources(); });

  zx::result result = event_fifo_.Init(bti_, /*cached=*/true);
  if (result.is_error()) {
    fdf::error("Event FIFO init failed: {}", result);
    return result.error_value();
  }

  // Now that we have allocated our event buffer, we have at least one region
  // pinned.  We need to be sure to place the hardware into reset before
  // unpinning the memory during shutdown.
  has_pinned_memory_ = true;

  zx_status_t status = dma_buffer::CreateBufferFactory()->CreateContiguous(
      bti_, kEp0BufferSize, 12, dma_buffer::CacheOptions::kDisabled, &ep0_.buffer);
  if (status != ZX_OK) {
    fdf::error("ep0_buffer init failed: {}", zx_status_get_string(status));
    return status;
  }

  if (zx_status_t status = Ep0Init(); status != ZX_OK) {
    fdf::error("Ep0Init init failed: {}", zx_status_get_string(status));
    return status;
  }

  irq_handler_.set_object(irq_.get());
  irq_handler_.Begin(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  // Ack IRQ in case previous ack was hanging
  irq_.ack();

  // Things went well.  Cancel our cleanup routine.
  cleanup.cancel();
  return ZX_OK;
}

void Dwc3::ReleaseResources() {
  TRACE_DURATION("dwc3", "Dwc3::ReleaseResources");
  // If we managed to get our registers mapped, place the device into reset so
  // we are certain that there is no DMA going on in the background.
  if (mmio_.has_value() && power_on_) {
    if (zx_status_t status = ResetHw(); status != ZX_OK) {
      // Deliberately panic and terminate this driver if we fail to place the
      // hardware into reset at this point and we have any pinned memory..  We do this
      // deliberately because, if we cannot put the hardware into reset, it may still be accessing
      // pages we previously pinned using DMA.  If we are on a system with no
      // IOMMU, deliberately terminating the process will ensure that our
      // pinned pages are quarantined instead of being returned to the page
      // pool.
      if (has_pinned_memory_) {
        fdf::error(
            "Failed to place HW into reset during shutdown ({}), self-terminating in order "
            "to ensure quarantine",
            zx_status_get_string(status));
        ZX_ASSERT(false);
      }
    }

    ResetEndpoints();
  }

  // Now go ahead and release any buffers we may have pinned.
  ep0_.buffer.reset();
  ep0_.shared_fifo.Release();

  for (UserEndpoint& uep : user_endpoints_) {
    uep.fifo.Release();
  }

  event_fifo_.Release();
  has_pinned_memory_ = false;
}

zx_status_t Dwc3::CheckHwVersion() {
  TRACE_DURATION("dwc3", "Dwc3::CheckHwVersion");
  auto* mmio = get_mmio();
  auto gsnpsid = GSNPSID::Get().ReadFrom(mmio);

  auto core_id = static_cast<uint16_t>(gsnpsid.core_id());

  if (core_id == 0x5533) {
    // Major and minor versioning is in nibble-packed binary-coded-decimal format with the revision
    // encoded in hex (e.g. 0x330a decodes to 3.30a).
    const uint8_t n1 = (gsnpsid.release_number() & 0xf000) >> 12;
    const uint8_t n2 = (gsnpsid.release_number() & 0x0f00) >> 8;
    const uint8_t n3 = (gsnpsid.release_number() & 0x00f0) >> 4;
    const uint8_t n4 = (gsnpsid.release_number() & 0x000f) >> 0;

    const uint8_t major = n1;
    const uint8_t minor = n2 * 10 + n3;
    const uint8_t rev = n4;

    // Only valid on core versions 3.10a+
    // clang-format off
    poll_end_xfer_ = (
        major > 3
        || (major == 3 && minor > 10)
        || (major == 3 && minor == 10 && rev >= 0xa));

    // GFLADJ is supported on core versions 2.50a+
    supports_gfladj_ = (
        major > 2
        || (major == 2 && minor >= 50));
    // clang-format on

    fdf::info("Detected Synopsys DWC_usb3 core version {}.{:02d}{:x}", major, minor, rev);
    return ZX_OK;
  }

  if (core_id == 0x3331) {
    auto ver_num = USB31_VER_NUMBER::Get().ReadFrom(mmio);
    auto ver_type = USB31_VER_TYPE::Get().ReadFrom(mmio);

    poll_end_xfer_ = false;  // Unsupported.
    supports_gfladj_ = true;

    fdf::info("Detected Synopsys DWC_usb31 core version number 0x{:08x} type 0x{:08x}",
              ver_num.reg_value(), ver_type.reg_value());
    return ZX_OK;
  }

  fdf::error("Unsupported Synopsys core id 0x{:04x}", core_id);
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Dwc3::ResetHw() {
  TRACE_DURATION("dwc3", "Dwc3::ResetHw");
  auto* mmio = get_mmio();

  // Clear the run/stop bit and request a software reset.
  DCTL::Get().ReadFrom(mmio).set_RUN_STOP(0).set_CSFTRST(1).WriteTo(mmio);

  // HW will clear the software reset bit when it is finished with the reset
  // process.
  zx::time start = zx::clock::get_monotonic();
  while (DCTL::Get().ReadFrom(mmio).CSFTRST()) {
    if ((zx::clock::get_monotonic() - start) >= kHwResetTimeout) {
      metrics_.RecordEvent("Error: Hardware reset timed out!");
      return ZX_ERR_TIMED_OUT;
    }
  }

  // Fuchsia's dwc3.cc doesn't parse snps,incr-burst-type-adjustment yet.
  GSBUSCFG0::Get().FromValue(0).set_INCR4BRSTENA(1).WriteTo(mmio);

  if (poll_end_xfer_) {
    GUCTL2::Get().ReadFrom(mmio).set_Rst_actbitlater(1).WriteTo(mmio);
  }

  if (supports_gfladj_ && (fladj_.has_value() || fladj_refclk_lpm_sel_)) {
    auto gfladj = GFLADJ::Get().ReadFrom(mmio);
    if (fladj_.has_value()) {
      gfladj.set_GFLADJ_30MHZ(*fladj_ & 0x3f);
      gfladj.set_GFLADJ_30MHZ_SDBND_SEL(1);
    }
    if (fladj_refclk_lpm_sel_) {
      gfladj.set_GFLADJ_REFCLK_LPM_SEL(1);
    }
    gfladj.WriteTo(mmio);
  }

  return ZX_OK;
}

void Dwc3::SetDeviceAddress(uint32_t address) {
  TRACE_DURATION("dwc3", "Dwc3::SetDeviceAddress", "address", address);
  auto* mmio = get_mmio();

  DCFG::Get().ReadFrom(mmio).set_DEVADDR(address).WriteTo(mmio);

  if (address > 0) {
    SetDeviceState(fpolicy::DeviceState::kAddress, static_cast<uint8_t>(address));
  } else {
    SetDeviceState(fpolicy::DeviceState::kDefault, 0);
  }
}

void Dwc3::StartPeripheralMode() {
  TRACE_DURATION("dwc3", "Dwc3::StartPeripheralMode");
  auto* mmio = get_mmio();

  if (interrupt_moderation_ != 0) {
    // Enable interrupt moderation.
    DEVIMOD::Get(0).FromValue(0).set_IMODI(interrupt_moderation_).WriteTo(mmio);
  }

  // configure and enable PHYs
  GUSB2PHYCFG::Get(0)
      .ReadFrom(mmio)
      .set_USBTRDTIM(9)    // USB2.0 Turn-around time == 9 phy clocks
      .set_ULPIAUTORES(0)  // No auto resume
      .set_ENBLSLPM(0)     // Disable PHY suspend for stability
      .set_SUSPENDUSB20(0)
      .WriteTo(mmio);

  GUSB3PIPECTL::Get(0)
      .ReadFrom(mmio)
      .set_DELAYP1TRANS(0)
      .set_SUSPENDENABLE(0)
      .set_LFPSFILTER(1)
      .set_SS_TX_DE_EMPHASIS(1)
      .WriteTo(mmio);

  // TODO(johngro): This is the number of receive buffers.  Why do we set it to 16?
  constexpr uint32_t nump = 16;
  DCFG::Get()
      .ReadFrom(mmio)
      .set_NUMP(nump)                  // number of receive buffers
      .set_DEVSPD(DCFG::DEVSPD_SUPER)  // max speed is 5Gbps USB3.1
      .set_DEVADDR(0)                  // device address is 0
      .WriteTo(mmio);

  // Program the location of the event buffer, then enable event delivery.
  StartEvents();

  Ep0Start();

  // Set the run/stop bit to start the controller
  DCTL::Get().FromValue(0).set_RUN_STOP(1).WriteTo(mmio);
}

void Dwc3::ResetConfiguration() {
  TRACE_DURATION("dwc3", "Dwc3::ResetConfiguration");
  auto* mmio = get_mmio();
  // disable all endpoints except EP0_OUT and EP0_IN
  DALEPENA::Get().FromValue(0).EnableEp(kEp0Out).EnableEp(kEp0In).WriteTo(mmio);

  for (UserEndpoint& uep : user_endpoints_) {
    // Disabled above.
    uep.ep.enabled = false;
    uep.ep.stalled = false;
    UserEpReset(uep);
  }

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    dci_intf_.buffer(arena)->SetConnected(true).Then(
        [](fidl::WireUnownedResult<fuchsia_hardware_usb_dci::UsbDciInterface::SetConnected>&
               result) {
          if (!result.ok()) {
            fdf::error("(framework) SetConnected() (ResetConfiguration): {}",
                       result.FormatDescription());
          } else if (result->is_error()) {
            fdf::error("SetConnected(): {}", zx_status_get_string(result->error_value()));
          }
        });
  }

  if (phy_.is_valid()) {
    if (fidl::Result result = phy_->ConnectStatusChanged({{.connected = true, .wake_lease = {}}});
        result.is_error()) {
      fdf::warn("Call to ConnectStatusChanged on USB phy failed: {}", result.error_value());
    }
  }
}

void Dwc3::HandleResetEvent() {
  TRACE_DURATION("dwc3", "Dwc3::HandleResetEvent");
  fdf::info("Dwc3::HandleResetEvent");

  ResetEndpoints();
  SetDeviceAddress(0);
  Ep0Start();

  connection_speed_ = fdescriptor::UsbSpeed::kUndefined;

  SetDeviceState(fpolicy::DeviceState::kDefault);

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    dci_intf_.buffer(arena)->SetConnected(false).Then(
        [](fidl::WireUnownedResult<fuchsia_hardware_usb_dci::UsbDciInterface::SetConnected>&
               result) {
          if (!result.ok()) {
            fdf::error("(framework) SetConnected() (HandleResetEvent): {}",
                       result.FormatDescription());
          } else if (result->is_error()) {
            fdf::error("SetConnected(): {}", zx_status_get_string(result->error_value()));
          }
        });
  }
}

void Dwc3::HandleConnectionDoneEvent() {
  TRACE_DURATION("dwc3", "Dwc3::HandleConnectionDoneEvent");
  uint16_t ep0_max_packet = 0;
  fdescriptor::wire::UsbSpeed new_speed{fdescriptor::UsbSpeed::kUndefined};
  metrics_.RecordEvent("USB Reset Complete. Setting up control endpoint.");

  auto* mmio = get_mmio();

  uint32_t speed = DSTS::Get().ReadFrom(mmio).CONNECTSPD();

  switch (speed) {
    case DSTS::CONNECTSPD_HIGH:
      new_speed = fdescriptor::UsbSpeed::kHigh;
      ep0_max_packet = 64;
      break;
    case DSTS::CONNECTSPD_FULL:
      new_speed = fdescriptor::UsbSpeed::kFull;
      ep0_max_packet = 64;
      break;
    case DSTS::CONNECTSPD_SUPER:
      new_speed = fdescriptor::UsbSpeed::kSuper;
      ep0_max_packet = 512;
      break;
    case DSTS::CONNECTSPD_ENHANCED_SUPER:
      new_speed = fdescriptor::UsbSpeed::kEnhancedSuper;
      ep0_max_packet = 512;
      break;
    default:
      fdf::error("unsupported speed {}", speed);
      break;
  }

  if (ep0_max_packet) {
    std::array eps{&ep0_.out, &ep0_.in};
    for (Endpoint* ep : eps) {
      ep->type = fdescriptor::EndpointType::kControl;
      ep->interval = 0;
      ep->max_packet_size = ep0_max_packet;
      CmdEpSetConfig(*ep, true);
    }
    ep0_.cur_speed = new_speed;
    connection_speed_ = new_speed;
    if (platform_extension_) {
      if (zx::result result = platform_extension_->SetConnectionSpeed(connection_speed_);
          result.is_error()) {
        fdf::error("platform_extension_->SetConnectionSpeed failed: {}", result.status_string());
      }
    }
  }

  std::ostringstream buf;
  buf << "USB Connection Done (Speed: "
      << fidl::ostream::Formatted<fdescriptor::UsbSpeed>(new_speed) << ")";
  metrics_.RecordEvent(buf.str());

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    dci_intf_.buffer(arena)->SetSpeed(new_speed).Then(
        [](fidl::WireUnownedResult<fuchsia_hardware_usb_dci::UsbDciInterface::SetSpeed>& result) {
          if (!result.ok()) {
            fdf::error("(framework) SetSpeed(): {}", result.FormatDescription());
          } else if (result->is_error()) {
            fdf::error("SetSpeed(): {}", zx_status_get_string(result->error_value()));
          }
        });
  }
}

void Dwc3::HandleDisconnectedEvent() {
  TRACE_DURATION("dwc3", "Dwc3::HandleDisconnectedEvent");
  fdf::info("Dwc3::HandleDisconnectedEvent");

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    dci_intf_.buffer(arena)->SetConnected(false).Then(
        [](fidl::WireUnownedResult<fuchsia_hardware_usb_dci::UsbDciInterface::SetConnected>&
               result) {
          if (!result.ok()) {
            fdf::error("(framework) SetConnected() (HandleDisconnect): {}",
                       result.FormatDescription());
          } else if (result->is_error()) {
            fdf::error("SetConnected(): {}", zx_status_get_string(result->error_value()));
          }
        });
  }

  ResetEndpoints();

  connection_speed_ = fdescriptor::UsbSpeed::kUndefined;
  if (platform_extension_) {
    if (zx::result result = platform_extension_->SetConnectionSpeed(connection_speed_);
        result.is_error()) {
      fdf::warn("Failed to set connection speed on disconnect event: {}", result.status_string());
    }
  }

  if (phy_.is_valid()) {
    if (fidl::Result result = phy_->ConnectStatusChanged({{.connected = false, .wake_lease = {}}});
        result.is_error()) {
      fdf::warn("Call to ConnectStatusChanged on USB phy failed: {}", result.error_value());
    }
  }
}

Dwc3::~Dwc3() {
  fdf::debug("~Dwc3()");
  irq_handler_.Cancel();
  ReleaseResources();

  // The OnUnbound() handler for each endpoint server should have already called zx_bti_unpin() for
  // each registered VMO. To guard against a crashed or stalled dispatcher, release all page
  // quarantines. The hardware is stopped and no further DMA transactions are scheduled.
  if (bti_.is_valid()) {
    zx_status_t status = bti_.release_quarantine();
    if (status != ZX_OK) {
      fdf::error("Failed to release page quarantine ({})", zx_status_get_string(status));
    }
  }

  controller_started_ = false;
}

void Dwc3::Stop(fdf::StopCompleter completer) {
  TRACE_DURATION("dwc3", "Dwc3::Stop");
  fdf::info("Dwc3::Stop called");
  dwc3_root_ = {};
  dci_bindings_.RemoveAll();
  policy_bindings_.RemoveAll();
  completer(zx::ok());
}

void Dwc3::Suspend(fdf_power::SuspendCompleter completer) {
  TRACE_DURATION("dwc3", "Dwc3::Suspend");
  if (platform_extension_) {
    if (zx::result result = platform_extension_->Suspend(); result.is_error()) {
      fdf::error("platform_extension_->Suspend() failed: {}", result.status_string());
    }
  }
  completer();
}

void Dwc3::Resume(fdf_power::ResumeCompleter completer) {
  TRACE_DURATION("dwc3", "Dwc3::Resume");
  if (platform_extension_) {
    if (zx::result result = platform_extension_->Resume(); result.is_error()) {
      fdf::error("platform_extension_->Resume() failed: {}", result.status_string());
    }
  }
  completer();
}

bool Dwc3::SuspendEnabled() {
  TRACE_INSTANT("dwc3", "Dwc3::SuspendEnabled", TRACE_SCOPE_THREAD);
  return false;
}

void Dwc3::ConnectToEndpoint(ConnectToEndpointRequest& request,
                             ConnectToEndpointCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::ConnectToEndpoint");
  UserEndpoint* uep{get_user_endpoint(UsbAddressToEpNum(request.ep_addr()))};
  if (uep == nullptr || !uep->server.has_value()) {
    completer.Reply(fit::as_error(ZX_ERR_INVALID_ARGS));
    return;
  }

  uep->server->Connect(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(request.ep()));
  completer.Reply(fit::ok());
}

void Dwc3::SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::SetInterface");
  if (!request.interface().is_valid()) {
    fdf::error("Interface should be valid");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (dci_intf_.is_valid()) {
    fdf::error("{}: DCI Interface already set", __func__);
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  dci_intf_.Bind(std::move(request.interface()), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  completer.Reply(zx::ok());
}

void Dwc3::StartController(StartControllerCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::StartController");
  controller_started_ = true;

  if (is_active()) {
    StartPeripheralMode();
  }

  completer.Reply(zx::ok());
}

void Dwc3::StopController(StopControllerCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::StopController");
  controller_started_ = false;
  ResetEndpoints();

  if (power_on_) {
    StopEvents();

    zx_status_t status = ResetHw();
    if (status != ZX_OK) {
      fdf::error("Failed to reset hardware {}", zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(50)));
  }

  completer.Reply(zx::ok());
}

void Dwc3::ConfigureEndpoint(ConfigureEndpointRequest& request,
                             ConfigureEndpointCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::ConfigureEndpoint");
  if (!power_on_) {
    completer.Reply(zx::error(ZX_ERR_IO_NOT_PRESENT));
    return;
  }

  const uint8_t ep_num = UsbAddressToEpNum(request.ep_descriptor().b_endpoint_address());
  UserEndpoint* const uep = get_user_endpoint(ep_num);

  if (uep == nullptr) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  auto ep_type = usb_ep_type2(request.ep_descriptor());

  if (ep_type == fdescriptor::EndpointType::kIsochronous) {
    fdf::error("isochronous endpoints are not supported");
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  if (uep->ep.enabled) {
    // Endpoint already configured, nothing to do.
    fdf::error("Endpoint({}) already configured!", uep->ep.ep_num);
    completer.Reply(zx::ok());
    return;
  }

  uint16_t max_packet_size = usb_ep_max_packet2(request.ep_descriptor());

  auto* const mmio = get_mmio();
  const uint32_t mdwidth = GHWPARAMS0::Get().ReadFrom(mmio).DWC_USB31_MDWIDTH();
  const uint32_t ram_width_bytes = 4 << mdwidth;
  uint32_t depth = 0;

  const bool is_in =
      usb_ep_direction2(request.ep_descriptor().b_endpoint_address()) == USB_ENDPOINT_IN;
  if (is_in) {
    // IN endpoint.
    const uint8_t fifo_num = ep_num >> 1;
    const uint32_t num_in_eps = GHWPARAMS3::Get().ReadFrom(mmio).DWC_USB31_NUM_IN_EPS();
    if (fifo_num >= num_in_eps) {
      fdf::error("dwc3: endpoint {} mapped to invalid FIFO {}", ep_num, fifo_num);
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    depth = GTXFIFOSIZ::Get(fifo_num).ReadFrom(mmio).TXFDEP();
  } else {
    // OUT endpoint.
    depth = GRXFIFOSIZ::Get(0).ReadFrom(mmio).RXFDEP();
  }

  const uint64_t fifo_size = static_cast<uint64_t>(depth) * ram_width_bytes;
  if (max_packet_size > fifo_size) {
    fdf::error("dwc3: endpoint {} max packet size {} exceeds {} FIFO size {}", ep_num,
               max_packet_size, is_in ? "TX" : "RX", fifo_size);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  // TODO(https://fxbug.dev/527137302): When queuing many TRBs we need to use uncached memory
  // because we don't currently have a way to align TRB dumps to cache line
  // sizes.
  const bool cached = !AllowEnqueueManyTRBs(ep_type);
  if (zx::result result = uep->fifo.Init(bti_, cached); result.is_error()) {
    fdf::error("fifo init failed {}", result);
    completer.Reply(result.take_error());
    return;
  }

  uep->ep.max_packet_size = max_packet_size;
  uep->ep.type = ep_type;
  uep->ep.interval = request.ep_descriptor().b_interval();
  uep->ep.usb_endpoint_address = request.ep_descriptor().b_endpoint_address();
  // TODO(voydanoff) USB3 support

  EpSetConfig(uep->ep, true);
  UserEpQueueNext(*uep);

  completer.Reply(zx::ok());
}

void Dwc3::DisableEndpoint(DisableEndpointRequest& request,
                           DisableEndpointCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::DisableEndpoint");
  if (!power_on_) {
    completer.Reply(zx::error(ZX_ERR_IO_NOT_PRESENT));
    return;
  }

  const uint8_t ep_num = UsbAddressToEpNum(request.ep_address());
  UserEndpoint* const uep = get_user_endpoint(ep_num);

  if (uep == nullptr) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  uep->server->CancelAll(ZX_ERR_IO_NOT_PRESENT);
  EpSetConfig(uep->ep, false);

  completer.Reply(zx::ok());
}

void Dwc3::EndpointSetStall(EndpointSetStallRequest& request,
                            EndpointSetStallCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::EndpointSetStall");
  if (!power_on_) {
    completer.Reply(zx::error(ZX_ERR_IO_NOT_PRESENT));
    return;
  }

  const uint8_t ep_num = UsbAddressToEpNum(request.ep_address());
  UserEndpoint* const uep = get_user_endpoint(ep_num);

  if (uep == nullptr) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (zx_status_t status = EpSetStall(uep->ep, true); status != ZX_OK) {
    completer.Reply(zx::error(status));
  } else {
    completer.Reply(zx::ok());
  }
}

void Dwc3::EndpointClearStall(EndpointClearStallRequest& request,
                              EndpointClearStallCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::EndpointClearStall");
  if (!power_on_) {
    completer.Reply(zx::error(ZX_ERR_IO_NOT_PRESENT));
    return;
  }

  const uint8_t ep_num = UsbAddressToEpNum(request.ep_address());
  UserEndpoint* const uep = get_user_endpoint(ep_num);

  if (uep == nullptr) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (zx_status_t status = EpSetStall(uep->ep, false); status != ZX_OK) {
    completer.Reply(zx::error(status));
  } else {
    UserEpQueueNext(*uep);
    completer.Reply(zx::ok());
  }
}

void Dwc3::CancelAll(CancelAllRequest& request, CancelAllCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::CancelAll");
  const uint8_t ep_num = UsbAddressToEpNum(request.ep_address());
  UserEndpoint* const uep = get_user_endpoint(ep_num);

  if (uep == nullptr) {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  uep->server->CancelAll(ZX_ERR_IO_REFUSED);
  completer.Reply(zx::ok());
}

void Dwc3::EpServer::GetInfo(GetInfoCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::EpServer::GetInfo");
  auto info{fendpoint::EndpointInfo::WithControl(fendpoint::ControlEndpointInfo{})};

  switch (uep_->ep.type) {
    case fdescriptor::EndpointType::kControl:
      // Set up above.
      break;
    case fdescriptor::EndpointType::kIsochronous: {
      fendpoint::IsochronousEndpointInfo isoc;
      isoc.lead_time(1);
      isoc.supports_scatter_gather(false);
      info.isochronous(std::move(isoc));
      break;
    }
    case fdescriptor::EndpointType::kBulk:
      info.bulk(fendpoint::BulkEndpointInfo().supports_scatter_gather(false));
      break;
    case fdescriptor::EndpointType::kInterrupt:
      info.interrupt(fendpoint::InterruptEndpointInfo().supports_scatter_gather(false));
      break;
    default:
      // In theory, this should never happen unless a new EP type is added to the spec.
      fdf::error("unknown usb endpoint type: 0x{:x}", static_cast<uint8_t>(uep_->ep.type));
      completer.Reply(zx::error(ZX_ERR_BAD_STATE));
  }

  completer.Reply(zx::ok(std::move(info)));
}

void Dwc3::EpServer::QueueRequests(QueueRequestsRequest& request,
                                   QueueRequestsCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::EpServer::QueueRequests");
  if (!uep_->ep.enabled || !dwc3_->power_on()) {
    // If the device is powered on, the rejection must be due to the endpoint being disabled.
    if (dwc3_->power_on()) {
      fdf::debug(
          "Dwc3: ep({}) QueueRequests rejected: endpoint disabled (expected during teardown or stack reset)",
          uep_->ep.ep_num);
    } else {
      fdf::debug("Dwc3: ep({}) QueueRequests rejected: power off", uep_->ep.ep_num);
    }
    for (auto& req : request.req()) {
      RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, usb::FidlRequest{std::move(req)},
                      /*send_now=*/false);
    }
    SendCompletions();
    return;
  }

  for (auto& req : request.req()) {
    usb::FidlRequest freq{std::move(req)};

    if (freq->data()->size() != 1) {
      fdf::error("scatter-gather not implemented");
      RequestComplete(ZX_ERR_NOT_SUPPORTED, 0, std::move(freq));
      continue;
    }

    if (uep_->ep.IsOutput()) {
      // Dig the length out of the request data block.
      size_t length = freq->data()->at(0).size().value();

      if (length == 0 || (length % uep_->ep.max_packet_size) != 0) {
        fdf::error("Dwc3: OUT transfers must be multiple of max packet size (len {} mps {})",
                   length, uep_->ep.max_packet_size);
        RequestComplete(ZX_ERR_INVALID_ARGS, 0, std::move(freq));
        continue;
      }
    }

    queued_reqs.emplace(std::move(freq));
  }

  dwc3_->UserEpQueueNext(*uep_);
}

void Dwc3::EpServer::CancelAll(CancelAllCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "Dwc3::EpServer::CancelAll");
  CancelAll(ZX_ERR_CANCELED);
  if (!dwc3_->controller_started_ ||
      (uep_->ep.transfer_state == Endpoint::TransferState::kIdle && active_reqs.empty())) {
    completer.Reply(zx::ok());
  } else {
    cancel_completers.push_back(completer.ToAsync());
  }
}

void Dwc3::EpReset(Endpoint& ep) {
  TRACE_DURATION("dwc3", "Dwc3::EpReset", "ep_num", ep.ep_num);
  if (!power_on_) {
    return;
  }

  EpSetStall(ep, false);
  EpSetConfig(ep, false);
  ep.got_not_ready = false;
}

void Dwc3::UserEpReset(UserEndpoint& uep) {
  TRACE_DURATION("dwc3", "Dwc3::UserEpReset", "ep_num", uep.ep.ep_num);
  uep.server->CancelAll(ZX_ERR_IO_NOT_PRESENT);
  EpReset(uep.ep);
}

void Dwc3::Ep0Reset() {
  TRACE_DURATION("dwc3", "Dwc3::Ep0Reset");
  if (ep0_.out.TransferStateIsActive() && ep0_.out.rsrc_id != Endpoint::kInvalidResourceId) {
    CmdEpEndTransfer(ep0_.out);
  }
  if (ep0_.in.TransferStateIsActive() && ep0_.in.rsrc_id != Endpoint::kInvalidResourceId) {
    CmdEpEndTransfer(ep0_.in);
  }
  EpReset(ep0_.out);
  EpReset(ep0_.in);
  ep0_.cur_setup = {};
  ep0_.cur_speed = fuchsia_hardware_usb_descriptor::wire::UsbSpeed::kUndefined;
  ep0_.state = Ep0::State::None;
  ep0_.shared_fifo.Clear();
}

void Dwc3::ResetEndpoints() {
  TRACE_DURATION("dwc3", "Dwc3::ResetEndpoints");
  Ep0Reset();
  for (UserEndpoint& uep : user_endpoints_) {
    UserEpReset(uep);
  }
}

void Dwc3::OnConnectStatusChanged(
    fidl::Result<fuchsia_hardware_usb_phy::ConnectionWatcher::WatchConnectStatusChanged>& result) {
  TRACE_DURATION("dwc3", "Dwc3::OnConnectStatusChanged");

  if (result.is_error() && result.error_value().is_framework_error()) {
    // Something happened to the FIDL connection, so don't make repeated calls.
    return;
  }

  zx::eventpair wake_lease{};
  auto next_call = fit::defer([&]() {
    TRACE_DURATION("dwc3", "WatchConnectStatusChanged");
    connection_watcher_->WatchConnectStatusChanged({std::move(wake_lease)})
        .Then(fit::bind_member<&Dwc3::OnConnectStatusChanged>(this));
  });

  if (result.is_ok()) {
    // TODO: b/550476536 to revert this workaround
    // Workaround: max77779 PMIC does not yet provide wake lease via USB PHY driver on connection
    // This hack comments the assert message to bypass the same.
    // ZX_DEBUG_ASSERT_MSG(!config_->enable_suspend() || result->wake_lease().is_valid(),
    //                    "USB phy driver did not provide a wake lease");
    wake_lease = std::move(result->wake_lease());
    // TODO: b/550476536 to revert this workaround
    // Workaround: max77779 PMIC does not yet provide wake lease via USB PHY driver on connection
    // This hack forces DWC3 to generate its own lease to prevent suspends while plugged in.
    if (!wake_lease.is_valid()) {
      wake_lease = AcquireWakeLease();
    }
  } else {
    fdf::error("WatchConnectStatusChanged returned {}",
               zx_status_get_string(result.error_value().domain_error()));
    // Best-effort error recovery for domain errors.
    wake_lease = AcquireWakeLease();
    return;
  }

  if (result->connected()) {
    fdf::debug("OnConnectStatusChanged: now connected");

    if (platform_extension_) {
      if (zx::result result = platform_extension_->Resume(); result.is_error()) {
        return;
      }
      if (zx::result result = platform_extension_->SetConnectionSpeed(fdescriptor::UsbSpeed::kHigh);
          result.is_error()) {
        fdf::warn("Failed to set provisional speed on connect: {}", result.status_string());
      }
    }
    if (!power_on_ &&
        (!platform_extension_ || !platform_extension_->PowersDownCoreOnDisconnect())) {
      // For platforms that do not power down the core via external reset controller,
      // perform a software core reset (CSFTRST) when transitioning from powered-off to
      // ensure the controller state machine and internal FIFOs start in a clean state.
      if (zx_status_t status = ResetHw(); status != ZX_OK) {
        fdf::error("Failed to reset hardware on connect: {}", zx_status_get_string(status));
        return;
      }
    }
    power_on_ = true;
    metrics_.RecordEvent("Power On / Resume (PHY Resumed)");

    SetDeviceState(fpolicy::DeviceState::kPowered);

    if (is_active()) {
      StartPeripheralMode();
    }

    if (wake_lease.is_valid()) {
      zx_status_t status = wake_lease.duplicate(ZX_RIGHT_SAME_RIGHTS, &connection_lease_);
      if (status != ZX_OK) {
        fdf::error("Failed to duplicate wake lease: {}", zx_status_get_string(status));
      }
    }
  } else {
    fdf::debug("OnConnectStatusChanged: now disconnected");

    // Cancel all pending requests.
    ResetEndpoints();

    connection_speed_ = fdescriptor::UsbSpeed::kUndefined;
    if (platform_extension_) {
      if (zx::result result = platform_extension_->SetConnectionSpeed(connection_speed_);
          result.is_error()) {
        fdf::warn("Failed to set connection speed on disconnect: {}", result.status_string());
      }
    }

    if (controller_started_) {
      StopEvents();
    }

    SetDeviceState(fpolicy::DeviceState::kNotAttached);

    if (platform_extension_) {
      if (zx::result result = platform_extension_->Suspend(); result.is_error()) {
        return;
      }
    }
    if (power_on_ && (!platform_extension_ || !platform_extension_->PowersDownCoreOnDisconnect())) {
      // For platforms that do not power down the core via external reset controller,
      // perform a software core reset on disconnect while still powered to safely halt
      // active DMA transfers and deassert level-triggered interrupts.
      if (zx_status_t status = ResetHw(); status != ZX_OK) {
        fdf::warn("Failed to reset hardware on disconnect: {}", zx_status_get_string(status));
      }
    }
    power_on_ = false;
    metrics_.RecordEvent("Power Off / Suspend (PHY Suspended)");
  }

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    dci_intf_.buffer(arena)
        ->SetConnected(result->connected())
        .Then([](fidl::WireUnownedResult<fuchsia_hardware_usb_dci::UsbDciInterface::SetConnected>&
                     result) {
          if (!result.ok()) {
            fdf::error("(framework) SetConnected() (OnConnectStatusChanged): {}",
                       result.FormatDescription());
          } else if (result->is_error()) {
            fdf::error("SetConnected(): {}", zx_status_get_string(result->error_value()));
          }
        });
  }

  if (!power_on_) {
    connection_lease_.reset();
  }
}

void Dwc3::WatchDeviceState(WatchDeviceStateCompleter::Sync& completer) {
  TRACE_DURATION("dwc3", "{}", __func__);

  if (has_new_device_state_) {
    // If new state is ready, reply immediately.
    fdf::info("{} New state is available. Replying with {}", __func__, device_state_);
    completer.Reply(
        zx::ok(fpolicy::DeviceStateUpdate{{.state = device_state_, .address = assigned_address_}}));
    has_new_device_state_ = false;
  } else {
    // Otherwise, "hang" the get.
    pending_completers_.push_back(completer.ToAsync());
  }
}

void Dwc3::SetDeviceState(fpolicy::DeviceState state) { SetDeviceState(state, assigned_address_); }

void Dwc3::SetDeviceState(fpolicy::DeviceState state, uint8_t address) {
  TRACE_DURATION("dwc3", "{}", __func__);

  device_state_ = state;
  assigned_address_ = address;

  fdf::info("{}({}, {})", __func__, state, address);

  // Provide the state to the USB Policy Manager
  // Check if there is a hanging client waiting
  if (pending_completers_.size() > 0) {
    for (auto& completer : pending_completers_) {
      fdf::info("{} have a pending completer - sending {}", __func__, state);
      completer.Reply(zx::ok(fpolicy::DeviceStateUpdate{{.state = state, .address = address}}));
    }
    pending_completers_.clear();
    has_new_device_state_ = false;
  } else {
    fdf::info("{} no pending completer", __func__);
    has_new_device_state_ = true;
  }
}

void Dwc3::GetHardwareInfo(GetHardwareInfoCompleter::Sync& completer) {
  if (power_on_) {
    auto* mmio = get_mmio();
    std::vector<fuchsia_hardware_usb_dci::EndpointInfo> endpoints;
    endpoints.reserve(num_in_eps_ + num_out_eps_ - 2);

    uint32_t mdwidth = GHWPARAMS0::Get().ReadFrom(mmio).DWC_USB31_MDWIDTH();
    uint32_t ram_width_bytes = 4 << mdwidth;

    for (uint32_t i = 1; i < num_in_eps_; i++) {
      fuchsia_hardware_usb_dci::EndpointInfo ep_info;
      ep_info.ep_address(static_cast<uint8_t>(0x80 | i));

      uint32_t depth = GTXFIFOSIZ::Get(i).ReadFrom(mmio).TXFDEP();
      uint16_t max_ps = static_cast<uint16_t>(depth * ram_width_bytes);

      fuchsia_hardware_usb_dci::SupportedEndpointInfo bulk_info;
      bulk_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
      bulk_info.max_packet_size_limit(max_ps);

      fuchsia_hardware_usb_dci::SupportedEndpointInfo intr_info;
      intr_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
      intr_info.max_packet_size_limit(max_ps);

      ep_info.supported_types(std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo>{
          std::move(bulk_info),
          std::move(intr_info),
      });
      endpoints.push_back(std::move(ep_info));
    }

    for (uint32_t i = 1; i < num_out_eps_; i++) {
      fuchsia_hardware_usb_dci::EndpointInfo ep_info;
      ep_info.ep_address(static_cast<uint8_t>(i));

      fuchsia_hardware_usb_dci::SupportedEndpointInfo bulk_info;
      bulk_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
      bulk_info.max_packet_size_limit(1024);

      fuchsia_hardware_usb_dci::SupportedEndpointInfo intr_info;
      intr_info.endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
      intr_info.max_packet_size_limit(1024);

      ep_info.supported_types(std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo>{
          std::move(bulk_info),
          std::move(intr_info),
      });
      endpoints.push_back(std::move(ep_info));
    }

    dci_hardware_info_.endpoints(std::move(endpoints));
    dci_hardware_info_.supports_dynamic_ep_sizing(false);
  }

  fuchsia_hardware_usb_dci::UsbDciGetHardwareInfoResponse response;
  response.info(dci_hardware_info_);
  completer.Reply(zx::ok(std::move(response)));
}

void Dwc3::AllocEndpoint(AllocEndpointRequest& request, AllocEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Dwc3::FreeEndpoint(FreeEndpointRequest& request, FreeEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

}  // namespace dwc3

FUCHSIA_DRIVER_EXPORT2(dwc3::Dwc3);
