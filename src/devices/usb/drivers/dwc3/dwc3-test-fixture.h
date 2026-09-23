// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_TEST_FIXTURE_H_
#define SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_TEST_FIXTURE_H_

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clock/cpp/test_base.h>
#include <fidl/fuchsia.hardware.interconnect/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.reset/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.dci/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <fidl/fuchsia.hardware.vreg/cpp/test_base.h>
#include <lib/driver/fake-clock/cpp/fake-clock.h>
#include <lib/driver/fake-reset/cpp/fake-reset.h>
#include <lib/driver/fake-vreg/cpp/fake-vreg.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>

#include <algorithm>
#include <mutex>
#include <optional>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <gtest/gtest.h>

#include "lib/driver/fake-platform-device/cpp/fake-pdev.h"
#include "lib/driver/testing/cpp/driver_test.h"
#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"
#include "src/devices/usb/drivers/dwc3/dwc3_config.h"

namespace dwc3 {

namespace fclock = fuchsia_hardware_clock;
namespace fhi = fuchsia_hardware_interconnect;
namespace fpdev = fuchsia_hardware_platform_device;
namespace fphy = fuchsia_hardware_usb_phy;
namespace freset = fuchsia_hardware_reset;
namespace fvreg = fuchsia_hardware_vreg;

class Dwc3TestHelper {
 public:
  using State = Dwc3::Ep0::State;
  using TransferState = Dwc3::Endpoint::TransferState;
  static void HandleEp0TransferCompleteEvent(Dwc3& drv, uint8_t ep_num) {
    if (!drv.ep0_.shared_fifo.IsEmpty()) {
      dwc3_trb_t* trb = drv.ep0_.shared_fifo.current_read();
      trb->control &= ~TRB_HWO;
    }
    drv.HandleEp0TransferCompleteEvent(ep_num);
  }

  static void SimulateGhostTransferCompleteEvent(Dwc3& drv, uint8_t ep_num) {
    drv.HandleEp0TransferCompleteEvent(ep_num);
  }
  static void HandleEp0TransferNotReadyEvent(Dwc3& drv, uint8_t ep_num, uint32_t stage) {
    drv.HandleEp0TransferNotReadyEvent(ep_num, stage);
  }
  static void SetEp0State(Dwc3& drv, Dwc3::Ep0::State state) { drv.ep0_.state = state; }
  static void Ep0Reset(Dwc3& drv) { drv.Ep0Reset(); }
  static void Ep0QueueSetup(Dwc3& drv) { drv.Ep0QueueSetup(); }
  static Dwc3::Ep0::State GetEp0State(Dwc3& drv) { return drv.ep0_.state; }
  static fuchsia_hardware_usb_policy::wire::DeviceState GetDeviceState(Dwc3& drv) {
    return drv.device_state_;
  }
  static void SetPowerOn(Dwc3& drv, bool power_on) { drv.power_on_ = power_on; }
  static fuchsia_hardware_usb_descriptor::UsbSpeed GetConnectionSpeed(const Dwc3& drv) {
    return drv.connection_speed_;
  }
  static void SetConnectionSpeed(Dwc3& drv, fuchsia_hardware_usb_descriptor::UsbSpeed speed) {
    drv.connection_speed_ = speed;
  }
  static void SetDriverStopping(Dwc3& drv, bool driver_stopping) {
    // Stubbed out: driver_stopping_ is not in production yet.
    (void)drv;
    (void)driver_stopping;
  }
  static void SetDeviceState(Dwc3& drv, fuchsia_hardware_usb_policy::DeviceState state) {
    drv.SetDeviceState(state);
  }
  static PlatformExtension* GetPlatformExtension(Dwc3& drv) {
    return drv.platform_extension_.get();
  }
  static void HandleEvent(Dwc3& drv, uint32_t event) { drv.HandleEvent(event); }
  static void CmdEpSetConfig(Dwc3& drv, Dwc3::Endpoint& ep, bool modify) {
    drv.CmdEpSetConfig(ep, modify);
  }
  static void CmdEpStartTransfer(Dwc3& drv, Dwc3::Endpoint& ep, zx_paddr_t trb_phys) {
    drv.CmdEpStartTransfer(ep, trb_phys);
  }
  static void CmdEpEndTransfer(Dwc3& drv, Dwc3::Endpoint& ep) { drv.CmdEpEndTransfer(ep); }
  static void ForceCmdEpEndTransfer(Dwc3& drv, uint8_t ep_num) {
    // Stubbed out: ForceCmdEpEndTransfer is not in production yet.
    (void)drv;
    (void)ep_num;
  }
  static void CmdEpSetStall(Dwc3& drv, Dwc3::Endpoint& ep) { drv.CmdEpSetStall(ep); }
  static void CmdEpClearStall(Dwc3& drv, Dwc3::Endpoint& ep) { drv.CmdEpClearStall(ep); }
  static void CmdStartNewConfig(Dwc3& drv, Dwc3::Endpoint& ep, uint32_t rsrc_id_base) {
    drv.CmdStartNewConfig(ep, rsrc_id_base);
  }
  static void CmdEpTransferConfig(Dwc3& drv, Dwc3::Endpoint& ep) { drv.CmdEpTransferConfig(ep); }
  static zx::result<> InitFifo(Dwc3& drv, uint8_t ep_num, bool cached = true) {
    auto* uep = drv.get_user_endpoint(ep_num);
    if (!uep)
      return zx::error(ZX_ERR_NOT_FOUND);
    return uep->fifo.Init(drv.bti_, cached);
  }
  static void HandleEpTransferCompleteEvent(Dwc3& drv, uint8_t ep_num) {
    drv.HandleEpTransferCompleteEvent(ep_num);
    if (ep_num >= 2) {
      if (auto* uep = drv.get_user_endpoint(ep_num); uep && uep->server.has_value()) {
        uep->server->SendCompletions();
      }
    }
  }
  static void HandleEpTransferInProgressEvent(Dwc3& drv, uint8_t ep_num) {
    drv.HandleEpTransferInProgressEvent(ep_num);
    if (ep_num >= 2) {
      if (auto* uep = drv.get_user_endpoint(ep_num); uep && uep->server.has_value()) {
        uep->server->SendCompletions();
      }
    }
  }
  static void HandleEpTransferStartedEvent(Dwc3& drv, uint8_t ep_num, uint32_t rsrc_id) {
    drv.HandleEpTransferStartedEvent(ep_num, rsrc_id);
  }
  static void HandleEpTransferNotReadyEvent(Dwc3& drv, uint8_t ep_num, uint32_t stage) {
    drv.HandleEpTransferNotReadyEvent(ep_num, stage);
  }
  static void HandleEpTransferEndedEvent(Dwc3& drv, uint8_t ep_num) {
    drv.HandleEpTransferEndedEvent(ep_num);
  }
  static void EpReset(Dwc3& drv, Dwc3::Endpoint& ep) { drv.EpReset(ep); }
  static zx_status_t ResetHw(Dwc3& drv, bool is_resume) {
    // Stubbed out: ResetHw in current production takes 0 arguments.
    (void)is_resume;
    return drv.ResetHw();
  }
  static void SetEnableSuspend(Dwc3& drv, bool enable) {
    if (drv.config_.has_value()) {
      drv.config_->enable_suspend() = enable;
    }
  }
  static bool GetPowerOn(Dwc3& drv) { return drv.power_on_; }
  static bool IsActive(Dwc3& drv) { return drv.is_active(); }
  static zx_status_t EpSetStall(Dwc3& drv, Dwc3::Endpoint& ep, bool stall) {
    return drv.EpSetStall(ep, stall);
  }
  static void UserEpQueueNext(Dwc3& drv, Dwc3::UserEndpoint& uep) { drv.UserEpQueueNext(uep); }
  static void UserEpReset(Dwc3& drv, Dwc3::UserEndpoint& uep) { drv.UserEpReset(uep); }
  static void SetDeviceAddress(Dwc3& drv, uint32_t address) { drv.SetDeviceAddress(address); }
  static bool IsFifoEmpty(Dwc3& drv) { return drv.ep0_.shared_fifo.IsEmpty(); }
  static void SetEpTransferState(Dwc3& drv, uint8_t ep_num, TransferState state) {
    if (ep_num < 2) {
      ((ep_num == 0) ? drv.ep0_.out : drv.ep0_.in).transfer_state = state;
    } else {
      auto* uep = drv.get_user_endpoint(ep_num);
      if (uep) {
        uep->ep.transfer_state = state;
      }
    }
  }
  static void SetEpRsrcId(Dwc3& drv, uint8_t ep_num, uint32_t rsrc_id) {
    if (ep_num < 2) {
      ((ep_num == 0) ? drv.ep0_.out : drv.ep0_.in).rsrc_id = rsrc_id;
    } else {
      auto* uep = drv.get_user_endpoint(ep_num);
      if (uep) {
        uep->ep.rsrc_id = rsrc_id;
      }
    }
  }

  static bool GetGotNotReady(Dwc3& drv, uint8_t ep_num) {
    if (ep_num < 2) {
      return ((ep_num == 0) ? drv.ep0_.out : drv.ep0_.in).got_not_ready;
    }
    auto* uep = drv.get_user_endpoint(ep_num);
    return uep ? uep->ep.got_not_ready : false;
  }

  static void SetGotNotReady(Dwc3& drv, uint8_t ep_num, bool got_not_ready) {
    if (ep_num < 2) {
      ((ep_num == 0) ? drv.ep0_.out : drv.ep0_.in).got_not_ready = got_not_ready;
    } else {
      auto* uep = drv.get_user_endpoint(ep_num);
      if (uep) {
        uep->ep.got_not_ready = got_not_ready;
      }
    }
  }

  static uint32_t GetEpRsrcId(Dwc3& drv, uint8_t ep_num) {
    if (ep_num < 2) {
      return ((ep_num == 0) ? drv.ep0_.out : drv.ep0_.in).rsrc_id;
    }
    auto* uep = drv.get_user_endpoint(ep_num);
    return uep ? uep->ep.rsrc_id : UINT32_MAX;
  }

  static bool IsXferIdle(Dwc3& drv, uint8_t ep_num) {
    if (ep_num < 2) {
      return ((ep_num == 0) ? drv.ep0_.out : drv.ep0_.in).transfer_state ==
             dwc3::Dwc3::Endpoint::TransferState::kIdle;
    }
    auto* uep = drv.get_user_endpoint(ep_num);
    return uep ? uep->ep.transfer_state == dwc3::Dwc3::Endpoint::TransferState::kIdle : true;
  }

  static Dwc3::UserEndpoint* GetUserEndpoint(Dwc3& drv, uint8_t ep_num) {
    return drv.get_user_endpoint(ep_num);
  }
  static void AdvanceFifo(TrbFifo& fifo, size_t count) {
    for (size_t i = 0; i < count; ++i) {
      fifo.AdvanceWrite();
      fifo.AdvanceRead();
    }
  }
  static size_t GetQueuedReqsSize(Dwc3& drv, uint8_t ep_num) {
    auto* uep = drv.get_user_endpoint(ep_num);
    if (uep && uep->server.has_value()) {
      return uep->server->queued_reqs.size();
    }
    return 0;
  }

  static void EpSetConfig(Dwc3& drv, Dwc3::Endpoint& ep, bool enable) {
    drv.EpSetConfig(ep, enable);
  }
  static void EpEnable(Dwc3& drv, Dwc3::Endpoint& ep, bool enable) { drv.EpEnable(ep, enable); }
  static async_dispatcher_t* GetDispatcher(Dwc3& drv) { return drv.dispatcher(); }
  static void SetControllerStarted(Dwc3& drv, bool started) { drv.controller_started_ = started; }
  static void HandleIrq(Dwc3& drv, async_dispatcher_t* dispatcher, async::IrqBase* irq,
                        zx_status_t status, const zx_packet_interrupt_t* interrupt) {
    drv.HandleIrq(dispatcher, irq, status, interrupt);
  }
  static void SetCurSetup(Dwc3& drv, const fuchsia_hardware_usb_descriptor::wire::UsbSetup& setup) {
    drv.ep0_.cur_setup = setup;
  }
  static void* GetEp0BufferVirt(Dwc3& drv) { return drv.ep0_.buffer->virt(); }
  static size_t GetEp0BufferSize(Dwc3& drv) { return drv.ep0_.buffer->size(); }
  static bool GetEp0BufferEnableCache(Dwc3& drv) { return drv.ep0_.buffer->enable_cache(); }
  static dma_buffer::CacheOptions GetEp0BufferCacheOptions(Dwc3& drv) {
    return drv.ep0_.buffer->cache_options();
  }
  static zx::unowned_vmo GetEp0BufferVmo(Dwc3& drv) { return drv.ep0_.buffer->vmo(); }
  static void WriteEp0Buffer(Dwc3& drv, const void* src, size_t offset, size_t size) {
    ZX_ASSERT_MSG(drv.ep0_.buffer != nullptr, "WriteEp0Buffer called with null ep0_.buffer!");
    zx::result<> status = drv.ep0_.buffer->Write(src, offset, size);
    ZX_ASSERT_MSG(status.is_ok(), "WriteEp0Buffer failed: %s", status.status_string());
  }
  // Note: dma_buffer::Buffer::Read takes (offset, length, dest), unlike Write which takes (src,
  // offset, length).
  static void ReadEp0Buffer(Dwc3& drv, void* dest, size_t offset, size_t size) {
    ZX_ASSERT_MSG(drv.ep0_.buffer != nullptr, "ReadEp0Buffer called with null ep0_.buffer!");
    zx::result<> status = drv.ep0_.buffer->Read(offset, size, dest);
    ZX_ASSERT_MSG(status.is_ok(), "ReadEp0Buffer failed: %s", status.status_string());
  }
  static bool IsEp0OutStalled(Dwc3& drv) { return drv.ep0_.out.stalled; }
  static bool IsEp0InStalled(Dwc3& drv) { return drv.ep0_.in.stalled; }
  static void SetEp0OutEnabled(Dwc3& drv, bool enabled) { drv.ep0_.out.enabled = enabled; }
  static void SetEp0InEnabled(Dwc3& drv, bool enabled) { drv.ep0_.in.enabled = enabled; }
  static void PushTrbToSharedFifo(Dwc3& drv, const dwc3_trb_t& trb) {
    dwc3_trb_t* ptr = drv.ep0_.shared_fifo.AdvanceWrite();
    *ptr = trb;
    drv.ep0_.shared_fifo.Write(ptr);
  }
  static void HandleResetEvent(Dwc3& drv) { drv.HandleResetEvent(); }

  static void ClearSharedFifo(Dwc3& drv) { drv.ep0_.shared_fifo.Clear(); }
  static bool IsSharedFifoEmpty(Dwc3& drv) { return drv.ep0_.shared_fifo.IsEmpty(); }

  static void BindDciInterface(Dwc3& drv,
                               fidl::ClientEnd<fuchsia_hardware_usb_dci::UsbDciInterface> client) {
    drv.dci_intf_.Bind(std::move(client), drv.dispatcher());
  }

  // Simulation constructs for EP0
  static void SimulateSetupReceived(Dwc3& drv,
                                    const fuchsia_hardware_usb_descriptor::wire::UsbSetup& setup) {
    WriteEp0Buffer(drv, &setup, 0, sizeof(setup));

    ZX_ASSERT_MSG(!drv.ep0_.shared_fifo.IsEmpty(), "SimulateSetupReceived called on empty FIFO!");
    dwc3_trb_t* trb = drv.ep0_.shared_fifo.current_read();
    trb->control &= ~TRB_HWO;

    drv.HandleEp0TransferCompleteEvent(0);  // EP0 OUT
  }

  static void SimulateDataOutPhase(Dwc3& drv, uint32_t received_len) {
    ZX_ASSERT_MSG(!drv.ep0_.shared_fifo.IsEmpty(), "SimulateDataOutPhase called on empty FIFO!");
    dwc3_trb_t* trb = drv.ep0_.shared_fifo.current_read();
    trb->status = TRB_BUFSIZ(static_cast<uint32_t>(drv.ep0_.buffer->size() - received_len));

    trb->control &= ~TRB_HWO;
    drv.HandleEp0TransferCompleteEvent(0);  // EP0 OUT
  }

  static void SimulateDataInPhase(Dwc3& drv) {
    dwc3_trb_t* trb = drv.ep0_.shared_fifo.current_read();
    trb->status = 0;  // Simulate all bytes sent (0 remaining)
    trb->control &= ~TRB_HWO;
    drv.HandleEp0TransferCompleteEvent(1);  // EP0 IN
  }

  static void SimulateStatusPhase(Dwc3& drv, bool is_in) {
    uint8_t ep_num = is_in ? 1 : 0;
    drv.HandleEp0TransferNotReadyEvent(ep_num, DEPEVT_XFER_NOT_READY_STAGE_STATUS);

    // Simulate completion of the status phase TRB
    dwc3_trb_t* trb = drv.ep0_.shared_fifo.current_read();
    trb->status = TRB_BUFSIZ(
        static_cast<uint32_t>(drv.ep0_.buffer->size()));  // 0 bytes transferred in status phase
    trb->control &= ~TRB_HWO;

    drv.HandleEp0TransferCompleteEvent(ep_num);
  }
};

class FakeUsbPhy : public fidl::Server<fphy::UsbPhy>, public fidl::Server<fphy::ConnectionWatcher> {
 public:
  ~FakeUsbPhy() override {
    if (expect_connection_status_observer_call_) {
      EXPECT_TRUE(connection_status_observer_called_);
      EXPECT_TRUE(completer_.has_value());
    }
  }

  fuchsia_hardware_usb_phy::Service::InstanceHandler GetUsbPhyInstanceHandler(
      async_dispatcher_t* dispatcher) {
    return fuchsia_hardware_usb_phy::Service::InstanceHandler({
        .device = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });
  }

  fuchsia_hardware_usb_phy::ConnectionWatcherService::InstanceHandler
  GetConnectionWatcherInstanceHandler(async_dispatcher_t* dispatcher) {
    return fuchsia_hardware_usb_phy::ConnectionWatcherService::InstanceHandler({
        .watcher = watcher_bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });
  }

  void set_connection_status_observer_called(bool set) { connection_status_observer_called_ = set; }

  void set_initial_connected(bool set) { initial_connected_ = set; }
  void set_expect_connection_status_observer_call(bool expect) {
    expect_connection_status_observer_call_ = expect;
  }
  bool expect_connection_status_observer_call() const {
    return expect_connection_status_observer_call_;
  }
  bool has_completer() const { return completer_.has_value(); }

  void TriggerConnection(bool connected) {
    ZX_ASSERT(completer_.has_value());
    fuchsia_hardware_usb_phy::ConnectionWatcherWatchConnectStatusChangedResponse response{{
        .connected = connected,
        .wake_lease = {},
    }};
    completer_->Reply(zx::ok(std::move(response)));
    completer_.reset();
  }

  void TriggerDisconnect() { TriggerConnection(false); }

  libsync::Completion* completion() { return &completion_; }

 private:
  void ConnectStatusChanged(ConnectStatusChangedRequest& request,
                            ConnectStatusChangedCompleter::Sync& completer) override {
    completer.Reply(zx::ok());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fphy::UsbPhy> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method {}", metadata.method_ordinal);
  }

  void WatchConnectStatusChanged(WatchConnectStatusChangedRequest& request,
                                 WatchConnectStatusChangedCompleter::Sync& completer) override {
    if (!connection_status_observer_called_) {
      fuchsia_hardware_usb_phy::ConnectionWatcherWatchConnectStatusChangedResponse response{{
          .connected = initial_connected_,
          .wake_lease = {},
      }};
      completer.Reply(zx::ok(std::move(response)));

      connection_status_observer_called_ = true;
      return;
    }

    ASSERT_FALSE(completer_.has_value());
    completer_.emplace(completer.ToAsync());
    completion_.Signal();
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fphy::ConnectionWatcher> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method {}", metadata.method_ordinal);
  }

  fidl::ServerBindingGroup<fphy::UsbPhy> bindings_;
  fidl::ServerBindingGroup<fphy::ConnectionWatcher> watcher_bindings_;

  bool connection_status_observer_called_ = false;
  bool initial_connected_ = false;
  bool expect_connection_status_observer_call_ = true;
  std::optional<WatchConnectStatusChangedCompleter::Async> completer_;
  libsync::Completion completion_;  // Signaled when the above completer_ is saved.
};

class FakePath final : public fidl::Server<fhi::Path> {
 public:
  explicit FakePath() = default;
  virtual ~FakePath() = default;

  fhi::PathService::InstanceHandler GetInstanceHandler(async_dispatcher_t* dispatcher) {
    return fhi::PathService::InstanceHandler({
        .path = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });
  }

  void SetBandwidth(SetBandwidthRequest& request, SetBandwidthCompleter::Sync& completer) override {
    last_average_bandwidth_bps_ = request.average_bandwidth_bps().value_or(0);
    last_peak_bandwidth_bps_ = request.peak_bandwidth_bps().value_or(0);
    set_bandwidth_count_++;
    completer.Reply(zx::ok());
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fhi::Path> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  uint64_t last_average_bandwidth_bps() const { return last_average_bandwidth_bps_; }
  uint64_t last_peak_bandwidth_bps() const { return last_peak_bandwidth_bps_; }
  uint32_t set_bandwidth_count() const { return set_bandwidth_count_; }

 private:
  fidl::ServerBindingGroup<fhi::Path> bindings_;
  uint64_t last_average_bandwidth_bps_ = 0;
  uint64_t last_peak_bandwidth_bps_ = 0;
  uint32_t set_bandwidth_count_ = 0;
};

class Environment : public fdf_testing::Environment {
 public:
  Environment() {
    auto config = fdf_fake::FakePDev::Config{};
    config.mmios[0] = reg_region_.GetMmioBuffer();
    config.use_fake_bti = true;
    config.use_fake_irq = true;

    pdev_.SetConfig(std::move(config));
  }

  void SetDriverMetadata(std::optional<uint32_t> interrupt_moderation_us,
                         std::optional<uint32_t> fladj = std::nullopt,
                         bool refclk_lpm_sel = false) {
    fuchsia_driver_metadata::Dictionary dictionary;
    std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
    if (interrupt_moderation_us.has_value()) {
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry{{
          .key = "interrupt-moderation-us",
          .value = fuchsia_driver_metadata::DictionaryValue::WithInt64(*interrupt_moderation_us),
      }});
    }
    if (fladj.has_value()) {
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry{{
          .key = "quirk-frame-length-adjustment",
          .value = fuchsia_driver_metadata::DictionaryValue::WithInt64(*fladj),
      }});
    }
    if (refclk_lpm_sel) {
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry{{
          .key = "gfladj-refclk-lpm-sel-quirk",
          .value = fuchsia_driver_metadata::DictionaryValue::WithBoolean(true),
      }});
    }
    dictionary.entries() = std::move(entries);
    pdev_.AddFidlMetadata("fuchsia.driver.metadata.Dictionary", dictionary);
  }

  void SetInterruptModerationUs(uint32_t us) { SetDriverMetadata(us); }

  void SetFrameLengthAdjustment(std::optional<uint32_t> fladj = std::nullopt,
                                bool refclk_lpm_sel = false) {
    SetDriverMetadata(std::nullopt, fladj, refclk_lpm_sel);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& directory) override {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    zx::result result =
        directory.AddService<fpdev::Service>(pdev_.GetInstanceHandler(dispatcher), "pdev");
    EXPECT_TRUE(result.is_ok());

    result =
        directory.AddService<fhi::PathService>(path_.GetInstanceHandler(dispatcher), "usb-ddr");
    EXPECT_TRUE(result.is_ok());

    result =
        directory.AddService<fhi::PathService>(path_.GetInstanceHandler(dispatcher), "usb-ipa");
    EXPECT_TRUE(result.is_ok());

    result =
        directory.AddService<fhi::PathService>(path_.GetInstanceHandler(dispatcher), "ddr-usb");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fhi::PathService>(path_.GetInstanceHandler(dispatcher),
                                                    "usb-interconnect");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fphy::Service>(usb_phy_.GetUsbPhyInstanceHandler(dispatcher),
                                                 "dwc3-phy");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fphy::ConnectionWatcherService>(
        usb_phy_.GetConnectionWatcherInstanceHandler(dispatcher), "dwc3-phy");
    EXPECT_TRUE(result.is_ok());

    result =
        directory.AddService<fclock::Service>(clock_xo_.CreateInstanceHandler(dispatcher), "xo");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fclock::Service>(clock_sleep_.CreateInstanceHandler(dispatcher),
                                                   "sleep-clk");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fclock::Service>(clock_iface_.CreateInstanceHandler(dispatcher),
                                                   "iface-clk");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fclock::Service>(clock_core_.CreateInstanceHandler(dispatcher),
                                                   "core-clk");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fclock::Service>(clock_utmi_.CreateInstanceHandler(dispatcher),
                                                   "utmi-clk");
    EXPECT_TRUE(result.is_ok());

    result = directory.AddService<fclock::Service>(
        clock_bus_aggr_.CreateInstanceHandler(dispatcher), "bus-aggr-clk");
    EXPECT_TRUE(result.is_ok());

    if (serve_platform_mocks_) {
      result = directory.AddService<freset::Service>(reset_.CreateInstanceHandler(), "core_reset");
      EXPECT_TRUE(result.is_ok());

      // Mimic Qualcomm GDSC regulator behavior where voltage stepping is unsupported.
      vreg_.set_get_voltage_step_result(zx::error(ZX_ERR_NOT_SUPPORTED));
      result =
          directory.AddService<fvreg::Service>(vreg_.CreateInstanceHandler(), "dwc3-regulator");
      EXPECT_TRUE(result.is_ok());
    }

    return zx::ok();
  }

  // Note: Only intended for teardown, does not restore default mock behaviors.
  void Reset() {
    for (size_t i = 0; i < kRegCount; i++) {
      reg_region_[i * kRegSize].SetReadCallback([]() { return 0; });
      reg_region_[i * kRegSize].SetWriteCallback([](uint64_t value) {});
    }
    serve_platform_mocks_ = true;
  }

  ddk_fake::FakeMmioRegRegion& reg_region() { return reg_region_; }

  FakePath& path() { return path_; }
  FakeUsbPhy& usb_phy() { return usb_phy_; }
  const fdf_fake::FakeClock& clock_xo() const { return clock_xo_; }
  const fdf_fake::FakeClock& clock_sleep() const { return clock_sleep_; }
  const fdf_fake::FakeClock& clock_iface() const { return clock_iface_; }
  const fdf_fake::FakeClock& clock_core() const { return clock_core_; }
  const fdf_fake::FakeClock& clock_utmi() const { return clock_utmi_; }
  const fdf_fake::FakeClock& clock_bus_aggr() const { return clock_bus_aggr_; }
  fdf_fake::FakeReset& reset() { return reset_; }
  const fdf_fake::FakeVreg& vreg() const { return vreg_; }

  void set_serve_platform_mocks(bool serve) { serve_platform_mocks_ = serve; }
  static constexpr size_t kRegSize = sizeof(uint32_t);
  static constexpr size_t kMmioRegionSize = 0x10'0000;
  static constexpr size_t kRegCount = kMmioRegionSize / kRegSize;

 private:
  fdf_fake::FakePDev pdev_;
  ddk_fake::FakeMmioRegRegion reg_region_{kRegSize, kRegCount};
  FakePath path_;
  FakeUsbPhy usb_phy_;
  fdf_fake::FakeClock clock_xo_;
  fdf_fake::FakeClock clock_sleep_;
  fdf_fake::FakeClock clock_iface_;
  fdf_fake::FakeClock clock_core_;
  fdf_fake::FakeClock clock_utmi_;
  fdf_fake::FakeClock clock_bus_aggr_;
  fdf_fake::FakeReset reset_;
  fdf_fake::FakeVreg vreg_;
  bool serve_platform_mocks_ = true;
};

class Config final {
 public:
  using DriverType = Dwc3;
  using EnvironmentType = Environment;
};

// Test is templated on a parameter which, if true, will have the harness start and stop the driver.
// Otherwise, it is the individual test(s) responsibility to start and stop the driver.
template <bool manage_lifetime, typename gtest_base = testing::Test>
class TestFixture : public gtest_base {
 public:
  using Endpoint = Dwc3::Endpoint;
  using TransferState = Dwc3::Endpoint::TransferState;

  static Dwc3::UserEndpoint& GetUserEndpoint(Dwc3& drv, uint8_t ep_num) {
    auto* uep = drv.get_user_endpoint(ep_num);
    ZX_ASSERT(uep != nullptr);
    return *uep;
  }

  static uint8_t UsbAddressToEpNum(uint8_t addr) { return Dwc3::UsbAddressToEpNum(addr); }

  static const zx::bti& GetBti(const Dwc3& drv) { return drv.bti_; }

  static void TriggerEpTransferNotReady(Dwc3& drv, uint8_t ep_num, uint32_t stage) {
    drv.HandleEpTransferNotReadyEvent(ep_num, stage);
  }

  static void TriggerEpTransferComplete(Dwc3& drv, uint8_t ep_num, uint32_t residual = 0) {
    auto* uep = drv.get_user_endpoint(ep_num);
    ZX_ASSERT(uep != nullptr);

    if (uep->fifo.GetActiveCount() > 0) {
      dwc3_trb_t* trb = uep->fifo.read_;
      trb->control &= ~TRB_HWO;
      trb->status = TRB_BUFSIZ(residual);
      uep->fifo.Write(trb, 1);
    }

    drv.HandleEpTransferCompleteEvent(ep_num);

    // In production, SendCompletions is called at the end of the global event
    // interrupt handler loop (once per interrupt batch), rather than from
    // inside the individual endpoint event handlers. We simulate that final
    // step here so that completions are immediately dispatched to test
    // clients.
    uep->server->SendCompletions();
  }

  static void TriggerEpTransferInProgress(Dwc3& drv, uint8_t ep_num) {
    auto* uep = drv.get_user_endpoint(ep_num);
    ZX_ASSERT(uep != nullptr);

    if (uep->fifo.GetActiveCount() > 0) {
      dwc3_trb_t* trb = uep->fifo.read_;
      trb->control &= ~TRB_HWO;
      trb->status = 0;
      uep->fifo.Write(trb, 1);
    }

    drv.HandleEpTransferInProgressEvent(ep_num);

    // In production, SendCompletions is called at the end of the global event
    // interrupt handler loop (once per interrupt batch), rather than from
    // inside the individual endpoint event handlers. We simulate that final
    // step here so that completions are immediately dispatched to test
    // clients.
    uep->server->SendCompletions();
  }

  static void TriggerEpTransferStarted(Dwc3& drv, uint8_t ep_num, uint32_t rsrc_id) {
    drv.HandleEpTransferStartedEvent(ep_num, rsrc_id);
  }

  static void TriggerEpTransferEnded(Dwc3& drv, uint8_t ep_num) {
    drv.HandleEpTransferEndedEvent(ep_num);
  }

  static void TriggerConnectionDone(Dwc3& drv) { drv.HandleConnectionDoneEvent(); }

 protected:
  PlatformExtension* GetPlatformExtension(Dwc3& drv) { return drv.platform_extension_.get(); }

 public:
  void TriggerConnectionPlugIn(fuchsia_hardware_usb_descriptor::UsbSpeed speed) {
    namespace fdescriptor = fuchsia_hardware_usb_descriptor;
    // Wait for the mock PHY to establish connection observer registration.
    // Relies on the test runtime's overarching test timeout to prevent flakiness under CI load.
    dut_.runtime().RunUntil([&]() {
      bool has_comp = false;
      dut_.RunInEnvironmentTypeContext(
          [&](Environment& env) { has_comp = env.usb_phy().has_completer(); });
      return has_comp;
    });
    dut_.RunInEnvironmentTypeContext([&](Environment& env) {
      auto& dsts_reg = env.reg_region()[DSTS::Get().addr()];
      dsts_reg.SetReadCallback([speed]() -> uint32_t {
        uint32_t speed_val = 0;
        if (speed == fdescriptor::UsbSpeed::kSuper) {
          speed_val = DSTS::CONNECTSPD_SUPER;
        }
        return DSTS::Get().FromValue(0).set_CONNECTSPD(speed_val).reg_value();
      });
      env.usb_phy().TriggerConnection(true);
    });

    // Deterministic synchronization: Wait for the driver dispatcher to process the event.
    dut_.runtime().RunUntil(
        [&]() { return dut_.RunInDriverContext<bool>([](Dwc3& drv) { return drv.power_on(); }); });
  }

  // Returns an RAII guard that restores DEPCMD register callbacks for the endpoint to their default
  // no-op state on scope exit. Note: FakeMmioReg does not maintain internal storage and invokes
  // its callbacks directly, so restoring the constructor defaults ([](){}) is required to avoid
  // null fit::function invocations on subsequent register accesses.
  [[nodiscard]] auto DeferClearDepcmdCallbacks(uint8_t ep_num) {
    return fit::defer([this, ep_num]() {
      dut_.RunInEnvironmentTypeContext([ep_num](Environment& env) {
        env.reg_region()[DEPCMD::Get(ep_num).addr()].SetWriteCallback([](uint64_t) {});
        env.reg_region()[DEPCMD::Get(ep_num).addr()].SetReadCallback(
            []() -> uint32_t { return 0; });
      });
    });
  }

  // Returns an RAII guard that clears the DCTL callback on scope exit.
  [[nodiscard]] auto DeferClearDctlCallback() {
    return fit::defer([this]() { SetDctlCallback(nullptr); });
  }

  std::vector<fuchsia_hardware_usb_request::Request> CreateVmoBuffer(
      const fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint>& sync_client, size_t size,
      size_t buffer_size, uint8_t vmo_id = 1) {
    fuchsia_hardware_usb_endpoint::VmoInfo vmo_info;
    vmo_info.id(vmo_id);
    vmo_info.size(size);

    std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_infos;
    vmo_infos.reserve(1);
    vmo_infos.push_back(std::move(vmo_info));

    auto reg_result = sync_client->RegisterVmos({std::move(vmo_infos)});
    ZX_ASSERT_MSG(reg_result.is_ok(), "RegisterVmos failed: %s",
                  reg_result.error_value().status_string());
    ZX_ASSERT(reg_result->vmos().size() == 1UL);

    auto make_request = [](uint8_t id, size_t b_size) {
      fuchsia_hardware_usb_request::BufferRegion region;
      region.buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(id));
      region.size(b_size);
      region.offset(0);

      std::vector<fuchsia_hardware_usb_request::BufferRegion> regions;
      regions.reserve(1);
      regions.push_back(std::move(region));

      fuchsia_hardware_usb_request::Request req;
      req.data(std::move(regions)).defer_completion(false);
      return req;
    };

    std::vector<fuchsia_hardware_usb_request::Request> requests;
    requests.reserve(1);
    requests.push_back(make_request(vmo_id, buffer_size));
    return requests;
  }

  void QueueRequestsAndWaitForStartTransfer(
      uint8_t ep_addr, const fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint>& sync_client,
      std::vector<fuchsia_hardware_usb_request::Request> requests) {
    auto completion = std::make_shared<libsync::Completion>();
    auto cleanup_callbacks = DeferClearDepcmdCallbacks(ep_addr);

    dut_.RunInEnvironmentTypeContext([ep_addr, completion](Environment& env) {
      auto& depcmd = env.reg_region()[DEPCMD::Get(ep_addr).addr()];
      depcmd.SetWriteCallback([ep_addr, completion](uint64_t val_raw) {
        uint32_t val = static_cast<uint32_t>(val_raw);
        if (DEPCMD::Get(ep_addr).FromValue(val).CMDTYP() == DEPCMD::DEPSTRTXFER) {
          completion->Signal();
        }
      });
      depcmd.SetReadCallback([]() -> uint32_t { return 0; });
    });

    auto result = sync_client->QueueRequests({std::move(requests)});
    ZX_ASSERT_MSG(result.is_ok(), "QueueRequests failed: %s",
                  result.error_value().FormatDescription().c_str());

    dut_.runtime().RunUntil([&]() { return completion->signaled(); });
    ZX_ASSERT_MSG(completion->signaled(), "Wait for StartTransfer timed out");
  }

  void SetUpAndPowerOnEndpoints() {
    dut_.RunInEnvironmentTypeContext(
        [&](Environment& env) { env.usb_phy().set_initial_connected(true); });
    dut_.RunInDriverContext([&](Dwc3& drv) { Dwc3TestHelper::SetPowerOn(drv, true); });
  }

  void SetUp() override {
    stuck_reset_test_.store(false);
    stuck_halt_test_.store(false);
    vbus_high_.store(false);

    dut_.RunInEnvironmentTypeContext([&](Environment& env) {
      auto& hwparams3 = env.reg_region()[GHWPARAMS3::Get().addr()];
      auto& dctl_reg = env.reg_region()[DCTL::Get().addr()];
      auto& gsnpsid_reg = env.reg_region()[GSNPSID::Get().addr()];

      hwparams3.SetReadCallback([this]() -> uint32_t { return Read_GHWPARAMS3(); });
      dctl_reg.SetReadCallback([this]() -> uint32_t { return Read_DCTL(); });
      dctl_reg.SetWriteCallback(
          [this](uint64_t val) { return Write_DCTL(static_cast<uint32_t>(val)); });
      gsnpsid_reg.SetReadCallback([this]() -> uint32_t { return Read_GSNPSID(); });

      env.reg_region()[DSTS::Get().addr()].SetReadCallback([this]() { return Read_DSTS(); });

      auto& ver_num_reg = env.reg_region()[USB31_VER_NUMBER::Get().addr()];
      auto& ver_type_reg = env.reg_region()[USB31_VER_TYPE::Get().addr()];
      ver_num_reg.SetReadCallback([this]() -> uint32_t { return Read_USB31_VER_NUMBER(); });
      ver_type_reg.SetReadCallback([this]() -> uint32_t { return Read_USB31_VER_TYPE(); });
    });

    if (manage_lifetime) {
      ASSERT_TRUE(dut_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                        dwc3_config::Config cfg;
                        cfg.enable_suspend() = false;
                        cfg.bypass_platform_extension() = true;
                        args.config(cfg.ToVmo());
                      })
                      .is_ok());
      ASSERT_EQ(WaitForPhy(), ZX_OK);
    }
  }

  void TearDown() override {
    stuck_reset_test_.store(false);
    vbus_high_.store(false);

    dut_.runtime().RunUntilIdle();
    if (manage_lifetime) {
      EXPECT_EQ(WaitForPhy(), ZX_OK);
      EXPECT_EQ(dut_.StopDriver().status_value(), ZX_OK);
    }

    dut_.RunInEnvironmentTypeContext([](Environment& env) { env.Reset(); });
    dut_.runtime().RunUntilIdle();
  }

  void SetDctlCallback(fit::function<void()> cb) {
    std::lock_guard<std::mutex> lock(dctl_mutex_);
    dctl_callback_ = std::move(cb);
  }

 protected:
  // Section 1.2.22 of the DWC3 Programmer's guide
  //
  // DWC_USB31_CACHE_TOTAL_XFER_RESOURCES : 32
  // DWC_USB31_NUM_IN_EPS                 : 16
  // DWC_USB31_NUM_EPS                    : 32
  // DWC_USB31_VENDOR_CTL_INTERFACE       : 0
  // DWC_USB31_HSPHY_DWIDTH               : 2
  // DWC_USB31_HSPHY_INTERFACE            : 1
  // DWC_USB31_SSPHY_INTERFACE            : 2
  uint32_t Read_GHWPARAMS3() { return 0x10420086; }

  uint32_t ver_number_{0x5533160a};  // 1.60a by default

  // Section 1.4.2 of the DWC3 Programmer's guide
  uint32_t Read_DCTL() { return dctl_val_.load(); }
  void Write_DCTL(uint32_t val) {
    if (DCTL::Get().FromValue(val).CSFTRST() == 1 && vbus_high_.load()) {
      ADD_FAILURE()
          << "BUG TRIPPED: CSFTRST asserted while physically connected to host! PMIC over-current crowbar spike imminent!";
    }

    constexpr uint32_t kUnwriteableMask =
        (1 << 29) | (1 << 17) | (1 << 16) | (1 << 15) | (1 << 14) | (1 << 13) | (1 << 0);
    uint32_t updated_val = static_cast<uint32_t>(val & ~kUnwriteableMask);

    if (!stuck_reset_test_.load()) {
      updated_val = DCTL::Get().FromValue(updated_val).set_CSFTRST(0).reg_value();
    }
    dctl_val_.store(updated_val);

    {
      std::lock_guard<std::mutex> lock(dctl_mutex_);
      if (dctl_callback_) {
        dctl_callback_();
      }
    }

    // Satisfy the categorical Spec conformance loop: When controller is halted, set DEVCTRLHLT.
    if (!stuck_halt_test_.load()) {
      uint32_t expected = dsts_val_.load();
      while (true) {
        uint32_t desired =
            DSTS::Get()
                .FromValue(expected)
                .set_DEVCTRLHLT(DCTL::Get().FromValue(dctl_val_.load()).RUN_STOP() == 0 ? 1 : 0)
                .reg_value();
        if (dsts_val_.compare_exchange_weak(expected, desired)) {
          break;
        }
      }
    }
  }

  uint32_t Read_DSTS() { return dsts_val_.load(); }

  // Section 1.2.9 of the DWC3 Programmer's guide
  //
  // core_id = 0x5533
  // version = 1.60a
  uint32_t Read_GSNPSID() { return ver_number_; }
  uint32_t Read_USB31_VER_NUMBER() { return ver_number31_.load(); }
  uint32_t Read_USB31_VER_TYPE() { return ver_type31_.load(); }

  std::atomic<uint32_t> dctl_val_{DCTL::Get().FromValue(0).set_LPM_NYET_thres(0xF).reg_value()};
  // Initialize as halted by default before driver enables RUN_STOP.
  std::atomic<uint32_t> dsts_val_{DSTS::Get().FromValue(0).set_DEVCTRLHLT(1).reg_value()};
  std::atomic<uint32_t> ver_number31_{0};
  std::atomic<uint32_t> ver_type31_{0};
  std::atomic<bool> stuck_reset_test_{false};
  std::atomic<bool> stuck_halt_test_{false};
  std::atomic<bool> vbus_high_{false};
  std::mutex dctl_mutex_;
  fit::function<void()> dctl_callback_;

  fdf_testing::BackgroundDriverTest<Config> dut_;

  // There's an inherent race in the way this test is set up between the three threads: the
  // foreground testing thread, the background driver thread, and the environment thread the fakes
  // are running on. Driving the driver's dispatcher to an idle state and then tearing down the test
  // will race with the environment's dispatcher execution of the Watch handler. If the environment
  // dispatcher is torn down before the side effects of the Watch handler execute, ~FakeUsbPhy()
  // will sometimes fail. To resolve this race, the foreground testing thread needs to be
  // synchronized against the environment thread and wait for the fakes to catch up.
  zx_status_t WaitForPhy() {
    bool expect = true;
    dut_.RunInEnvironmentTypeContext(
        [&](Environment& env) { expect = env.usb_phy().expect_connection_status_observer_call(); });
    if (!expect) {
      return ZX_OK;
    }
    return dut_.runtime().RunWithTimeoutOrUntil(
               [&]() {
                 bool has_comp = false;
                 dut_.RunInEnvironmentTypeContext(
                     [&](Environment& env) { has_comp = env.usb_phy().has_completer(); });
                 return has_comp;
               },
               zx::sec(5))
               ? ZX_OK
               : ZX_ERR_TIMED_OUT;
  }
};

using ManagedTestFixture = TestFixture<true>;

class TestEndpointEventHandler
    : public fidl::SyncEventHandler<fuchsia_hardware_usb_endpoint::Endpoint> {
 public:
  TestEndpointEventHandler(bool& completed, zx_status_t& status, size_t* length = nullptr)
      : completed_(completed), status_(status), length_(length) {}
  void OnCompletion(
      fidl::Event<fuchsia_hardware_usb_endpoint::Endpoint::OnCompletion>& event) override {
    if (!event.completion().empty()) {
      if (event.completion()[0].status().has_value()) {
        status_ = *event.completion()[0].status();
      }
      if (length_ && event.completion()[0].transfer_size().has_value()) {
        *length_ = *event.completion()[0].transfer_size();
      }
    }
    completed_ = true;
  }

 private:
  bool& completed_;
  zx_status_t& status_;
  size_t* length_;
};

class FakeUsbDciInterface : public fidl::WireServer<fuchsia_hardware_usb_dci::UsbDciInterface> {
 public:
  using ControlCallback = std::function<void(fuchsia_hardware_usb_descriptor::wire::UsbSetup,
                                             cpp20::span<const uint8_t>)>;
  using SetConnectedCallback = std::function<void(bool connected)>;
  using SetSpeedCallback =
      std::function<void(fuchsia_hardware_usb_descriptor::wire::UsbSpeed speed)>;

  void SetControlCallback(ControlCallback cb) { control_cb_ = std::move(cb); }
  void SetControlStatus(zx_status_t status) { control_status_ = status; }
  void SetReadData(std::vector<uint8_t> data) { read_data_ = std::move(data); }
  void SetSetConnectedCallback(SetConnectedCallback cb) { set_connected_cb_ = std::move(cb); }
  void SetSetSpeedCallback(SetSpeedCallback cb) { set_speed_cb_ = std::move(cb); }

  void Control(ControlRequestView request, ControlCompleter::Sync& completer) override {
    control_called_.store(true);

    if (control_cb_) {
      control_cb_(request->setup,
                  cpp20::span<const uint8_t>(request->write.data(), request->write.size()));
    }

    if (control_status_ != ZX_OK) {
      completer.Reply(zx::error(control_status_));
      return;
    }

    if ((request->setup.bm_request_type & USB_DIR_MASK) == USB_DIR_IN) {
      uint8_t* data = read_data_.data();
      size_t size = read_data_.size();
      response_.read = fidl::VectorView<uint8_t>::FromExternal(data, size);
    }
    completer.Reply(zx::ok(&response_));
  }

  void SetConnected(SetConnectedRequestView request,
                    SetConnectedCompleter::Sync& completer) override {
    set_connected_called_.store(true);
    if (set_connected_cb_) {
      set_connected_cb_(request->is_connected);
    }
    completer.Reply(zx::ok());
  }

  void SetSpeed(SetSpeedRequestView request, SetSpeedCompleter::Sync& completer) override {
    set_speed_called_.store(true);
    if (set_speed_cb_) {
      set_speed_cb_(request->speed);
    }
    completer.Reply(zx::ok());
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_dci::UsbDciInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  bool control_called() const { return control_called_.load(); }
  bool set_connected_called() const { return set_connected_called_.load(); }
  bool set_speed_called() const { return set_speed_called_.load(); }

 private:
  std::atomic<bool> control_called_{false};
  std::atomic<bool> set_connected_called_{false};
  std::atomic<bool> set_speed_called_{false};
  ControlCallback control_cb_;
  SetConnectedCallback set_connected_cb_;
  SetSpeedCallback set_speed_cb_;
  fuchsia_hardware_usb_dci::wire::UsbDciInterfaceControlResponse response_;
  zx_status_t control_status_ = ZX_OK;
  std::vector<uint8_t> read_data_;
};

class UnmanagedTestFixture : public TestFixture<false> {
 public:
  using TestFixture<false>::TestFixture;

  fidl::SyncClient<fuchsia_hardware_usb_dci::UsbDci> ConnectController() {
    auto client_end = dut_.Connect<fuchsia_hardware_usb_dci::UsbDciService::Device>();
    ZX_ASSERT(client_end.is_ok());
    return fidl::SyncClient<fuchsia_hardware_usb_dci::UsbDci>(std::move(client_end.value()));
  }

  void SetUpAndPowerOnDriver() {
    dut_.RunInEnvironmentTypeContext([](Environment& env) {
      env.usb_phy().set_initial_connected(true);
      env.reg_region()[DEPCMD::Get(0).addr()].SetReadCallback([]() -> uint32_t { return 0; });
      env.reg_region()[DEPCMD::Get(1).addr()].SetReadCallback([]() -> uint32_t { return 0; });
    });

    ASSERT_TRUE(dut_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                      dwc3_config::Config cfg;
                      cfg.enable_suspend() = false;
                      cfg.bypass_platform_extension() = true;
                      args.config(cfg.ToVmo());
                    })
                    .is_ok());

    // Wait for driver to be powered on after potential restart to stabilize dispatcher
    EXPECT_TRUE(dut_.runtime().RunWithTimeoutOrUntil(
        [&]() {
          bool powered = false;
          dut_.RunInDriverContext([&](Dwc3& drv) {
            powered = (Dwc3TestHelper::GetDeviceState(drv) ==
                       fuchsia_hardware_usb_policy::wire::DeviceState::kPowered);
          });
          return powered;
        },
        zx::sec(10)));
  }

  zx::result<> StartDriverWithoutPlatformExtension() {
    return dut_.StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
      dwc3_config::Config cfg;
      cfg.enable_suspend() = false;
      cfg.bypass_platform_extension() = true;
      args.config(cfg.ToVmo());
    });
  }

  std::shared_ptr<std::atomic<uint32_t>> InterceptGctlReads() {
    auto count = std::make_shared<std::atomic<uint32_t>>(0);
    dut_.RunInEnvironmentTypeContext([count](Environment& env) {
      auto& gctl_reg = env.reg_region()[GCTL::Get().addr()];
      gctl_reg.SetReadCallback([count]() -> uint64_t {
        (*count)++;
        return GCTL::Get().FromValue(0).set_PWRDNSCALE(2).reg_value();
      });
    });
    return count;
  }

  void TearDownAndPowerOffDriver() {
    dut_.RunInDriverContext([&](Dwc3& drv) {
      Dwc3TestHelper::SetEpRsrcId(drv, 0, 2);
      Dwc3TestHelper::SetEpRsrcId(drv, 1, 2);
    });
    EXPECT_EQ(WaitForPhy(), ZX_OK);
    EXPECT_EQ(dut_.StopDriver().status_value(), ZX_OK);
    dut_.RunInEnvironmentTypeContext([](Environment& env) { env.Reset(); });
  }

  void BindDciInterfaceWithoutServer() {
    dut_.RunInDriverContext([&](Dwc3& drv) {
      auto [client_end, server_end] =
          fidl::Endpoints<fuchsia_hardware_usb_dci::UsbDciInterface>::Create();
      Dwc3TestHelper::BindDciInterface(drv, std::move(client_end));
    });
  }

  template <typename Server, typename UnbindCallback = std::nullptr_t>
  auto BindDciInterface(Server* server, UnbindCallback&& unbind_cb = nullptr) {
    std::optional<fidl::ServerBindingRef<fuchsia_hardware_usb_dci::UsbDciInterface>> binding;
    dut_.RunInDriverContext([&](Dwc3& drv) {
      auto [client_end, server_end] =
          fidl::Endpoints<fuchsia_hardware_usb_dci::UsbDciInterface>::Create();
      if constexpr (std::is_same_v<std::decay_t<UnbindCallback>, std::nullptr_t>) {
        binding = fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   std::move(server_end), server);
      } else {
        binding = fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   std::move(server_end), server,
                                   std::forward<UnbindCallback>(unbind_cb));
      }
      Dwc3TestHelper::BindDciInterface(drv, std::move(client_end));
    });
    return binding;
  }

  static fuchsia_hardware_usb_descriptor::wire::UsbSetup MakeSetupPacket(uint8_t bm_request_type,
                                                                         uint8_t b_request,
                                                                         uint16_t w_value,
                                                                         uint16_t w_index,
                                                                         uint16_t w_length) {
    fuchsia_hardware_usb_descriptor::wire::UsbSetup setup;
    setup.bm_request_type = bm_request_type;
    setup.b_request = b_request;
    setup.w_value = w_value;
    setup.w_index = w_index;
    setup.w_length = w_length;
    return setup;
  }

  static fuchsia_hardware_usb_descriptor::wire::UsbSetup MakeGetDescriptorSetup(
      uint16_t length = 18) {
    fuchsia_hardware_usb_descriptor::wire::UsbSetup setup;
    setup.bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE;
    setup.b_request = USB_REQ_GET_DESCRIPTOR;
    setup.w_value = static_cast<uint16_t>(USB_DT_DEVICE << 8);
    setup.w_index = 0;
    setup.w_length = length;
    return setup;
  }
};
}  // namespace dwc3

#endif  // SRC_DEVICES_USB_DRIVERS_DWC3_DWC3_TEST_FIXTURE_H_
