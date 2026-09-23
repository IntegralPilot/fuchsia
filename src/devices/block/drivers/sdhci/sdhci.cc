// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Notes and limitations:
// 1. This driver only supports SDHCv3 and above. Lower versions of SD are not
//    currently supported. The driver should fail gracefully if a lower version
//    card is detected.

#include "sdhci.h"

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/pmt.h>
#include <lib/zx/time.h>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>

#include "src/devices/block/drivers/sdhci/sdhci_config.h"

namespace {

const std::string kIrqProfileName = "fuchsia.devices.sdhci.irq";

constexpr uint32_t kSdFreqSetupHz = 400'000;

constexpr int kMaxTuningCount = 40;

constexpr zx_paddr_t k32BitPhysAddrMask = 0xffff'ffff;

constexpr zx::duration kResetTime = zx::sec(1);
constexpr zx::duration kClockStabilizationTime = zx::msec(150);
constexpr zx::duration kVoltageStabilizationTime = zx::msec(5);
constexpr zx::duration kInhibitWaitTime = zx::msec(1);
constexpr zx::duration kWaitYieldTime = zx::usec(1);

constexpr uint32_t Hi32(zx_paddr_t val) { return static_cast<uint32_t>((val >> 32) & 0xffffffff); }
constexpr uint32_t Lo32(zx_paddr_t val) { return val & 0xffffffff; }

// for 2M max transfer size for fully discontiguous
// also see SDMMC_PAGES_COUNT in fuchsia.hardware.sdmmc
constexpr int kDmaDescCount = 512;

// We should be able to query at least the crypto capabilities from 0x7C + 4.
constexpr size_t kMinimumCqhciMmioSize = (0x7C + 4);

uint16_t GetClockDividerValue(const uint32_t base_clock, const uint32_t target_rate) {
  if (target_rate >= base_clock) {
    // A clock divider of 0 means "don't divide the clock"
    // If the base clock is already slow enough to use as the SD clock then
    // we don't need to divide it any further.
    return 0;
  }

  // SDHCI Versions 1.00 and 2.00 handle the clock divider slightly
  // differently compared to SDHCI version 3.00. Since this driver doesn't
  // support SDHCI versions < 3.00, we ignore this incongruency for now.
  //
  // V3.00 supports a 10 bit divider where the SD clock frequency is defined
  // as F/(2*D) where F is the base clock frequency and D is the divider.
  uint32_t result = base_clock / (2 * target_rate);
  if (result * target_rate * 2 < base_clock)
    result++;

  return std::min(sdhci::ClockControl::kMaxFrequencySelect, static_cast<uint16_t>(result));
}

}  // namespace

namespace sdhci {

void Sdhci::PrepareCmd(const fuchsia_hardware_sdmmc::wire::SdmmcReq& req,
                       TransferMode* transfer_mode, Command* command) {
  command->set_command_index(static_cast<uint16_t>(req.cmd_idx));

  if (req.cmd_flags & SDMMC_RESP_LEN_EMPTY) {
    command->set_response_type(Command::kResponseTypeNone);
  } else if (req.cmd_flags & SDMMC_RESP_LEN_136) {
    command->set_response_type(Command::kResponseType136Bits);
  } else if (req.cmd_flags & SDMMC_RESP_LEN_48) {
    command->set_response_type(Command::kResponseType48Bits);
  } else if (req.cmd_flags & SDMMC_RESP_LEN_48B) {
    command->set_response_type(Command::kResponseType48BitsWithBusy);
  }

  if (req.cmd_flags & SDMMC_CMD_TYPE_NORMAL) {
    command->set_command_type(Command::kCommandTypeNormal);
  } else if (req.cmd_flags & SDMMC_CMD_TYPE_SUSPEND) {
    command->set_command_type(Command::kCommandTypeSuspend);
  } else if (req.cmd_flags & SDMMC_CMD_TYPE_RESUME) {
    command->set_command_type(Command::kCommandTypeResume);
  } else if (req.cmd_flags & SDMMC_CMD_TYPE_ABORT) {
    command->set_command_type(Command::kCommandTypeAbort);
  }

  if (req.cmd_flags & SDMMC_CMD_AUTO12) {
    transfer_mode->set_auto_cmd_enable(TransferMode::kAutoCmd12);
  } else if (req.cmd_flags & SDMMC_CMD_AUTO23) {
    transfer_mode->set_auto_cmd_enable(TransferMode::kAutoCmd23);
  }

  if (req.cmd_flags & SDMMC_RESP_CRC_CHECK) {
    command->set_command_crc_check(1);
  }
  if (req.cmd_flags & SDMMC_RESP_CMD_IDX_CHECK) {
    command->set_command_index_check(1);
  }
  if (req.cmd_flags & SDMMC_RESP_DATA_PRESENT) {
    command->set_data_present(1);
  }
  if (req.cmd_flags & SDMMC_CMD_READ) {
    transfer_mode->set_read(1);
  }
}

zx_status_t Sdhci::WaitForReset(const SoftwareReset mask) {
  const zx::time deadline = zx::clock::get_monotonic() + kResetTime;
  do {
    if ((SoftwareReset::Get().ReadFrom(&*regs_mmio_buffer_).reg_value() & mask.reg_value()) == 0) {
      return ZX_OK;
    }
    zx::nanosleep(zx::deadline_after(kWaitYieldTime));
  } while (zx::clock::get_monotonic() <= deadline);

  fdf::error("sdhci: timed out while waiting for reset");
  return ZX_ERR_TIMED_OUT;
}

void Sdhci::EnableInterrupts() {
  InterruptSignalEnable::Get()
      .FromValue(0)
      .EnableErrorInterrupts()
      .EnableNormalInterrupts()
      .set_card_interrupt(interrupt_cb_.is_valid() ? 1 : 0)
      .WriteTo(&*regs_mmio_buffer_);
  InterruptStatusEnable::Get()
      .FromValue(0)
      .EnableErrorInterrupts()
      .EnableNormalInterrupts()
      .set_card_interrupt((interrupt_cb_.is_valid() && !card_interrupt_masked_) ? 1 : 0)
      .WriteTo(&*regs_mmio_buffer_);
}

void Sdhci::DisableInterrupts() {
  InterruptSignalEnable::Get()
      .FromValue(0)
      .set_card_interrupt(interrupt_cb_.is_valid() ? 1 : 0)
      .WriteTo(&*regs_mmio_buffer_);
  InterruptStatusEnable::Get()
      .FromValue(0)
      .set_card_interrupt((interrupt_cb_.is_valid() && !card_interrupt_masked_) ? 1 : 0)
      .WriteTo(&*regs_mmio_buffer_);
}

zx_status_t Sdhci::WaitForInhibit(const PresentState mask) const {
  const zx::time deadline = zx::clock::get_monotonic() + kInhibitWaitTime;
  do {
    if ((PresentState::Get().ReadFrom(&*regs_mmio_buffer_).reg_value() & mask.reg_value()) == 0) {
      return ZX_OK;
    }
    zx::nanosleep(zx::deadline_after(kWaitYieldTime));
  } while (zx::clock::get_monotonic() <= deadline);

  fdf::error("sdhci: timed out while waiting for command/data inhibit");
  return ZX_ERR_TIMED_OUT;
}

zx_status_t Sdhci::WaitForInternalClockStable() const {
  const zx::time deadline = zx::clock::get_monotonic() + kClockStabilizationTime;
  do {
    if ((ClockControl::Get().ReadFrom(&*regs_mmio_buffer_).internal_clock_stable())) {
      return ZX_OK;
    }
    zx::nanosleep(zx::deadline_after(kWaitYieldTime));
  } while (zx::clock::get_monotonic() <= deadline);

  fdf::error("sdhci: timed out while waiting for internal clock to stabilize");
  return ZX_ERR_TIMED_OUT;
}

bool Sdhci::CmdStageComplete() {
  const uint32_t response_0 = Response::Get(0).ReadFrom(&*regs_mmio_buffer_).reg_value();
  const uint32_t response_1 = Response::Get(1).ReadFrom(&*regs_mmio_buffer_).reg_value();
  const uint32_t response_2 = Response::Get(2).ReadFrom(&*regs_mmio_buffer_).reg_value();
  const uint32_t response_3 = Response::Get(3).ReadFrom(&*regs_mmio_buffer_).reg_value();

  // Read the response data.
  if (pending_request_->cmd_flags & SDMMC_RESP_LEN_136) {
    if (quirks_ & fuchsia_hardware_sdhci::Quirk::kStripResponseCrc) {
      pending_request_->response[0] = (response_3 << 8) | ((response_2 >> 24) & 0xFF);
      pending_request_->response[1] = (response_2 << 8) | ((response_1 >> 24) & 0xFF);
      pending_request_->response[2] = (response_1 << 8) | ((response_0 >> 24) & 0xFF);
      pending_request_->response[3] = (response_0 << 8);
    } else if (quirks_ & fuchsia_hardware_sdhci::Quirk::kStripResponseCrcPreserveOrder) {
      pending_request_->response[0] = (response_0 << 8);
      pending_request_->response[1] = (response_1 << 8) | ((response_0 >> 24) & 0xFF);
      pending_request_->response[2] = (response_2 << 8) | ((response_1 >> 24) & 0xFF);
      pending_request_->response[3] = (response_3 << 8) | ((response_2 >> 24) & 0xFF);
    } else {
      pending_request_->response[0] = response_0;
      pending_request_->response[1] = response_1;
      pending_request_->response[2] = response_2;
      pending_request_->response[3] = response_3;
    }
  } else if (pending_request_->cmd_flags & (SDMMC_RESP_LEN_48 | SDMMC_RESP_LEN_48B)) {
    pending_request_->response[0] = response_0;
  }

  pending_request_->cmd_complete = true;

  if (pending_request_->cmd_flags & (SDMMC_RESP_DATA_PRESENT | SDMMC_RESP_LEN_48B)) {
    return false;
  }

  // We're done if the command has no data or busy stage
  CompleteRequest();
  return true;
}

void Sdhci::TransferComplete() {
  if (!pending_request_->cmd_complete) {
    fdf::error("Transfer complete interrupt received before command complete");
    pending_request_->status.set_error(1).set_command_timeout_error(1);
    ErrorRecovery();
  } else if (!pending_request_->data_transfer_complete()) {
    fdf::error("Transfer complete interrupt received before data transferred");
    pending_request_->status.set_error(1).set_data_timeout_error(1);
    ErrorRecovery();
  } else {
    CompleteRequest();
  }
}

bool Sdhci::DataStageReadReady() {
  if ((pending_request_->cmd_idx == MMC_SEND_TUNING_BLOCK) ||
      (pending_request_->cmd_idx == SD_SEND_TUNING_BLOCK)) {
    // This is the final interrupt expected for tuning transfers.
    CompleteRequest();
    return true;
  }

  if (SupportsAdma2() || pending_request_->data_transfer_complete()) {
    return false;
  }

  for (uint32_t i = 0; i < pending_request_->blocksize; i += sizeof(uint32_t)) {
    const uint32_t data = BufferData::Get().ReadFrom(&*regs_mmio_buffer_).reg_value();
    memcpy(pending_request_->data.data(), &data, sizeof(data));
    pending_request_->data = pending_request_->data.subspan(sizeof(data));
  }

  return false;
}

void Sdhci::DataStageWriteReady() {
  if (SupportsAdma2() || pending_request_->data_transfer_complete()) {
    return;
  }

  for (uint32_t i = 0; i < pending_request_->blocksize; i += sizeof(uint32_t)) {
    uint32_t data;
    memcpy(&data, pending_request_->data.data(), sizeof(data));
    pending_request_->data = pending_request_->data.subspan(sizeof(data));
    BufferData::Get().FromValue(data).WriteTo(&*regs_mmio_buffer_);
  }
}

void Sdhci::ErrorRecovery() {
  // Reset internal state machines
  {
    SoftwareReset::Get()
        .ReadFrom(&*regs_mmio_buffer_)
        .set_reset_cmd(1)
        .WriteTo(&*regs_mmio_buffer_);
    [[maybe_unused]] auto _ = WaitForReset(SoftwareReset::Get().FromValue(0).set_reset_cmd(1));
  }
  {
    SoftwareReset::Get()
        .ReadFrom(&*regs_mmio_buffer_)
        .set_reset_dat(1)
        .WriteTo(&*regs_mmio_buffer_);
    [[maybe_unused]] auto _ = WaitForReset(SoftwareReset::Get().FromValue(0).set_reset_dat(1));
  }

  // Complete any pending txn with error status
  CompleteRequest();
}

void Sdhci::CompleteRequest() {
  pending_request_->request_complete = true;
  DisableInterrupts();
  sync_completion_signal(&req_completion_);
}

void Sdhci::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                      const zx_packet_interrupt_t* interrupt) {
  if (status != ZX_OK) {
    if (status != ZX_ERR_CANCELED) {
      fdf::error("Failed to wait for interrupt: {}", zx_status_get_string(status));
    }
    return;
  }

  // Acknowledge the IRQs that we stashed. IRQs are cleared by writing
  // 1s into the IRQs that fired.
  auto interrupt_status =
      InterruptStatus::Get().ReadFrom(&*regs_mmio_buffer_).WriteTo(&*regs_mmio_buffer_);

  fdf::debug("got irq 0x{:08x} en 0x{:08x}", interrupt_status.reg_value(),
             InterruptSignalEnable::Get().ReadFrom(&*regs_mmio_buffer_).reg_value());

  std::lock_guard<std::mutex> lock(mtx_);
  if (pending_request_ && !pending_request_->request_complete) {
    HandleTransferInterrupt(interrupt_status);
  }

  if (interrupt_status.card_interrupt()) {
    // Disable the card interrupt and call the callback if there is one.
    InterruptStatusEnable::Get()
        .ReadFrom(&*regs_mmio_buffer_)
        .set_card_interrupt(0)
        .WriteTo(&*regs_mmio_buffer_);
    card_interrupt_masked_ = true;
    if (interrupt_cb_.is_valid()) {
      fdf::Arena arena('SDHC');
      // TODO(436824088): Clean this up after changes to the in-band interrupt.
      interrupt_cb_.buffer(arena)->Callback().Then(
          [this](
              fdf::WireUnownedResult<fuchsia_hardware_sdmmc::InBandInterrupt::Callback>& result) {
            if (!result.ok()) {
              // This could be called synchronously during teardown, so post a task to prevent
              // double locking.
              async::PostTask(fdf::Dispatcher::GetCurrent()->async_dispatcher(), [this]() {
                std::lock_guard<std::mutex> lock(mtx_);
                interrupt_cb_ = {};
              });
            }
          });
    }
  }

  zx::unowned_interrupt(irq->object())->ack();
}

void Sdhci::HandleTransferInterrupt(const InterruptStatus status) {
  if (status.ErrorInterrupt()) {
    pending_request_->status = status;
    pending_request_->status.set_error(1);
    ErrorRecovery();
    return;
  }

  // Clear the interrupt status to indicate that a normal interrupt was handled.
  pending_request_->status = InterruptStatus::Get().FromValue(0);
  if (status.command_complete() && CmdStageComplete()) {
    return;
  }
  if (status.buffer_read_ready() && DataStageReadReady()) {
    return;
  }
  if (status.buffer_write_ready()) {
    DataStageWriteReady();
  }
  if (status.transfer_complete()) {
    TransferComplete();
  }
}

void Sdhci::RegisterVmo(RegisterVmoRequestView request, fdf::Arena& arena,
                        RegisterVmoCompleter::Sync& completer) {
  if (request->client_id >= std::size(registered_vmo_stores_)) {
    completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }
  if (!(request->vmo_rights & (fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead |
                               fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite))) {
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  vmo_store::StoredVmo<OwnedVmoInfo> stored_vmo(std::move(request->vmo),
                                                OwnedVmoInfo{
                                                    .offset = request->offset,
                                                    .size = request->size,
                                                    .rights = request->vmo_rights,
                                                });

  const uint32_t write_perm =
      (request->vmo_rights & fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite) ? ZX_BTI_PERM_WRITE : 0;

  if (SupportsAdma2()) {
    const uint32_t read_perm =
        (request->vmo_rights & fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead) ? ZX_BTI_PERM_READ : 0;

    zx_status_t status = stored_vmo.Pin(bti_, read_perm | write_perm, true);
    if (status != ZX_OK) {
      fdf::error("Failed to pin VMO {} for client {}: {}", request->vmo_id, request->client_id,
                 zx_status_get_string(status));
      completer.buffer(arena).ReplyError(status);
      return;
    }
  } else {
    zx_status_t status = stored_vmo.Map(ZX_VM_PERM_READ | write_perm);
    if (status != ZX_OK) {
      fdf::error("Failed to map VMO {} for client {}: {}", request->vmo_id, request->client_id,
                 zx_status_get_string(status));
      completer.buffer(arena).ReplyError(status);
      return;
    }
    if (request->offset > stored_vmo.data().size() ||
        request->size > (stored_vmo.data().size() - request->offset)) {
      fdf::error("Invalid size or offset for VMO {} for client {}: {}", request->vmo_id,
                 request->client_id, zx_status_get_string(status));
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
  }

  completer.buffer(arena).Reply(
      zx::make_result(registered_vmo_stores_[request->client_id].RegisterWithKey(
          request->vmo_id, std::move(stored_vmo))));
}

void Sdhci::UnregisterVmo(UnregisterVmoRequestView request, fdf::Arena& arena,
                          UnregisterVmoCompleter::Sync& completer) {
  if (request->client_id >= std::size(registered_vmo_stores_)) {
    completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
    return;
  }

  vmo_store::StoredVmo<OwnedVmoInfo>* const vmo_info =
      registered_vmo_stores_[request->client_id].GetVmo(request->vmo_id);
  if (!vmo_info) {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  zx::vmo out_vmo;
  zx_status_t status = vmo_info->vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &out_vmo);
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
    return;
  }

  status = registered_vmo_stores_[request->client_id].Unregister(request->vmo_id).status_value();
  if (status != ZX_OK) {
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess(std::move(out_vmo));
}

void Sdhci::Request(RequestRequestView request, fdf::Arena& arena,
                    RequestCompleter::Sync& completer) {
  fidl::Array<uint32_t, 4> out_response;
  for (const fuchsia_hardware_sdmmc::wire::SdmmcReq& request : request->reqs) {
    out_response = {};
    if (zx::result<fidl::Array<uint32_t, 4>> result = Request(request); result.is_ok()) {
      out_response = result.value();
    } else {
      completer.buffer(arena).Reply(result.take_error());
      return;
    }
  }

  completer.buffer(arena).ReplySuccess(out_response);
}

void Sdhci::EnableCqhci(fdf::Arena& arena, EnableCqhciCompleter::Sync& completer) {
  fdf::debug("Enabling CQHCI");
  std::lock_guard<std::mutex> lock(mtx_);
  if (pending_request_) {
    fdf::error("Enabled CQHCI with inflight request");
    completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  // CQE requires 512-byte blocks.
  BlockSize::Get().FromValue(512).WriteTo(&*regs_mmio_buffer_);

  // Set the command timeout.
  TimeoutControl::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_data_timeout_counter(TimeoutControl::kDataTimeoutMax)
      .WriteTo(&*regs_mmio_buffer_);

  cqhci_enabled_ = true;
  completer.buffer(arena).ReplySuccess();
}

void Sdhci::DisableCqhci(fdf::Arena& arena, DisableCqhciCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mtx_);
  if (!cqhci_enabled_) {
    fdf::error("Disabled CQHCI before enabling");
    completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  fdf::debug("Disabling CQHCI");

  InterruptSignalEnable::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_cqhci_interrupt(0)
      .EnableErrorInterrupts()
      .WriteTo(&*regs_mmio_buffer_);
  InterruptStatusEnable::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_cqhci_interrupt(0)
      .EnableErrorInterrupts()
      .WriteTo(&*regs_mmio_buffer_);
  // Command complete bit is latched during CQE command transfers, so be sure to clear the interrupt
  // now.
  InterruptStatus::Get().FromValue(0).set_command_complete(1).WriteTo(&*regs_mmio_buffer_);
  cqhci_enabled_ = false;
  completer.buffer(arena).ReplySuccess();
}

void Sdhci::OnLifelineClosed(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                             zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    fdf::error("Lifeline wait failed: {}", zx_status_get_string(status));
    return;
  }
  if (signal->observed & ZX_EVENTPAIR_PEER_CLOSED) {
    OnInterruptDelegateStopped();
  }
}

void Sdhci::OnInterruptDelegateStopped() {
  std::lock_guard<std::mutex> lock(mtx_);
  fdf::debug("sdhci: OnInterruptDelegateStopped");
  virtual_irq_handler_.Cancel();
  virtual_irq_lifeline_wait_.Cancel();
  virtual_irq_.reset();
  virtual_irq_lifeline_.reset();

  if (shutdown_) {
    return;
  }

  if (zx_status_t status = irq_handler_.Begin(irq_dispatcher_.async_dispatcher());
      status != ZX_OK) {
    fdf::error("Failed to bind interrupt to dispatcher: {}", zx_status_get_string(status));
  }
}

void Sdhci::InitializeCommandQueueing(InitializeCommandQueueingRequestView request,
                                      fdf::Arena& arena,
                                      InitializeCommandQueueingCompleter::Sync& completer) {
  zx::bti bti;
  zx::vmo sdhci_mmio;
  zx::vmo cqhci_mmio;
  zx::interrupt physical_interrupt;

  {
    std::lock_guard<std::mutex> lock(mtx_);
    if (cqhci_enabled_) {
      completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
      return;
    }

    if (virtual_irq_) {
      completer.buffer(arena).ReplyError(ZX_ERR_ALREADY_BOUND);
      return;
    }

    zx_status_t status = bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, &bti);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }

    status = regs_mmio_buffer_->get_vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &sdhci_mmio);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }

    status = regs_cqhci_mmio_buffer_->get_vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &cqhci_mmio);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }

    if (zx_status_t status = irq_.duplicate(ZX_RIGHT_SAME_RIGHTS, &physical_interrupt);
        status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
  }

  sync_completion_t completion;
  async::PostTask(
      irq_dispatcher_.async_dispatcher(),
      [this, &completion, virtual_interrupt = std::move(request->virtual_interrupt),
       virtual_interrupt_lifeline = std::move(request->virtual_interrupt_lifeline)]() mutable {
        {
          std::lock_guard<std::mutex> lock(mtx_);
          virtual_irq_ = std::move(virtual_interrupt);
          virtual_irq_lifeline_ = std::move(virtual_interrupt_lifeline);
          virtual_irq_handler_.set_object(virtual_irq_.get());
          virtual_irq_lifeline_wait_.set_object(virtual_irq_lifeline_.get());
          virtual_irq_lifeline_wait_.set_trigger(ZX_EVENTPAIR_PEER_CLOSED);

          if (zx_status_t status = virtual_irq_handler_.Begin(irq_dispatcher_.async_dispatcher());
              status != ZX_OK) {
            fdf::error("Failed to bind virtual irq to dispatcher: {}",
                       zx_status_get_string(status));
          }
          if (zx_status_t status =
                  virtual_irq_lifeline_wait_.Begin(irq_dispatcher_.async_dispatcher());
              status != ZX_OK) {
            fdf::error("Failed to bind virtual irq lifeline wait to dispatcher: {}",
                       zx_status_get_string(status));
          }
        }
        if (zx_status_t status = irq_handler_.Cancel(); status != ZX_OK) {
          fdf::error("Failed to unbind interrupt: {}", zx_status_get_string(status));
        }
        sync_completion_signal(&completion);
      });
  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);

  completer.buffer(arena).ReplySuccess(std::move(cqhci_mmio), regs_cqhci_mmio_buffer_->get_offset(),
                                       std::move(sdhci_mmio), regs_mmio_buffer_->get_offset(),
                                       std::move(bti), std::move(physical_interrupt));
}

zx::result<fidl::Array<uint32_t, 4>> Sdhci::Request(
    const fuchsia_hardware_sdmmc::wire::SdmmcReq& request) {
  TRACE_DURATION("sdhci", "request", "cmd", request.cmd_idx, "arg", request.arg);
  if (request.client_id >= std::size(registered_vmo_stores_)) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  DmaDescriptorBuilder<OwnedVmoInfo> builder(request, registered_vmo_stores_[request.client_id],
                                             dma_boundary_alignment_, bti_.borrow());

  {
    std::lock_guard<std::mutex> lock(mtx_);

    if (shutdown_) {
      return zx::error(ZX_ERR_CANCELED);
    }
    // one command at a time
    if (pending_request_) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }

    if (cqhci_enabled_) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    if (request.use_inline_crypto) {
      if (!supports_inline_crypto_) {
        return zx::error(ZX_ERR_NOT_SUPPORTED);
      }
      CryptoNonQueueParameters::Get()
          .FromValue(0)
          .set_crypto_config_idx(request.slot)
          .set_crypto_enable(true)
          .WriteTo(&*regs_cqhci_mmio_buffer_);
      CryptoNonQueueDun::Get().FromValue(0).set_dun(request.dun).WriteTo(&*regs_cqhci_mmio_buffer_);
    } else if (supports_inline_crypto_) {
      // The controller supports inline crypto but this request doesn't use it. Make sure we disable
      // the crypto configuration for this request.
      CryptoNonQueueParameters::Get().FromValue(0).set_crypto_enable(false).WriteTo(
          &*regs_cqhci_mmio_buffer_);
    }

    if (zx::result pending_request = StartRequest(request, builder); pending_request.is_ok()) {
      pending_request_.emplace(*std::move(pending_request));
    } else {
      return pending_request.take_error();
    }
  }

  sync_completion_wait(&req_completion_, ZX_TIME_INFINITE);
  sync_completion_reset(&req_completion_);

  std::lock_guard<std::mutex> lock(mtx_);

  PendingRequest pending_request = *std::move(pending_request_);
  pending_request_.reset();

  zx::result<fidl::Array<uint32_t, 4>> response = FinishRequest(request, pending_request);

  if (stop_completer_) {
    (*stop_completer_)(zx::ok());
    stop_completer_.reset();
  }

  return response;
}

zx::result<Sdhci::PendingRequest> Sdhci::StartRequest(
    const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
    DmaDescriptorBuilder<OwnedVmoInfo>& builder) {
  // Every command requires that the Command Inhibit is unset.
  auto inhibit_mask = PresentState::Get().FromValue(0).set_command_inhibit_cmd(1);
  if (request.cmd_flags & SDMMC_RESP_DATA_PRESENT) {
    inhibit_mask.set_command_inhibit_dat(1);
  }

  // Busy type commands must also wait for the DATA Inhibit to be 0 UNLESS
  // it's an abort command which can be issued with the data lines active.
  if ((request.cmd_flags & SDMMC_RESP_LEN_48B) && !(request.cmd_flags & SDMMC_CMD_TYPE_ABORT)) {
    inhibit_mask.set_command_inhibit_dat(1);
  }

  // Wait for the inhibit masks from above to become 0 before issuing the command.
  zx_status_t status = WaitForInhibit(inhibit_mask);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  TransferMode transfer_mode = TransferMode::Get().FromValue(0);

  const bool is_tuning_request =
      request.cmd_idx == MMC_SEND_TUNING_BLOCK || request.cmd_idx == SD_SEND_TUNING_BLOCK;

  const auto blocksize = static_cast<BlockSizeType>(request.blocksize);

  PendingRequest pending_request(request);

  if (is_tuning_request) {
    // The SDHCI controller has special logic to handle tuning transfers, so there is no need to set
    // up any DMA buffers.
    BlockSize::Get().FromValue(blocksize).WriteTo(&*regs_mmio_buffer_);
    BlockCount::Get().FromValue(0).WriteTo(&*regs_mmio_buffer_);
  } else if (request.cmd_flags & SDMMC_RESP_DATA_PRESENT) {
    if (request.blocksize > std::numeric_limits<BlockSizeType>::max()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    if (request.blocksize == 0) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    size_t blockcount = 0;
    if (SupportsAdma2()) {
      if (zx_status_t status = SetUpDma(request, builder); status != ZX_OK) {
        return zx::error(status);
      }

      blockcount = builder.block_count();
      transfer_mode.set_dma_enable(1);
    } else {
      if (zx_status_t status = SetUpBuffer(request, &pending_request)) {
        return zx::error(status);
      }

      blockcount = pending_request.data.size() / blocksize;
      transfer_mode.set_dma_enable(0);
    }

    if (blockcount > std::numeric_limits<BlockCountType>::max()) {
      fdf::error("Block count ({}) exceeds the maximum ({})", blockcount,
                 std::numeric_limits<BlockCountType>::max());
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }

    transfer_mode.set_multi_block(blockcount > 1 ? 1 : 0).set_block_count_enable(1);

    BlockSize::Get().FromValue(blocksize).WriteTo(&*regs_mmio_buffer_);
    BlockCount::Get()
        .FromValue(static_cast<BlockCountType>(blockcount))
        .WriteTo(&*regs_mmio_buffer_);
  } else {
    BlockSize::Get().FromValue(0).WriteTo(&*regs_mmio_buffer_);
    BlockCount::Get().FromValue(0).WriteTo(&*regs_mmio_buffer_);
  }

  Command command = Command::Get().FromValue(0);
  PrepareCmd(request, &transfer_mode, &command);

  Argument::Get().FromValue(request.arg).WriteTo(&*regs_mmio_buffer_);

  // Clear any pending interrupts before starting the transaction.
  auto irq_mask = InterruptSignalEnable::Get().ReadFrom(&*regs_mmio_buffer_);
  InterruptStatus::Get().FromValue(irq_mask.reg_value()).WriteTo(&*regs_mmio_buffer_);

  // Unmask and enable interrupts
  EnableInterrupts();

  // Start command
  transfer_mode.WriteTo(&*regs_mmio_buffer_);
  command.WriteTo(&*regs_mmio_buffer_);

  return zx::ok(std::move(pending_request));
}

zx_status_t Sdhci::SetUpDma(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
                            DmaDescriptorBuilder<OwnedVmoInfo>& builder) {
  zx_status_t status;
  for (const auto& buffer : request.buffers) {
    if (status = builder.ProcessBuffer(buffer); status != ZX_OK) {
      return status;
    }
  }

  size_t descriptor_size;
  if (Capabilities0::Get().ReadFrom(&*regs_mmio_buffer_).v3_64_bit_system_address_support()) {
    const cpp20::span descriptors{reinterpret_cast<AdmaDescriptor96*>(iobuf_->virt()),
                                  kDmaDescCount};
    descriptor_size = sizeof(descriptors[0]);
    status = builder.BuildDmaDescriptors(descriptors);
  } else {
    const cpp20::span descriptors{reinterpret_cast<AdmaDescriptor64*>(iobuf_->virt()),
                                  kDmaDescCount};
    descriptor_size = sizeof(descriptors[0]);
    status = builder.BuildDmaDescriptors(descriptors);
  }

  if (status != ZX_OK) {
    return status;
  }

  if (auto res = iobuf_->CacheFlush(0, builder.descriptor_count() * descriptor_size);
      res.is_error()) {
    fdf::error("Failed to clean cache: {}", res.status_string());
    return res.error_value();
  }

  AdmaSystemAddress::Get(0).FromValue(Lo32(iobuf_->phys())).WriteTo(&*regs_mmio_buffer_);
  AdmaSystemAddress::Get(1).FromValue(Hi32(iobuf_->phys())).WriteTo(&*regs_mmio_buffer_);
  return ZX_OK;
}

zx_status_t Sdhci::SetUpBuffer(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
                               PendingRequest* const pending_request) {
  if (request.buffers.size() != 1) {
    fdf::error("Only one buffer is supported without DMA");
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto& buffer = request.buffers[0];
  if (buffer.size % request.blocksize != 0) {
    fdf::error("Total buffer size ({}) is not a multiple of the request block size ({})",
               buffer.size, request.blocksize);
    return ZX_ERR_INVALID_ARGS;
  }
  if (request.blocksize % sizeof(uint32_t) != 0) {
    fdf::error("Block size ({}) is not a multiple of {}", request.blocksize, sizeof(uint32_t));
    return ZX_ERR_INVALID_ARGS;
  }

  if (buffer.buffer.is_vmo()) {
    const uint32_t write_perm = (request.cmd_flags & SDMMC_CMD_READ) ? ZX_VM_PERM_WRITE : 0;

    zx::unowned_vmo buffer_vmo(buffer.buffer.vmo());
    zx_status_t status =
        pending_request->vmo_mapper.Map(*buffer_vmo, 0, 0, ZX_VM_PERM_READ | write_perm);
    if (status != ZX_OK) {
      fdf::error("Failed to map request VMO: {}", zx_status_get_string(status));
      return status;
    }

    if (buffer.offset > pending_request->vmo_mapper.size() ||
        buffer.size > (pending_request->vmo_mapper.size() - buffer.offset)) {
      fdf::error("Buffer size and offset out of range of the VMO");
      return ZX_ERR_OUT_OF_RANGE;
    }

    pending_request->data = {
        reinterpret_cast<uint8_t*>(pending_request->vmo_mapper.start()) + buffer.offset,
        buffer.size};
  } else if (buffer.buffer.is_vmo_id()) {
    vmo_store::StoredVmo<OwnedVmoInfo>* vmo =
        registered_vmo_stores_[request.client_id].GetVmo(buffer.buffer.vmo_id());
    if (vmo == nullptr) {
      fdf::error("Unknown VMO ID {} for client {}", buffer.buffer.vmo_id(), request.client_id);
      return ZX_ERR_NOT_FOUND;
    }

    // Size and offset were previously validated by RegisterVmo().
    const cpp20::span<uint8_t> data = vmo->data().subspan(vmo->meta().offset, vmo->meta().size);
    if (buffer.offset > data.size() || buffer.size > (data.size() - buffer.offset)) {
      fdf::error("Buffer size and offset out of range of the VMO");
      return ZX_ERR_OUT_OF_RANGE;
    }
    pending_request->data = data.subspan(buffer.offset, buffer.size);
  } else {
    fdf::error("Unknown buffer type");
    return ZX_ERR_INVALID_ARGS;
  }

  return ZX_OK;
}

zx::result<fidl::Array<uint32_t, 4>> Sdhci::FinishRequest(
    const fuchsia_hardware_sdmmc::wire::SdmmcReq& request, const PendingRequest& pending_request) {
  fidl::Array<uint32_t, 4> out_response;
  constexpr uint32_t kResponseMask = SDMMC_RESP_LEN_136 | SDMMC_RESP_LEN_48 | SDMMC_RESP_LEN_48B;
  if (pending_request.cmd_complete && request.cmd_flags & kResponseMask) {
    memcpy(out_response.data(), pending_request.response, sizeof(uint32_t) * 4);
  }

  if (request.cmd_flags & SDMMC_CMD_TYPE_ABORT) {
    // SDHCI spec section 3.8.2: reset the data line after an abort to discard data in the buffer.
    [[maybe_unused]] auto _ =
        WaitForReset(SoftwareReset::Get().FromValue(0).set_reset_cmd(1).set_reset_dat(1));
  }

  if ((request.cmd_flags & SDMMC_RESP_DATA_PRESENT) && (request.cmd_flags & SDMMC_CMD_READ) &&
      SupportsAdma2()) {
    for (const auto& region : request.buffers) {
      if (region.buffer.is_vmo_id()) {
        continue;
      }

      // Invalidate the cache so that the next CPU read will pick up data that was written to main
      // memory by the controller.
      zx_status_t status = region.buffer.vmo().op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE,
                                                        region.offset, region.size, nullptr, 0);
      if (status != ZX_OK) {
        fdf::error("Failed to clean/invalidate cache: {}", zx_status_get_string(status));
        return zx::error(status);
      }
    }
  }

  const InterruptStatus interrupt_status = pending_request.status;
  if (!interrupt_status.error()) {
    return zx::ok(out_response);
  }

  if (interrupt_status.tuning_error()) {
    fdf::error("Tuning error");
  }
  if (interrupt_status.adma_error()) {
    fdf::error("ADMA error cmd{}", request.cmd_idx);
  }
  if (interrupt_status.auto_cmd_error()) {
    fdf::error("Auto cmd error cmd{}", request.cmd_idx);
  }
  if (interrupt_status.current_limit_error()) {
    fdf::error("Current limit error cmd{}", request.cmd_idx);
  }
  if (interrupt_status.data_end_bit_error()) {
    fdf::error("Data end bit error cmd{}", request.cmd_idx);
  }
  if (interrupt_status.data_crc_error()) {
    if (request.suppress_error_messages) {
      fdf::debug("Data CRC error cmd{}", request.cmd_idx);
    } else {
      fdf::error("Data CRC error cmd{}", request.cmd_idx);
    }
  }
  if (interrupt_status.data_timeout_error()) {
    fdf::error("Data timeout error cmd{} ({} buffers)", request.cmd_idx, request.buffers.size());
  }
  if (interrupt_status.command_index_error()) {
    fdf::error("Command index error cmd{}", request.cmd_idx);
  }
  if (interrupt_status.command_end_bit_error()) {
    fdf::error("Command end bit error cmd{}", request.cmd_idx);
  }
  if (interrupt_status.command_crc_error()) {
    if (request.suppress_error_messages) {
      fdf::debug("Command CRC error cmd{}", request.cmd_idx);
    } else {
      fdf::error("Command CRC error cmd{}", request.cmd_idx);
    }
  }
  if (interrupt_status.command_timeout_error()) {
    if (request.suppress_error_messages) {
      fdf::debug("Command timeout error cmd{}", request.cmd_idx);
    } else {
      fdf::error("Command timeout error cmd{}", request.cmd_idx);
    }
  }
  if (interrupt_status.reg_value() ==
      InterruptStatusEnable::Get().FromValue(0).set_error(1).reg_value()) {
    // Log an unknown error only if no other bits were set.
    fdf::error("Unknown error cmd{}", request.cmd_idx);
  }

  return zx::error(ZX_ERR_IO);
}

void Sdhci::HostInfo(fdf::Arena& arena, HostInfoCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(info_);
}

void Sdhci::SetSignalVoltage(SetSignalVoltageRequestView request, fdf::Arena& arena,
                             SetSignalVoltageCompleter::Sync& completer) {
  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kVendorSetSignalVoltage) {
    auto voltage = fuchsia_hardware_sdhci::wire::DeviceVendorConfigureBusRequest::WithVoltage(
        request->voltage);
    fdf::WireUnownedResult result = sdhci_.buffer(arena_)->VendorConfigureBus(voltage);
    if (!result.ok()) {
      fdf::error("Failed to send VendorConfigureBus request: {}", result.status_string());
      completer.buffer(arena).ReplyError(result.status());
      return;
    }
    if (result->is_error()) {
      fdf::error("Failed to set signal voltage: {}", zx_status_get_string(result->error_value()));
      completer.buffer(arena).Reply(result->take_error());
      return;
    }
    completer.buffer(arena).ReplySuccess();
    return;
  }

  std::lock_guard<std::mutex> lock(mtx_);

  // Validate the controller supports the requested voltage
  if ((request->voltage == fuchsia_hardware_sdmmc::SdmmcVoltage::kV330) &&
      !(info_.caps & fuchsia_hardware_sdmmc::SdmmcHostCap::kVoltage330)) {
    fdf::debug("sdhci: 3.3V signal voltage not supported");
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  auto ctrl2 = HostControl2::Get().ReadFrom(&*regs_mmio_buffer_);
  uint16_t voltage_1v8_value = 0;
  switch (request->voltage) {
    case fuchsia_hardware_sdmmc::SdmmcVoltage::kV180: {
      voltage_1v8_value = 1;
      break;
    }
    case fuchsia_hardware_sdmmc::SdmmcVoltage::kV330: {
      voltage_1v8_value = 0;
      break;
    }
    default:
      fdf::error("sdhci: unknown signal voltage value {}", static_cast<uint32_t>(request->voltage));
      completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      return;
  }

  // Note: the SDHCI spec indicates that the data lines should be checked to see if the card is
  // ready for a voltage switch, however that doesn't seem to work for one of our devices.

  ctrl2.set_voltage_1v8_signalling_enable(voltage_1v8_value).WriteTo(&*regs_mmio_buffer_);

  // Wait 5ms for the regulator to stabilize.
  zx::nanosleep(zx::deadline_after(kVoltageStabilizationTime));

  if (ctrl2.ReadFrom(&*regs_mmio_buffer_).voltage_1v8_signalling_enable() != voltage_1v8_value) {
    fdf::error("sdhci: voltage regulator output did not become stable");
    // Cut power to the card if the voltage switch failed.
    PowerControl::Get()
        .ReadFrom(&*regs_mmio_buffer_)
        .set_sd_bus_power_vdd1(0)
        .WriteTo(&*regs_mmio_buffer_);
    completer.buffer(arena).ReplyError(ZX_ERR_INTERNAL);
    return;
  }

  fdf::debug("sdhci: switch signal voltage to {}", static_cast<uint32_t>(request->voltage));

  completer.buffer(arena).ReplySuccess();
}

void Sdhci::SetBusWidth(SetBusWidthRequestView request, fdf::Arena& arena,
                        SetBusWidthCompleter::Sync& completer) {
  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kVendorSetBusWidth) {
    auto width = fuchsia_hardware_sdhci::wire::DeviceVendorConfigureBusRequest::WithWidth(
        request->bus_width);
    fdf::WireUnownedResult result = sdhci_.buffer(arena_)->VendorConfigureBus(width);
    if (!result.ok()) {
      fdf::error("Failed to send VendorConfigureBus request: {}", result.status_string());
      completer.buffer(arena).ReplyError(result.status());
      return;
    }
    if (result->is_error()) {
      fdf::error("Failed to set bus width: {}", zx_status_get_string(result->error_value()));
      completer.buffer(arena).Reply(result->take_error());
      return;
    }
    completer.buffer(arena).ReplySuccess();
    return;
  }

  std::lock_guard<std::mutex> lock(mtx_);

  if ((request->bus_width == fuchsia_hardware_sdmmc::SdmmcBusWidth::kEight) &&
      !(info_.caps & fuchsia_hardware_sdmmc::SdmmcHostCap::kBusWidth8)) {
    fdf::debug("sdhci: 8-bit bus width not supported");
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  auto ctrl1 = HostControl1::Get().ReadFrom(&*regs_mmio_buffer_);

  switch (request->bus_width) {
    case fuchsia_hardware_sdmmc::SdmmcBusWidth::kOne:
      ctrl1.set_extended_data_transfer_width(0).set_data_transfer_width_4bit(0);
      break;
    case fuchsia_hardware_sdmmc::SdmmcBusWidth::kFour:
      ctrl1.set_extended_data_transfer_width(0).set_data_transfer_width_4bit(1);
      break;
    case fuchsia_hardware_sdmmc::SdmmcBusWidth::kEight:
      ctrl1.set_extended_data_transfer_width(1).set_data_transfer_width_4bit(0);
      break;
    default:
      fdf::error("sdhci: unknown bus width value {}", static_cast<uint32_t>(request->bus_width));
      completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      return;
  }

  ctrl1.WriteTo(&*regs_mmio_buffer_);

  fdf::debug("sdhci: set bus width to {}", static_cast<uint32_t>(request->bus_width));

  completer.buffer(arena).ReplySuccess();
}

void Sdhci::SetBusFreq(SetBusFreqRequestView request, fdf::Arena& arena,
                       SetBusFreqCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mtx_);

  zx_status_t st = WaitForInhibit(
      PresentState::Get().FromValue(0).set_command_inhibit_cmd(1).set_command_inhibit_dat(1));
  if (st != ZX_OK) {
    completer.buffer(arena).ReplyError(st);
    return;
  }

  completer.buffer(arena).Reply(zx::make_result(SetBusClock(request->bus_freq)));
}

zx_status_t Sdhci::SetBusClock(uint32_t frequency_hz) {
  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kVendorSetBusFreq) {
    auto request = fuchsia_hardware_sdhci::wire::DeviceVendorConfigureBusRequest::WithFrequencyHz(
        frequency_hz);
    fdf::WireUnownedResult result = sdhci_.buffer(arena_)->VendorConfigureBus(request);
    if (!result.ok()) {
      fdf::error("Failed to send VendorConfigureBus request: {}", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      fdf::error("Failed to set bus clock: {}", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
    return ZX_OK;
  }

  // Turn off the SD clock before messing with the clock rate.
  auto clock = ClockControl::Get()
                   .ReadFrom(&*regs_mmio_buffer_)
                   .set_sd_clock_enable(0)
                   .WriteTo(&*regs_mmio_buffer_)
                   .set_internal_clock_enable(1);

  if (frequency_hz > 0) {
    // Write the new divider into the control register.
    clock.set_frequency_select(GetClockDividerValue(base_clock_, frequency_hz));
  }

  clock.WriteTo(&*regs_mmio_buffer_);

  if (zx_status_t status = WaitForInternalClockStable(); status != ZX_OK) {
    return status;
  }

  if (frequency_hz > 0) {
    // Turn the SD clock back on.
    clock.set_sd_clock_enable(1).WriteTo(&*regs_mmio_buffer_);
  }

  return ZX_OK;
}

void Sdhci::SetTiming(SetTimingRequestView request, fdf::Arena& arena,
                      SetTimingCompleter::Sync& completer) {
  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kVendorSetTiming) {
    auto timing =
        fuchsia_hardware_sdhci::wire::DeviceVendorConfigureBusRequest::WithTiming(request->timing);
    fdf::WireUnownedResult result = sdhci_.buffer(arena_)->VendorConfigureBus(timing);
    if (!result.ok()) {
      fdf::error("Failed to send VendorConfigureBus request: {}", result.status_string());
      completer.buffer(arena).ReplyError(result.status());
      return;
    }
    if (result->is_error()) {
      fdf::error("Failed to set timing: {}", zx_status_get_string(result->error_value()));
      completer.buffer(arena).Reply(result->take_error());
      return;
    }
    completer.buffer(arena).ReplySuccess();
    return;
  }

  std::lock_guard<std::mutex> lock(mtx_);

  auto ctrl1 = HostControl1::Get().ReadFrom(&*regs_mmio_buffer_);

  // Toggle high-speed
  if (request->timing != fuchsia_hardware_sdmmc::SdmmcTiming::kLegacy) {
    ctrl1.set_high_speed_enable(1).WriteTo(&*regs_mmio_buffer_);
  } else {
    ctrl1.set_high_speed_enable(0).WriteTo(&*regs_mmio_buffer_);
  }

  auto ctrl2 = HostControl2::Get().ReadFrom(&*regs_mmio_buffer_);
  switch (request->timing) {
    case fuchsia_hardware_sdmmc::SdmmcTiming::kLegacy:
    case fuchsia_hardware_sdmmc::SdmmcTiming::kSdr12:
      ctrl2.set_uhs_mode_select(HostControl2::kUhsModeSdr12);
      break;
    case fuchsia_hardware_sdmmc::SdmmcTiming::kHs:
    case fuchsia_hardware_sdmmc::SdmmcTiming::kSdr25:
      ctrl2.set_uhs_mode_select(HostControl2::kUhsModeSdr25);
      break;
    case fuchsia_hardware_sdmmc::SdmmcTiming::kHsddr:
    case fuchsia_hardware_sdmmc::SdmmcTiming::kDdr50:
      ctrl2.set_uhs_mode_select(HostControl2::kUhsModeDdr50);
      break;
    case fuchsia_hardware_sdmmc::SdmmcTiming::kHs200:
    case fuchsia_hardware_sdmmc::SdmmcTiming::kSdr104:
      ctrl2.set_uhs_mode_select(HostControl2::kUhsModeSdr104);
      break;
    case fuchsia_hardware_sdmmc::SdmmcTiming::kHs400:
      ctrl2.set_uhs_mode_select(HostControl2::kUhsModeHs400);
      break;
    case fuchsia_hardware_sdmmc::SdmmcTiming::kSdr50:
      ctrl2.set_uhs_mode_select(HostControl2::kUhsModeSdr50);
      break;
    default:
      fdf::error("sdhci: unknown timing value {}", static_cast<uint32_t>(request->timing));
      completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      return;
  }
  ctrl2.WriteTo(&*regs_mmio_buffer_);

  fdf::debug("sdhci: set bus timing to {}", static_cast<uint32_t>(request->timing));

  completer.buffer(arena).ReplySuccess();
}

void Sdhci::HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mtx_);

  fdf::WireUnownedResult result = sdhci_.buffer(arena_)->HwReset();
  if (!result.ok()) {
    fdf::error("Failed to send HwReset request: {}", result.status_string());
    completer.buffer(arena).ReplyError(result.status());
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

zx_status_t Sdhci::PerformVendorTuning(uint32_t cmd_idx) {
  // Stop waiting on the interrupt, as the controller driver may need to use it for sending tuning
  // commands. Operations on the interrupt handler object must happen on the dispatcher it is
  // waiting on.
  sync_completion_t completion;
  async::PostTask(irq_dispatcher_.async_dispatcher(), [this, &completion]() {
    if (zx_status_t status = irq_handler_.Cancel(); status != ZX_OK) {
      fdf::error("Failed to unbind interrupt: {}", zx_status_get_string(status));
    }
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);

  fdf::Arena arena('SDHC');
  fdf::WireUnownedResult result = sdhci_.buffer(arena)->VendorPerformTuning(cmd_idx);

  // Resume waiting on the interrupt.
  async::PostTask(irq_dispatcher_.async_dispatcher(), [this, &completion]() {
    if (zx_status_t status = irq_handler_.Begin(irq_dispatcher_.async_dispatcher());
        status != ZX_OK) {
      fdf::error("Failed to wait on interrupt: {}", zx_status_get_string(status));
    }
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, zx::duration::infinite().get());

  if (!result.ok()) {
    fdf::error("Failed to send VendorPerformTuning request: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("Failed to perform tuning: {}", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

void Sdhci::PerformTuning(PerformTuningRequestView request, fdf::Arena& arena,
                          PerformTuningCompleter::Sync& completer) {
  fdf::debug("sdhci: perform tuning");

  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kVendorPerformTuning) {
    completer.buffer(arena).Reply(zx::make_result(PerformVendorTuning(request->cmd_idx)));
    return;
  }

  uint16_t blocksize;
  auto ctrl2 = HostControl2::Get().FromValue(0);

  {
    std::lock_guard<std::mutex> lock(mtx_);

    if (cqhci_enabled_) {
      completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
      return;
    }
    blocksize = static_cast<uint16_t>(
        HostControl1::Get().ReadFrom(&*regs_mmio_buffer_).extended_data_transfer_width() ? 128
                                                                                         : 64);
    ctrl2.ReadFrom(&*regs_mmio_buffer_).set_execute_tuning(1).WriteTo(&*regs_mmio_buffer_);
  }

  const fuchsia_hardware_sdmmc::wire::SdmmcReq req = {
      .cmd_idx = request->cmd_idx,
      .cmd_flags = MMC_SEND_TUNING_BLOCK_FLAGS,
      .arg = 0,
      .blocksize = blocksize,
      .suppress_error_messages = true,
      .client_id = 0,
      .buffers = {},
  };
  for (int count = 0; (count < kMaxTuningCount) && ctrl2.execute_tuning(); count++) {
    if (zx::result result = Request(req); result.is_error()) {
      fdf::error("Tuning transfer error: {}", result.status_string());
      completer.buffer(arena).Reply(result.take_error());
      return;
    }

    std::lock_guard<std::mutex> lock(mtx_);
    ctrl2.ReadFrom(&*regs_mmio_buffer_);
  }

  {
    std::lock_guard<std::mutex> lock(mtx_);
    ctrl2.ReadFrom(&*regs_mmio_buffer_);
  }

  const bool fail = ctrl2.execute_tuning() || !ctrl2.use_tuned_clock();

  fdf::debug("sdhci: tuning fail {}", fail);

  completer.buffer(arena).Reply(zx::make_result(fail ? ZX_ERR_IO : ZX_OK));
}

void Sdhci::RegisterInBandInterrupt(RegisterInBandInterruptRequestView request, fdf::Arena& arena,
                                    RegisterInBandInterruptCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mtx_);

  interrupt_cb_ = fdf::WireSharedClient<fuchsia_hardware_sdmmc::InBandInterrupt>(
      std::move(request->interrupt_cb), fdf::Dispatcher::GetCurrent()->get());

  InterruptSignalEnable::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_card_interrupt(1)
      .WriteTo(&*regs_mmio_buffer_);
  InterruptStatusEnable::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_card_interrupt(card_interrupt_masked_ ? 0 : 1)
      .WriteTo(&*regs_mmio_buffer_);

  if (!card_interrupt_masked_) {
    completer.buffer(arena).ReplySuccess();
    return;
  }

  // Call the callback if an interrupt was raised before it was registered.
  interrupt_cb_.buffer(arena)->Callback().Then(
      [this, completer = completer.ToAsync()](
          fdf::WireUnownedResult<fuchsia_hardware_sdmmc::InBandInterrupt::Callback>&
              result) mutable {
        fdf::Arena arena('SDHC');
        if (result.ok()) {
          completer.buffer(arena).ReplySuccess();
        } else {
          completer.buffer(arena).ReplyError(result.status());
          std::lock_guard<std::mutex> lock(mtx_);
          interrupt_cb_ = {};
        }
      });
}

void Sdhci::AckInBandInterrupt(fdf::Arena& arena, AckInBandInterruptCompleter::Sync& completer) {
  std::lock_guard<std::mutex> lock(mtx_);
  InterruptStatusEnable::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_card_interrupt(1)
      .WriteTo(&*regs_mmio_buffer_);
  card_interrupt_masked_ = false;
}

zx_status_t Sdhci::Init() {
  SetBusClock(0);

  // Perform a software reset against both the DAT and CMD interface.
  SoftwareReset::Get().ReadFrom(&*regs_mmio_buffer_).set_reset_all(1).WriteTo(&*regs_mmio_buffer_);

  // Wait for reset to take place. The reset is completed when all three
  // of the following flags are reset.
  const SoftwareReset target_mask =
      SoftwareReset::Get().FromValue(0).set_reset_all(1).set_reset_cmd(1).set_reset_dat(1);
  zx_status_t status = ZX_OK;
  if (status = WaitForReset(target_mask); status != ZX_OK) {
    return status;
  }

  // The core has been reset, which should have stopped any DMAs that were happening when the driver
  // started. It is now safe to release quarantined pages.
  if (status = bti_.release_quarantine(); status != ZX_OK) {
    fdf::error("Failed to release quarantined pages: {}", zx_status_get_string(status));
    return status;
  }

  // Ensure that we're SDv3.
  const uint16_t vrsn =
      HostControllerVersion::Get().ReadFrom(&*regs_mmio_buffer_).specification_version();
  if (vrsn < HostControllerVersion::kSpecificationVersion300) {
    fdf::error("sdhci: SD version is {}, only version {} is supported", vrsn,
               HostControllerVersion::kSpecificationVersion300);
    return ZX_ERR_NOT_SUPPORTED;
  }
  fdf::info("sdhci: controller version {}", vrsn);

  auto caps0 = Capabilities0::Get().ReadFrom(&*regs_mmio_buffer_);
  auto caps1 = Capabilities1::Get().ReadFrom(&*regs_mmio_buffer_);

  base_clock_ = caps0.base_clock_frequency_hz();
  if (base_clock_ == 0) {
    // try to get controller specific base clock
    fdf::WireUnownedResult base_clock = sdhci_.buffer(arena_)->GetBaseClock();
    if (!base_clock.ok()) {
      fdf::error("Failed to send GetBaseClock request: {}", base_clock.status_string());
      return base_clock.status();
    }
    base_clock_ = base_clock->clock;
  }
  if (base_clock_ == 0) {
    fdf::error("sdhci: base clock is 0!");
    return ZX_ERR_INTERNAL;
  }

  const bool non_standard_tuning =
      static_cast<bool>(quirks_ & fuchsia_hardware_sdhci::Quirk::kNonStandardTuning);
  const bool tuning_for_sdr50 = caps1.use_tuning_for_sdr50();

  // Get controller capabilities
  if (caps0.bus_width_8_support()) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kBusWidth8;
  }
  if (caps0.adma2_support() && !(quirks_ & fuchsia_hardware_sdhci::Quirk::kNoDma)) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kDma;
  }
  if (caps0.voltage_3v3_support()) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kVoltage330;
  }
  if (caps1.sdr50_support() && (!non_standard_tuning || !tuning_for_sdr50)) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kSdr50;
  }
  if (caps1.ddr50_support() && !(quirks_ & fuchsia_hardware_sdhci::Quirk::kNoDdr)) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kDdr50;
  }
  if (caps1.sdr104_support() && !non_standard_tuning) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kSdr104;
  }
  if (!tuning_for_sdr50) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kNoTuningSdr50;
  }
  if (!(quirks_ & fuchsia_hardware_sdhci::Quirk::kNoHs400EnhancedStrobe)) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kHs400EnhancedStrobe;
  }
  info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kAutoCmd12;
  if (regs_cqhci_mmio_buffer_) {
    info_.caps |= fuchsia_hardware_sdmmc::SdmmcHostCap::kCommandQueueing;
  }

  // allocate and setup DMA descriptor
  if (SupportsAdma2()) {
    auto buffer_factory = dma_buffer::CreateBufferFactory();
    auto host_control1 = HostControl1::Get().ReadFrom(&*regs_mmio_buffer_);
    if (caps0.v3_64_bit_system_address_support()) {
      const size_t buffer_size =
          fbl::round_up(kDmaDescCount * sizeof(AdmaDescriptor96), zx_system_get_page_size());
      status = buffer_factory->CreateContiguous(bti_, buffer_size, 0,
                                                dma_buffer::CacheOptions::kEnabled, &iobuf_);
      host_control1.set_dma_select(HostControl1::kDmaSelect64BitAdma2);
    } else {
      const size_t buffer_size =
          fbl::round_up(kDmaDescCount * sizeof(AdmaDescriptor64), zx_system_get_page_size());
      status = buffer_factory->CreateContiguous(bti_, buffer_size, 0,
                                                dma_buffer::CacheOptions::kEnabled, &iobuf_);
      host_control1.set_dma_select(HostControl1::kDmaSelect32BitAdma2);

      if ((iobuf_->phys() & k32BitPhysAddrMask) != iobuf_->phys()) {
        fdf::error("Got 64-bit physical address, only 32-bit DMA is supported");
        return ZX_ERR_NOT_SUPPORTED;
      }
    }

    if (status != ZX_OK) {
      fdf::error("sdhci: error allocating DMA descriptors");
      return status;
    }
    info_.max_transfer_size = kDmaDescCount * zx_system_get_page_size();

    host_control1.WriteTo(&*regs_mmio_buffer_);
  } else {
    // Assumes typical block size of 512 bytes, though in theory the request could specify a
    // different value.
    constexpr size_t kBlockSize = 512;
    info_.max_transfer_size = std::numeric_limits<BlockCountType>::max() * kBlockSize;
  }

  // Set the command timeout.
  TimeoutControl::Get()
      .ReadFrom(&*regs_mmio_buffer_)
      .set_data_timeout_counter(TimeoutControl::kDataTimeoutMax)
      .WriteTo(&*regs_mmio_buffer_);

  // Set SD bus voltage to maximum supported by the host controller
  auto power = PowerControl::Get().ReadFrom(&*regs_mmio_buffer_).set_sd_bus_power_vdd1(1);
  if (info_.caps & fuchsia_hardware_sdmmc::SdmmcHostCap::kVoltage330) {
    power.set_sd_bus_voltage_vdd1(PowerControl::kBusVoltage3V3);
  } else {
    power.set_sd_bus_voltage_vdd1(PowerControl::kBusVoltage1V8);
  }
  power.WriteTo(&*regs_mmio_buffer_);

  // Enable the SD clock.
  status = SetBusClock(kSdFreqSetupHz);
  if (status != ZX_OK) {
    return status;
  }

  // Disable all interrupts
  {
    std::lock_guard<std::mutex> lock(mtx_);
    DisableInterrupts();
  }

  if (zx::result irq_dispatcher = fdf::SynchronizedDispatcher::Create(
          {}, "sdhci_irq_thread", [](fdf_dispatcher_t*) {}, kIrqProfileName);
      irq_dispatcher.is_ok()) {
    irq_dispatcher_ = *std::move(irq_dispatcher);
  } else {
    fdf::error("Failed to create interrupt dispatcher: {}", irq_dispatcher.status_string());
    return irq_dispatcher.error_value();
  }

  async::PostTask(irq_dispatcher_.async_dispatcher(), [this, irq = irq_.get()]() {
    irq_handler_.set_object(irq);
    if (zx_status_t status = irq_handler_.Begin(irq_dispatcher_.async_dispatcher());
        status != ZX_OK) {
      fdf::error("Failed to wait on interrupt: {}", zx_status_get_string(status));
    }
  });

  // Set controller preferences
  fuchsia_hardware_sdmmc::SdmmcHostPrefs default_speed_capabilities{};
  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kNonStandardTuning) {
    // Disable HS200 and HS400 if tuning cannot be performed as per the spec.
    default_speed_capabilities |= fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs200 |
                                  fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400;
  }
  if (quirks_ & fuchsia_hardware_sdhci::Quirk::kNoDdr) {
    default_speed_capabilities |= fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHsddr |
                                  fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400;
  }

  fuchsia_hardware_sdmmc::SdmmcMetadata metadata{{
      .speed_capabilities = default_speed_capabilities,
      .use_fidl = true,
  }};

  zx::result existing_metadata =
      fdf_metadata::GetMetadataIfExists<fuchsia_hardware_sdmmc::SdmmcMetadata>(*incoming_);
  if (existing_metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", existing_metadata.status_string());
    return existing_metadata.status_value();
  }

  if (existing_metadata.value().has_value()) {
    metadata.max_frequency() = existing_metadata->max_frequency();
    metadata.enable_cache() = existing_metadata->enable_cache();
    metadata.removable() = existing_metadata->removable();
    metadata.max_command_packing() = existing_metadata->max_command_packing();
    metadata.vccq_off_with_controller_off() = existing_metadata->vccq_off_with_controller_off();

    const auto& speed_capabilities = existing_metadata->speed_capabilities();
    if (speed_capabilities.has_value()) {
      // OR the speed capabilities reported by the parent with the ones reported by the host
      // controller, which limits us to speed modes supported by both.
      metadata.speed_capabilities() = speed_capabilities.value() | default_speed_capabilities;
    }
  }

  if (!SupportsAdma2()) {
    // Non-DMA requests are only allowed to use a single buffer, so tell the core driver to disable
    // command packing. This limitation could be removed in the future.
    metadata.max_command_packing() = 0;
  }

  if (supports_inline_crypto_) {
    fdf::info("enabling cryptographic operation support");
    CommandQueuingConfiguration::Get()
        .ReadFrom(&*regs_cqhci_mmio_buffer_)
        .set_crypto_enable(true)
        .WriteTo(&*regs_cqhci_mmio_buffer_);
  }

  if (zx::result result = metadata_server_.Serve(*outgoing(), dispatcher(), metadata);
      result.is_error()) {
    fdf::error("Failed to serve metadata server: {}", result.status_string());
    return result.status_value();
  }

  return ZX_OK;
}

zx_status_t Sdhci::InitMmio() {
  // Map the Device Registers so that we can perform MMIO against the device.
  zx::vmo vmo;
  zx_off_t vmo_offset;
  {
    fdf::WireUnownedResult mmio = sdhci_.buffer(arena_)->GetSdhciMmio();
    if (!mmio.ok()) {
      fdf::error("Failed to send GetSdhciMmio request: {}", mmio.status_string());
      return mmio.status();
    }
    if (mmio->is_error()) {
      fdf::error("Failed to get sdhci mmio: {}", zx_status_get_string(mmio->error_value()));
      return mmio->error_value();
    }
    vmo = std::move(mmio.value()->mmio);
    vmo_offset = mmio.value()->offset;
  }

  zx::result<fdf::MmioBuffer> regs_mmio_buffer = fdf::MmioBuffer::Create(
      vmo_offset, kRegisterSetSize, std::move(vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (regs_mmio_buffer.is_error()) {
    fdf::error("sdhci: error {} in mmio_buffer_init", regs_mmio_buffer);
    return regs_mmio_buffer.status_value();
  }
  regs_mmio_buffer_ = *std::move(regs_mmio_buffer);

  return ZX_OK;
}

zx_status_t Sdhci::InitCqhciMmio() {
  // Map the Device Command Queuing Registers so that we can support command queuing and related
  // cryptographic operations.
  zx::vmo vmo;
  zx_off_t vmo_offset;
  {
    fdf::WireUnownedResult mmio = sdhci_.buffer(arena_)->GetCqhciMmio();
    if (!mmio.ok()) {
      fdf::error("Failed to send GetCqhciMmio request: {}", mmio.status_string());
      return mmio.status();
    }
    if (mmio->is_error()) {
      if (mmio->error_value() != ZX_ERR_NOT_SUPPORTED) {
        fdf::error("Failed to get mmio in InitCqhciMmio: {}",
                   zx_status_get_string(mmio->error_value()));
        return mmio->error_value();
      } else {
        return ZX_OK;
      }
    }
    vmo = std::move(mmio.value()->mmio);
    vmo_offset = mmio.value()->offset;
  }

  size_t vmo_size;
  if (zx_status_t status = vmo.get_size(&vmo_size); status != ZX_OK) {
    fdf::error("error querying mmio vmo size: {}", zx_status_get_string(status));
    return status;
  }
  if (vmo_size < vmo_offset) {
    fdf::error("cqhci mmio size ({}) is smaller than offset ({})!", vmo_size, vmo_offset);
    return ZX_ERR_IO_INVALID;
  }
  const size_t cqhci_mmio_size = vmo_size - vmo_offset;
  if (cqhci_mmio_size < kMinimumCqhciMmioSize) {
    fdf::error("cqhci mmio size ({}) is smaller than expected minimum ({})!", cqhci_mmio_size,
               kMinimumCqhciMmioSize);
    return ZX_ERR_IO_INVALID;
  }

  zx::result<fdf::MmioBuffer> regs_mmio_buffer = fdf::MmioBuffer::Create(
      vmo_offset, cqhci_mmio_size, std::move(vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (regs_mmio_buffer.is_error()) {
    fdf::error("error {} in mmio_buffer_init", regs_mmio_buffer);
    return regs_mmio_buffer.status_value();
  }
  regs_cqhci_mmio_buffer_ = *std::move(regs_mmio_buffer);
  supports_inline_crypto_ =
      CommandQueuingCapabilities::Get().ReadFrom(&*regs_cqhci_mmio_buffer_).crypto_support();

  return ZX_OK;
}

zx::result<> Sdhci::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  {
    zx::result sdhci = incoming_->Connect<fuchsia_hardware_sdhci::Service::Device>();
    if (sdhci.is_error()) {
      fdf::error("Failed to connect to sdhci: {}", sdhci);
      return sdhci.take_error();
    }
    sdhci_.Bind(std::move(sdhci.value()));
  }

  zx_status_t status = InitMmio();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  status = InitCqhciMmio();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  {
    fdf::WireUnownedResult bti = sdhci_.buffer(arena_)->GetBti(0);
    if (!bti.ok()) {
      fdf::error("Failed to send GetBti request: {}", bti.status_string());
      return zx::error(bti.status());
    }
    if (bti->is_error()) {
      fdf::error("Failed to get bti: {}", zx_status_get_string(bti->error_value()));
      return zx::error(bti->error_value());
    }
    bti_ = std::move(bti.value()->bti);
  }

  {
    fdf::WireUnownedResult irq = sdhci_.buffer(arena_)->GetInterrupt();
    if (!irq.ok()) {
      fdf::error("Failed to send GetInterrupt request: {}", irq.status_string());
      return zx::error(irq.status());
    }
    if (irq->is_error()) {
      fdf::error("Failed to get interrupt: {}", zx_status_get_string(irq->error_value()));
      return zx::error(irq->error_value());
    }
    irq_ = std::move(irq.value()->irq);
  }

  dma_boundary_alignment_ = 0;

  {
    fdf::WireUnownedResult quirks = sdhci_.buffer(arena_)->GetQuirks();
    if (!quirks.ok()) {
      fdf::error("Failed to send GetQuirks request: {}", quirks.status_string());
      return zx::error(quirks.status());
    }
    quirks_ = quirks.value().quirks;
    dma_boundary_alignment_ = quirks.value().dma_boundary_alignment;
  }

  if (!(quirks_ & fuchsia_hardware_sdhci::Quirk::kUseDmaBoundaryAlignment)) {
    dma_boundary_alignment_ = 0;
  } else if (dma_boundary_alignment_ == 0) {
    fdf::error("sdhci: DMA boundary alignment is zero");
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  // initialize the controller
  status = Init();
  if (status != ZX_OK) {
    fdf::error("{}: SDHCI Controller init failed", __func__);
    return zx::error(status);
  }

  fuchsia_hardware_sdmmc::SdmmcService::InstanceHandler handler({
      .sdmmc = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                       fidl::kIgnoreBindingClosure),
      .inline_crypto =
          [this](fdf::ServerEnd<fuchsia_hardware_sdhci::Service::InlineCrypto::ProtocolType>
                     server_end) {
            zx::result result = incoming_->Connect<fuchsia_hardware_sdhci::Service::InlineCrypto>(
                std::move(server_end));
            if (result.is_error()) {
              fdf::warn("Failed to connect to inline encryption service: {}", result);
            }
          },
  });

  if (zx::result<> result =
          outgoing()->AddService<fuchsia_hardware_sdmmc::SdmmcService>(std::move(handler));
      result.is_error()) {
    fdf::error("Failed to add service: {}", result);
    return result;
  }

  std::vector<fuchsia_driver_framework::Offer> offers;
  std::optional metadata_offer = metadata_server_.CreateOffer();
  if (metadata_offer.has_value()) {
    offers.push_back(std::move(metadata_offer.value()));
  }
  offers.push_back(fdf::MakeOffer2<fuchsia_hardware_sdmmc::SdmmcService>());

  // The SDHCI core driver does not have to take any action when the SDMMC device or controller
  // power elements change state. Therefore we can simply forward PowerTokenService from our parent
  // to our child so that we are not in the loop for state changes.
  if (context.take_config<sdhci_config::Config>().enable_suspend()) {
    fuchsia_hardware_power::PowerTokenService::InstanceHandler handler({
        .token_provider =
            [this](fidl::ServerEnd<fuchsia_hardware_power::PowerTokenProvider> server) {
              zx::result<> result =
                  incoming_->Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
                      std::move(server));
              if (result.is_error()) {
                fdf::warn("Failed to connect to power token service: {}", result);
              }
            },
    });

    zx::result result =
        outgoing()->AddService<fuchsia_hardware_power::PowerTokenService>(std::move(handler));
    if (result.is_error()) {
      fdf::error("Failed to add power token service: {}", result);
      return result.take_error();
    }

    offers.emplace_back(fdf::MakeOffer2<fuchsia_hardware_power::PowerTokenService>());
  }

  const auto kChildNodeName = name();
  zx::result result =
      AddChild(kChildNodeName, std::vector<fuchsia_driver_framework::NodeProperty>{}, offers);
  if (result.is_error()) {
    fdf::error("Failed to add child: {}", result);
    return result.take_error();
  }
  node_controller_.Bind(std::move(result.value()));

  return zx::ok();
}

void Sdhci::Stop(fdf::StopCompleter completer) {
  std::lock_guard<std::mutex> lock(mtx_);
  shutdown_ = true;
  if (pending_request_) {
    // Wait for the in-flight request to finish before completing PrepareStop().
    stop_completer_.emplace(std::move(completer));
  } else {
    completer(zx::ok());
  }
}

}  // namespace sdhci
