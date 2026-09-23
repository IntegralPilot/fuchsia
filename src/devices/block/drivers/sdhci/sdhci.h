// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDHCI_SDHCI_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDHCI_SDHCI_H_

#include <fidl/fuchsia.hardware.sdhci/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/driver/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/sdmmc/hw.h>
#include <lib/sync/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/bti.h>
#include <lib/zx/interrupt.h>

#include <mutex>
#include <optional>

#include "dma-descriptor-builder.h"
#include "sdhci-reg.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace sdhci {

class Sdhci : public fdf::DriverBase2, public fdf::WireServer<fuchsia_hardware_sdmmc::Sdmmc> {
 public:
  // Visible for testing.
  struct AdmaDescriptor96 {
    uint16_t attr;
    uint16_t length;
    uint64_t address;

    uint64_t get_address() const {
      uint64_t addr;
      memcpy(&addr, &address, sizeof(addr));
      return addr;
    }
  } __PACKED;
  static_assert(sizeof(AdmaDescriptor96) == 12, "unexpected ADMA2 descriptor size");

  struct AdmaDescriptor64 {
    uint16_t attr;
    uint16_t length;
    uint32_t address;
  } __PACKED;
  static_assert(sizeof(AdmaDescriptor64) == 8, "unexpected ADMA2 descriptor size");

  static constexpr char kDriverName[] = "sdhci";

  explicit Sdhci()
      : fdf::DriverBase2(kDriverName),
        irq_handler_{this},
        virtual_irq_handler_{this},
        virtual_irq_lifeline_wait_{this},
        registered_vmo_stores_{
            // SdmmcVmoStore does not have a default constructor, so construct each one using an
            // empty Options (do not map or pin automatically upon VMO registration).
            // clang-format off
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            // clang-format on
        } {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  void HostInfo(fdf::Arena& arena, HostInfoCompleter::Sync& completer) override;
  void SetSignalVoltage(SetSignalVoltageRequestView request, fdf::Arena& arena,
                        SetSignalVoltageCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void SetBusWidth(SetBusWidthRequestView request, fdf::Arena& arena,
                   SetBusWidthCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void SetBusFreq(SetBusFreqRequestView request, fdf::Arena& arena,
                  SetBusFreqCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void SetTiming(SetTimingRequestView request, fdf::Arena& arena,
                 SetTimingCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void PerformTuning(PerformTuningRequestView request, fdf::Arena& arena,
                     PerformTuningCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void RegisterInBandInterrupt(RegisterInBandInterruptRequestView request, fdf::Arena& arena,
                               RegisterInBandInterruptCompleter::Sync& completer)
      TA_EXCL(mtx_) override;
  void AckInBandInterrupt(fdf::Arena& arena, AckInBandInterruptCompleter::Sync& completer)
      TA_EXCL(mtx_) override;
  void RegisterVmo(RegisterVmoRequestView request, fdf::Arena& arena,
                   RegisterVmoCompleter::Sync& completer) override;
  void UnregisterVmo(UnregisterVmoRequestView request, fdf::Arena& arena,
                     UnregisterVmoCompleter::Sync& completer) override;
  void Request(RequestRequestView request, fdf::Arena& arena, RequestCompleter::Sync& completer)
      TA_EXCL(mtx_) override;

  void EnableCqhci(fdf::Arena& arena, EnableCqhciCompleter::Sync& completer) TA_EXCL(mtx_) override;
  void DisableCqhci(fdf::Arena& arena, DisableCqhciCompleter::Sync& completer)
      TA_EXCL(mtx_) override;
  void InitializeCommandQueueing(InitializeCommandQueueingRequestView request, fdf::Arena& arena,
                                 InitializeCommandQueueingCompleter::Sync& completer)
      TA_EXCL(mtx_) override;

  // Visible for testing.
  uint32_t base_clock() const { return base_clock_; }

 protected:
  // All protected members are visible for testing.
  enum class RequestStatus {
    IDLE,
    COMMAND,
    TRANSFER_DATA_DMA,
    READ_DATA_PIO,
    WRITE_DATA_PIO,
    BUSY_RESPONSE,
  };

  // Resumes handling of the physical IRQ.
  void OnInterruptDelegateStopped();
  void OnLifelineClosed(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                        const zx_packet_signal_t* signal);

  RequestStatus GetRequestStatus() TA_EXCL(&mtx_) {
    std::lock_guard<std::mutex> lock(mtx_);
    if (pending_request_ && !pending_request_->request_complete) {
      const bool has_data = pending_request_->cmd_flags & SDMMC_RESP_DATA_PRESENT;
      const bool busy_response = pending_request_->cmd_flags & SDMMC_RESP_LEN_48B;

      if (!pending_request_->cmd_complete) {
        return RequestStatus::COMMAND;
      }
      if (!pending_request_->data.empty()) {
        if (pending_request_->cmd_flags & SDMMC_CMD_READ) {
          return RequestStatus::READ_DATA_PIO;
        }
        return RequestStatus::WRITE_DATA_PIO;
      }
      if (has_data) {
        return RequestStatus::TRANSFER_DATA_DMA;
      }
      if (busy_response) {
        return RequestStatus::BUSY_RESPONSE;
      }
    }
    return RequestStatus::IDLE;
  }

  // Override to inject dependency for unit testing.
  virtual zx_status_t InitMmio();
  virtual zx_status_t InitCqhciMmio();
  virtual zx_status_t WaitForReset(SoftwareReset mask);
  async_dispatcher_t* irq_dispatcher() const { return irq_dispatcher_.async_dispatcher(); }

  std::optional<fdf::MmioBuffer> regs_mmio_buffer_;
  std::optional<fdf::MmioBuffer> regs_cqhci_mmio_buffer_;

  // DMA descriptors, visible for testing
  std::unique_ptr<dma_buffer::ContiguousBuffer> iobuf_;

 private:
  struct OwnedVmoInfo {
    uint64_t offset;
    uint64_t size;
    fuchsia_hardware_sdmmc::SdmmcVmoRight rights;
  };

  // Used to synchronize the request thread(s) with the interrupt thread for requests through
  // SdmmcRequest. See above for SdmmcRequest requests.
  struct PendingRequest {
    explicit PendingRequest(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request)
        : cmd_idx(request.cmd_idx),
          cmd_flags(request.cmd_flags),
          blocksize(request.blocksize),
          status(InterruptStatus::Get().FromValue(0).set_error(1)) {}

    bool data_transfer_complete() const {
      return !(cmd_flags & SDMMC_RESP_DATA_PRESENT) || data.empty();
    }

    const uint32_t cmd_idx;
    const uint32_t cmd_flags;
    const uint32_t blocksize;

    // If false, a command is in progress on the bus, and the interrupt thread is waiting for the
    // command complete interrupt.
    bool cmd_complete = false;

    // If true, all stages of the request have completed, and the main thread has been signaled.
    bool request_complete = false;

    // The 0-, 32-, or 128-bit response (unused fields set to zero). Set by the interrupt thread and
    // read by the request thread.
    uint32_t response[4] = {};

    // If an error occurred, the interrupt thread sets this field to the value of the status
    // register (and always sets the general error bit). If no error  occurred the interrupt thread
    // sets this field to zero.
    InterruptStatus status;

    // For a non-DMA request, data points to the buffer to read from/write to. This buffer may be
    // owned by vmo_mapper.
    cpp20::span<uint8_t> data;
    fzl::VmoMapper vmo_mapper;
  };

  using BlockSizeType = decltype(BlockSize::Get().FromValue(0).reg_value());
  using BlockCountType = decltype(BlockCount::Get().FromValue(0).reg_value());
  using SdmmcVmoStore = DmaDescriptorBuilder<OwnedVmoInfo>::VmoStore;

  static void PrepareCmd(const fuchsia_hardware_sdmmc::wire::SdmmcReq& req,
                         TransferMode* transfer_mode, Command* command);

  zx_status_t Init();

  bool SupportsAdma2() const {
    return (info_.caps & fuchsia_hardware_sdmmc::SdmmcHostCap::kDma) &&
           !(quirks_ & fuchsia_hardware_sdhci::Quirk::kNoDma);
  }

  void EnableInterrupts() TA_REQ(mtx_);
  void DisableInterrupts() TA_REQ(mtx_);

  zx_status_t WaitForInhibit(PresentState mask) const;
  zx_status_t WaitForInternalClockStable() const;

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt) TA_EXCL(mtx_);
  void HandleTransferInterrupt(InterruptStatus status) TA_REQ(mtx_);

  zx::result<fidl::Array<uint32_t, 4>> Request(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& request);

  zx::result<PendingRequest> StartRequest(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
                                          DmaDescriptorBuilder<OwnedVmoInfo>& builder) TA_REQ(mtx_);
  zx_status_t SetUpDma(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
                       DmaDescriptorBuilder<OwnedVmoInfo>& builder) TA_REQ(mtx_);
  zx_status_t SetUpBuffer(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
                          PendingRequest* pending_request) TA_REQ(mtx_);
  zx::result<fidl::Array<uint32_t, 4>> FinishRequest(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& request, const PendingRequest& pending_request)
      TA_REQ(mtx_);

  void CompleteRequest() TA_REQ(mtx_);

  // Always signals the main thread.
  void ErrorRecovery() TA_REQ(mtx_);

  // These return true if the main thread was signaled and no further processing is needed.
  bool CmdStageComplete() TA_REQ(mtx_);
  void TransferComplete() TA_REQ(mtx_);
  bool DataStageReadReady() TA_REQ(mtx_);
  void DataStageWriteReady() TA_REQ(mtx_);

  zx_status_t SetBusClock(uint32_t frequency_hz);

  zx_status_t PerformVendorTuning(uint32_t cmd_idx);

  zx::interrupt irq_;  // Always holds the physical interrupt
  async::IrqMethod<Sdhci, &Sdhci::HandleIrq> irq_handler_;

  zx::interrupt virtual_irq_ TA_GUARDED(mtx_);
  async::IrqMethod<Sdhci, &Sdhci::HandleIrq> virtual_irq_handler_;
  zx::eventpair virtual_irq_lifeline_ TA_GUARDED(mtx_);
  async::WaitMethod<Sdhci, &Sdhci::OnLifelineClosed> virtual_irq_lifeline_wait_;

  fdf::WireSyncClient<fuchsia_hardware_sdhci::Device> sdhci_;
  fdf::Arena arena_{'SDHC'};

  zx::bti bti_;

  // Held when a command or action is in progress.
  std::mutex mtx_;

  // used to signal request complete
  sync_completion_t req_completion_;

  // Controller info
  fuchsia_hardware_sdmmc::wire::SdmmcHostInfo info_ = {};

  // Controller specific quirks
  fuchsia_hardware_sdhci::Quirk quirks_;
  uint64_t dma_boundary_alignment_;

  // Base clock rate
  uint32_t base_clock_ = 0;

  fdf::WireSharedClient<fuchsia_hardware_sdmmc::InBandInterrupt> interrupt_cb_;
  bool card_interrupt_masked_ TA_GUARDED(mtx_) = false;
  bool cqhci_enabled_ TA_GUARDED(mtx_) = false;

  // Set to true if the device has inline crypto support.
  bool supports_inline_crypto_ = false;

  // Keep one SdmmcVmoStore for each possible client ID (IDs are in [0,
  // fuchsia_hardware_sdmmc::wire::kSdmmcMaxClientId]).
  std::array<SdmmcVmoStore, fuchsia_hardware_sdmmc::wire::kSdmmcMaxClientId + 1>
      registered_vmo_stores_;

  std::optional<PendingRequest> pending_request_ TA_GUARDED(mtx_);

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;

  fdf_metadata::MetadataServer<fuchsia_hardware_sdmmc::SdmmcMetadata> metadata_server_;
  fdf::Dispatcher irq_dispatcher_;

  fdf::ServerBindingGroup<fuchsia_hardware_sdmmc::Sdmmc> bindings_;

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<fdf::StopCompleter> stop_completer_ TA_GUARDED(mtx_);
  bool shutdown_ TA_GUARDED(mtx_) = false;
};

}  // namespace sdhci

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDHCI_SDHCI_H_
