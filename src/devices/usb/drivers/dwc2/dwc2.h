// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_DWC2_DWC2_H_
#define SRC_DEVICES_USB_DRIVERS_DWC2_DWC2_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.dci/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.dwc2/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <threads.h>

#include <memory>
#include <queue>
#include <utility>

#include "src/devices/usb/drivers/dwc2/dwc2_config.h"
#include "src/devices/usb/drivers/dwc2/usb_dwc_regs.h"
#include "src/devices/usb/lib/usb-endpoint/include/usb-endpoint/sdk/usb-endpoint-server.h"

namespace dwc2 {

// DWC2 FIFO sizes are specified in 32-bit words (4 bytes per word).
constexpr uint16_t kWordSizeBytes = 4;
// OUT endpoints share the Rx FIFO. A default FIFO depth of 256 words at 4 bytes per word
// yields a 1024-byte limit.
constexpr uint16_t kDefaultRxFifoDepthWords = 256;
constexpr uint16_t kMaxOutPacketSizeLimit = kDefaultRxFifoDepthWords * kWordSizeBytes;

class Dwc2Test;

class Dwc2 : public fdf::DriverBase2, public fidl::Server<fuchsia_hardware_usb_dci::UsbDci> {
 public:
  Dwc2() : fdf::DriverBase2("dwc2") {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // Neither copyable nor movable.
  Dwc2(Dwc2&&) = delete;
  Dwc2(const Dwc2&) = delete;
  Dwc2& operator=(Dwc2&&) = delete;
  Dwc2& operator=(const Dwc2&) = delete;

  zx_status_t Init(fdf::DriverContext& context, const dwc2_config::Config& config)
      __TA_EXCLUDES(lock_);
  int IrqThread();

  // fuchsia_hardware_usb_dci::UsbDci protocol implementation.
  void ConnectToEndpoint(ConnectToEndpointRequest& request,
                         ConnectToEndpointCompleter::Sync& completer) override;

  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;

  void StartController(StartControllerCompleter::Sync& completer) override;

  void StopController(StopControllerCompleter::Sync& completer) override;

  void ConfigureEndpoint(ConfigureEndpointRequest& request,
                         ConfigureEndpointCompleter::Sync& completer) override;

  void DisableEndpoint(DisableEndpointRequest& request,
                       DisableEndpointCompleter::Sync& completer) override;

  void EndpointSetStall(EndpointSetStallRequest& request,
                        EndpointSetStallCompleter::Sync& completer) override;

  void EndpointClearStall(EndpointClearStallRequest& request,
                          EndpointClearStallCompleter::Sync& completer) override;

  void CancelAll(CancelAllRequest& request, CancelAllCompleter::Sync& completer) override;

  void GetHardwareInfo(GetHardwareInfoCompleter::Sync& completer) override;
  void AllocEndpoint(AllocEndpointRequest& request,
                     AllocEndpointCompleter::Sync& completer) override;
  void FreeEndpoint(FreeEndpointRequest& request, FreeEndpointCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_usb_dci::UsbDci> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::warn("dwc2: received unknown UsbDci method: {}", metadata.method_ordinal);
  }

  // Allows tests to configure a fake interrupt.
  void SetInterrupt(zx::interrupt irq) { irq_ = std::move(irq); }

  const zx::bti& bti() const { return bti_; }

 private:
  friend class Dwc2Test;

  static inline const uint32_t kEp0BufferSize = UINT16_MAX + 1;

  zx_status_t DoControl(size_t write_size, size_t read_size, size_t* out_read_actual);

  zx_status_t DoControlRead(size_t read_size, size_t* out_read_actual) {
    return DoControl(0, read_size, out_read_actual);
  }

  zx_status_t DoControlWrite(size_t write_size) { return DoControl(write_size, 0, nullptr); }

  enum class Ep0State {
    DISCONNECTED,
    IDLE,
    DATA,
    STATUS,
    STALL,
    TIMEOUT_RECOVERY,
  };

  // clang-format off
  const char* Ep0StateToStr(Ep0State s) {
    switch (s) {
      case Ep0State::DISCONNECTED:     return "DISCONNECTED";
      case Ep0State::IDLE:             return "IDLE";
      case Ep0State::DATA:             return "DATA";
      case Ep0State::STATUS:           return "STATUS";
      case Ep0State::STALL:            return "STALL";
      case Ep0State::TIMEOUT_RECOVERY: return "TIMEOUT_RECOVERY";
    }
  }
  // clang-format on

  class Endpoint : public usb::EndpointServer {
   public:
    Endpoint(uint8_t ep_num, Dwc2* dwc2)
        : usb::EndpointServer(dwc2->bti_, ep_num, usb::ScatterGatherSupport::kUnsupported),
          dwc2_(dwc2) {}

    // fuchsia_hardware_usb_endpoint::Endpoint protocol implementation.
    void GetInfo(GetInfoCompleter::Sync& completer) override {
      completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
    }
    void QueueRequests(QueueRequestsRequest& request,
                       QueueRequestsCompleter::Sync& completer) override;
    void CancelAll(CancelAllCompleter::Sync& completer) override {
      CancelAll();
      completer.Reply(fit::ok());
    }

    void QueueRequest(usb::FidlRequest request);
    void CancelAll();

    // Requests waiting to be processed.
    std::queue<usb::RequestVariant> queued_reqs __TA_GUARDED(lock);
    // Request currently being processed.
    std::optional<usb::RequestVariant> current_req __TA_GUARDED(lock);

    // Values for current USB request
    uint32_t req_offset = 0;
    uint32_t req_xfersize = 0;
    uint32_t req_length = 0;
    uint32_t phys = 0;

    // Used for synchronizing endpoint state and ep specific hardware registers.
    // This should be acquired before Dwc2.lock_ if acquiring both locks.
    std::mutex lock;

    uint16_t max_packet_size = 0;
    bool enabled = false;
    bool pending_zlp = false;

   protected:
    void OnUnbound(fidl::UnbindInfo info,
                   fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) override;

   private:
    Dwc2* dwc2_;
  };

  zx::result<> ResetCore() __TA_REQUIRES(lock_);
  void FlushTxFifo(uint32_t fifo_num) __TA_REQUIRES(lock_);
  void FlushRxFifo() __TA_REQUIRES(lock_);
  void FlushTxFifoRetryIndefinite(uint32_t fifo_num) __TA_REQUIRES(lock_);
  void FlushRxFifoRetryIndefinite() __TA_REQUIRES(lock_);
  zx::result<> InitController() __TA_REQUIRES(lock_);
  void SetConnected(bool connected);
  void StartEp0();
  void StartEndpoints();
  void HandleEp0Setup();
  void StallEp0();
  void HandleEp0Status(bool is_in);
  void HandleEp0TimeoutRecovery() __TA_EXCLUDES(lock_);
  void HandleEp0TransferComplete(bool is_in) __TA_EXCLUDES(lock_);
  void HandleTransferComplete(uint8_t ep_num);
  void EnableEp(uint8_t ep_num, bool enable);
  void QueueNextRequest(Endpoint* ep) __TA_REQUIRES(ep->lock);
  void StartTransfer(Endpoint* ep, uint32_t length) __TA_REQUIRES(ep->lock);
  void SoftDisconnect() __TA_REQUIRES(lock_);
  uint32_t ReadTransfered(Endpoint* ep);

  // Interrupt handlers
  void HandleReset() __TA_EXCLUDES(lock_);
  void HandleSuspend() __TA_EXCLUDES(lock_);
  void HandleEnumDone() __TA_EXCLUDES(lock_);
  void HandleInEpInterrupt() __TA_EXCLUDES(lock_);
  void HandleOutEpInterrupt() __TA_EXCLUDES(lock_);

  zx_status_t HandleSetupRequest(size_t* out_actual);
  void SetAddress(uint8_t address);

  // Wait up to 100 mSec for a specified register to satisfy a user's predicate
  // and then return true, otherwise returns false.
  template <typename Predicate>
  bool WaitForRegisterPredicate(auto reg, Predicate predicate) __TA_REQUIRES(lock_) {
    for (uint32_t i = 0; !predicate(reg.ReadFrom(get_mmio())); ++i) {
      if (i >= 1000) {
        return false;
      }
      zx::nanosleep(zx::deadline_after(zx::usec(100)));
    }
    return true;
  }

  inline fdf::MmioBuffer* get_mmio() { return &*mmio_; }

  // Used for debugging.
  void dump_regs();

  std::optional<Endpoint> endpoints_[DWC_MAX_EPS];

  std::unique_ptr<fdf::PDev> pdev_;

  // Used for synchronizing global state
  // and non ep specific hardware registers.
  // Endpoint.lock should be acquired first
  // when acquiring both locks.
  std::mutex lock_;

  zx::bti bti_;
  // DMA buffer for endpoint zero requests
  std::unique_ptr<dma_buffer::ContiguousBuffer> ep0_buffer_;
  // Current endpoint zero request
  fuchsia_hardware_usb_descriptor::wire::UsbSetup cur_setup_ = {};
  Ep0State ep0_state_ = Ep0State::DISCONNECTED;

  fidl::WireSyncClient<fuchsia_hardware_usb_dci::UsbDciInterface> dci_intf_;
  fidl::SyncClient<fuchsia_hardware_usb_phy::UsbPhy> phy_;

  std::optional<fdf::MmioBuffer> mmio_;
  GSNPSID cached_gsnpsid_;
  GHWCFG2 cached_ghwcfg2_;
  GHWCFG4 cached_ghwcfg4_;

  zx::interrupt irq_;

  fuchsia_hardware_usb_dwc2::Metadata metadata_;
  bool connected_ = false;
  bool configured_ = false;
  // The length of the last IN-data sent to the host.
  uint32_t last_transmission_len_;
  // Raw IRQ timestamp from kernel
  zx::time_boot irq_timestamp_;
  // Timestamp we were dispatched at
  zx::time_boot irq_dispatch_timestamp_;
  // Timestamp when we started waiting for the interrupt
  zx::time_boot wait_start_time_;
  bool shutting_down_ __TA_GUARDED(lock_) = false;

  fdf::SynchronizedDispatcher irq_dispatcher_;
  libsync::Completion irq_thread_stopped_;  // Signled by shutdown handler.
  void DispatcherShutdownHandler(fdf_dispatcher_t* dispatcher);

  fidl::ServerBindingGroup<fuchsia_hardware_usb_dci::UsbDci> bindings_;

  fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>
      mac_address_metadata_server_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::SerialNumberMetadata>
      serial_number_metadata_server_;

  dwc2_config::Config config_;
  fidl::SyncClient<fuchsia_driver_framework::NodeController> child_;
};

}  // namespace dwc2

#endif  // SRC_DEVICES_USB_DRIVERS_DWC2_DWC2_H_
