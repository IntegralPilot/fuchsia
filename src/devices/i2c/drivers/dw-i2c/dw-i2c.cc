// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dw-i2c.h"

#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_offers.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/sync/completion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/assert.h>

#include <array>
#include <memory>
#include <mutex>

namespace dw_i2c {

void DwI2c::Dumpstate() {
  fdf::info("DW_kI2cEnable_STATUS = \t0x{:x}",
            EnableStatusReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_kI2cEnable = \t0x{:x}", EnableReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_CON = \t0x{:x}", ControlReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_TAR = \t0x{:x}", TargetAddressReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_HS_MADDR = \t0x{:x}",
            HSMasterAddrReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_SS_SCL_HCNT = \t0x{:x}",
            StandardSpeedSclHighCountReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_SS_SCL_LCNT = \t0x{:x}",
            StandardSpeedSclLowCountReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_FS_SCL_HCNT = \t0x{:x}",
            FastSpeedSclHighCountReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_FS_SCL_LCNT = \t0x{:x}",
            FastSpeedSclLowCountReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_INTR_MASK = \t0x{:x}",
            InterruptMaskReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_RAW_INTR_STAT = \t0x{:x}",
            RawInterruptStatusReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_RX_TL = \t0x{:x}",
            RxFifoThresholdReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_TX_TL = \t0x{:x}",
            TxFifoThresholdReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_STATUS = \t0x{:x}", StatusReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_TXFLR = \t0x{:x}", TxFifoLevelReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_RXFLR = \t0x{:x}", RxFifoLevelReg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_COMP_PARAM_1 = \t0x{:x}",
            CompParam1Reg::Get().ReadFrom(&mmio_.value()).reg_value());
  fdf::info("DW_I2C_TX_ABRT_SOURCE = \t0x{:x}",
            TxAbrtSourceReg::Get().ReadFrom(&mmio_.value()).reg_value());
}

zx::result<> DwI2c::EnableAndWait(bool enable) {
  uint32_t poll = 0;

  // Set enable bit.
  auto enable_reg = EnableReg::Get().ReadFrom(&mmio_.value());
  enable_reg.set_enable(enable).WriteTo(&mmio_.value());

  do {
    if (EnableStatusReg::Get().ReadFrom(&mmio_.value()).enable() == enable) {
      // We are done. Exit.
      return zx::ok();
    }
    // Sleep 10 times the signaling period for the highest i2c transfer speed (400K) ~25uS.
    zx_nanosleep(zx_deadline_after(kPollSleep));
  } while (poll++ < kMaxPoll);

  fdf::error("{}: Could not {} I2C contoller! DW_kI2cEnable_STATUS = 0x{:x}", __FUNCTION__,
             enable ? "enable" : "disable",
             EnableStatusReg::Get().ReadFrom(&mmio_.value()).enable());
  Dumpstate();

  return zx::error(ZX_ERR_TIMED_OUT);
}

zx::result<> DwI2c::Enable() { return EnableAndWait(kI2cEnable); }

void DwI2c::ClearInterrupts() {
  // Reading this register will clear all the interrupts.
  ClearInterruptReg::Get().ReadFrom(&mmio_.value());
}

void DwI2c::DisableInterrupts() { InterruptMaskReg::Get().FromValue(0).WriteTo(&mmio_.value()); }

void DwI2c::EnableInterrupts(uint32_t flag) {
  InterruptMaskReg::Get().FromValue(flag).WriteTo(&mmio_.value());
}

zx::result<> DwI2c::Disable() { return EnableAndWait(kI2cDisable); }

InterruptStatusReg DwI2c::ReadAndClearIrq() {
  auto irq = InterruptStatusReg::Get().ReadFrom(&mmio_.value());

  if (irq.tx_abrt()) {
    // The device did not respond with an ACK to its address. This is expected for some devices, so
    // don't log an error message.
    fdf::error("dw-i2c: error on bus - Abort source 0x{:x}",
               TxAbrtSourceReg::Get().ReadFrom(&mmio_.value()).reg_value());
    // ABRT_SOURCE should be read before clearing TX_ABRT.
    ClearTxAbrtReg::Get().ReadFrom(&mmio_.value());
  }
  if (irq.start_det()) {
    ClearStartDetReg::Get().ReadFrom(&mmio_.value());
  }
  if (irq.activity()) {
    ClearActivityReg::Get().ReadFrom(&mmio_.value());
  }
  if (irq.stop_det()) {
    ClearStopDetReg::Get().ReadFrom(&mmio_.value());
  }
  return irq;
}

zx::result<> DwI2c::WaitBusBusy() {
  uint32_t timeout = 0;
  auto status = StatusReg::Get();
  while (status.ReadFrom(&mmio_.value()).activity()) {
    if (timeout > 100) {
      return zx::error(ZX_ERR_TIMED_OUT);
    }
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    timeout++;
  }
  return zx::ok();
}

void DwI2c::SetOpsHelper(std::vector<Op> ops) {
  ops_ = std::move(ops);
  rx_op_idx_ = 0;
  tx_op_idx_ = 0;
  rx_done_len_ = 0;
  tx_done_len_ = 0;
  send_restart_ = false;
  rx_pending_ = 0;
}

void DwI2c::Transact(TransactRequestView request, fdf::Arena& arena,
                     TransactCompleter::Sync& completer) {
  size_t count = request->op.size();
  if (count == 0) {
    completer.buffer(arena).ReplySuccess({});
    return;
  }

  for (const auto& op : request->op) {
    if (op.type.is_read_size() && op.type.read_size() > kMaxTransfer) {
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    if (op.type.is_write_data() && op.type.write_data().size() > kMaxTransfer) {
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
  }

  for (size_t i = 1; i < count; ++i) {
    if (request->op[i].address != request->op[0].address) {
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
      return;
    }
  }

  if (auto result = WaitBusBusy(); result.is_error()) {
    fdf::error("I2C bus wait failed {}", result.status_value());
    completer.buffer(arena).ReplyError(result.status_value());
    return;
  }

  if (auto result = SetSlaveAddress(request->op[0].address); result.is_error()) {
    fdf::error("I2C set address failed {}", result.status_value());
    completer.buffer(arena).ReplyError(result.status_value());
    return;
  }

  // Count reads.
  size_t read_count = 0;
  for (const auto& op : request->op) {
    if (op.type.is_read_size()) {
      read_count++;
    }
  }

  std::vector<Op> ops;
  ops.reserve(request->op.size());
  std::vector<fuchsia_hardware_i2cimpl::wire::ReadData> reads;
  reads.reserve(read_count);

  for (const auto& op : request->op) {
    if (op.type.is_read_size()) {
      fidl::VectorView<uint8_t> dst(arena, op.type.read_size());
      reads.push_back({dst});
      ops.push_back(ReadOp{
          .data = std::span<uint8_t>(dst.data(), dst.size()),
          .stop = op.stop,
      });
    } else {
      ops.push_back(WriteOp{
          .data =
              std::span<const uint8_t>(op.type.write_data().data(), op.type.write_data().size()),
          .stop = op.stop,
      });
    }
  }

  DisableInterrupts();
  SetOpsHelper(std::move(ops));
  if (auto result = Enable(); result.is_error()) {
    fdf::error("I2C device enable failed {}", result.status_value());
    completer.buffer(arena).ReplyError(result.status_value());
    return;
  }

  ClearInterrupts();
  EnableInterrupts(kI2cInterruptDefaultMask);

  bool done = false;
  zx_status_t error_status = ZX_OK;
  while (!done) {
    // Poll instead of wait.
    for (uint32_t timeout = 0;; timeout++) {
      // Poll the masked interrupt status register (InterruptStatusReg) instead of the raw one.
      // After all TX operations are queued, the TX_EMPTY interrupt mask is disabled in the
      // InterruptMaskReg. If we poll RawInterruptStatusReg, the TX_EMPTY raw bit will remain
      // set (because the TX FIFO is empty), causing the polling loop to exit immediately without
      // sleeping, resulting in a CPU-spinning infinite loop. Polling the masked status register
      // ensures we only exit when an unmasked (active) interrupt event occurs.
      auto reg = InterruptStatusReg::Get().ReadFrom(&mmio_.value());
      if (reg.reg_value()) {
        break;
      }
      if (timeout > 1000) {  // 1 second timeout
        fdf::error("I2C polling timeout! Raw IRQ: 0x{:x}",
                   RawInterruptStatusReg::Get().ReadFrom(&mmio_.value()).reg_value());
        error_status = ZX_ERR_TIMED_OUT;
        done = true;
        break;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
    if (done) {
      break;
    }

    auto reg = ReadAndClearIrq();

    if (reg.tx_abrt()) {
      fdf::error("I2C transaction aborted");
      error_status = ZX_ERR_IO;
      done = true;
    }

    if (reg.rx_full()) {
      if (auto res = Receive(); res.is_error()) {
        error_status = res.status_value();
        done = true;
      }
    }

    if (reg.tx_empty()) {
      if (auto res = Transmit(); res.is_error()) {
        error_status = res.status_value();
        done = true;
      }
    }

    if (reg.stop_det()) {
      if (tx_op_idx_ == ops_.size() && rx_pending_ == 0) {
        done = true;
      }
    }
  }

  if (auto result = Disable(); result.is_error()) {
    fdf::error("I2C device disable failed {}", result.status_value());
  }

  if (error_status != ZX_OK) {
    completer.buffer(arena).ReplyError(error_status);
    return;
  }

  if (reads.empty()) {
    completer.buffer(arena).ReplySuccess({});
  } else {
    completer.buffer(arena).ReplySuccess({arena, reads});
  }
}

zx::result<> DwI2c::SetSlaveAddress(uint16_t addr) {
  if (addr & (~k7BitAddrMask)) {
    // support 7bit for now
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  addr &= k7BitAddrMask;
  auto reg = TargetAddressReg::Get().ReadFrom(&mmio_.value());
  reg.set_target_address(addr).set_master_10bitaddr(0);
  reg.WriteTo(&mmio_.value());
  return zx::ok();
}

zx::result<> DwI2c::Receive() {
  if (rx_pending_ == 0) {
    fdf::error("dw-i2c: Bytes received without being requested");
    return zx::error(ZX_ERR_IO_OVERRUN);
  }

  uint32_t avail_read = RxFifoLevelReg::Get().ReadFrom(&mmio_.value()).rx_fifo_level();

  while ((avail_read != 0) && (rx_op_idx_ < ops_.size())) {
    auto& op = ops_[rx_op_idx_];
    if (std::holds_alternative<WriteOp>(op)) {
      rx_op_idx_++;
      continue;
    }
    auto& read_op = std::get<ReadOp>(op);
    read_op.data[rx_done_len_] =
        static_cast<uint8_t>(DataCommandReg::Get().ReadFrom(&mmio_.value()).data());
    rx_done_len_++;
    rx_pending_--;
    if (rx_done_len_ == read_op.data.size()) {
      rx_op_idx_++;
      rx_done_len_ = 0;
    }
    avail_read--;
  }

  if (avail_read != 0) {
    fdf::error("dw-i2c: {} more bytes received than requested", avail_read);
    return zx::error(ZX_ERR_IO_OVERRUN);
  }

  return zx::ok();
}

zx::result<> DwI2c::Transmit() {
  uint32_t tx_limit;

  tx_limit = tx_fifo_depth_ - TxFifoLevelReg::Get().ReadFrom(&mmio_.value()).tx_fifo_level();

  // TODO(https://fxbug.dev/34403)
  // If IC_EMPTYFIFO_HOLD_MASTER_EN = 0, then STOP is sent on TX_EMPTY. All commands should be
  // queued up as soon as possible to avoid this. Possible race leading to failed
  // transaction, if the irq thread is deschedule in the midst for tx command queuing.
  // This is the mode used in as370 and currently this issue is not addressed.
  // See bug https://fxbug.dev/34403 for details.
  // If IC_EMPTYFIFO_HOLD_MASTER_EN = 1, then STOP and RESTART must be sent explicitly, which is
  // handled by this code.
  while ((tx_limit != 0) && (tx_op_idx_ < ops_.size())) {
    auto& op = ops_[tx_op_idx_];
    auto cmd = DataCommandReg::Get().FromValue(0);
    bool stop = false;
    size_t len = 0;
    if (std::holds_alternative<ReadOp>(op)) {
      auto& read_op = std::get<ReadOp>(op);
      len = read_op.data.size() - tx_done_len_;
      stop = read_op.stop;
    } else {
      auto& write_op = std::get<WriteOp>(op);
      len = write_op.data.size() - tx_done_len_;
      stop = write_op.stop;
    }

    if (len == 1 && stop) {
      cmd.set_stop(1);
    }
    if (send_restart_) {
      cmd.set_start(1);
      send_restart_ = false;
    }

    if (std::holds_alternative<ReadOp>(op)) {
      auto& read_op = std::get<ReadOp>(op);
      cmd.set_command(1);
      rx_pending_++;
      if (tx_done_len_ == 0) {
        RxFifoThresholdReg::Get()
            .FromValue(0)
            .set_rx_threshold_level(static_cast<uint32_t>(read_op.data.size() - 1))
            .WriteTo(&mmio_.value());
      }
      cmd.WriteTo(&mmio_.value());
      tx_done_len_++;
      if (tx_done_len_ == read_op.data.size()) {
        tx_op_idx_++;
        tx_done_len_ = 0;
        send_restart_ = true;
      }
    } else {
      auto& write_op = std::get<WriteOp>(op);
      cmd.set_data(write_op.data[tx_done_len_]);
      cmd.WriteTo(&mmio_.value());
      tx_done_len_++;
      if (tx_done_len_ == write_op.data.size()) {
        tx_op_idx_++;
        tx_done_len_ = 0;
        send_restart_ = true;
      }
    }
    tx_limit--;
  }

  if (tx_op_idx_ == ops_.size()) {
    // All tx are complete. Remove TX_EMPTY from interrupt mask.
    EnableInterrupts(kI2cInterruptReadMask);
  }

  return zx::ok();
}

zx::result<> DwI2c::HostInit() {
  // Make sure we are truly running on a DesignWire IP.
  auto dw_comp_type = CompTypeReg::Get().ReadFrom(&mmio_.value()).reg_value();

  if (dw_comp_type != kDwCompTypeNum) {
    fdf::error("{}: Incompatible IP Block detected. Expected = 0x{:x}, Actual = 0x{:x}",
               __FUNCTION__, kDwCompTypeNum, dw_comp_type);

    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Read the various capabilities of the fragment.
  auto comp_reg = CompParam1Reg::Get().ReadFrom(&mmio_.value());
  tx_fifo_depth_ = comp_reg.tx_buffer_depth();
  rx_fifo_depth_ = comp_reg.rx_buffer_depth();

  // Minimum fifo depth would be max transfer limit.
  kMaxTransfer = tx_fifo_depth_ > rx_fifo_depth_ ? rx_fifo_depth_ : tx_fifo_depth_;

  /* I2C Block Initialization based on DW_apb_i2c_databook Section 7.3 */

  // Disable I2C Block.
  if (auto status = Disable(); status.is_error()) {
    return status;
  }

  // Configure the controller:
  auto ctrl_reg = ControlReg::Get().FromValue(0);

  // - slave disable
  ctrl_reg.set_slave_disable(1);

  // - enable restart mode
  ctrl_reg.set_restart_en(1);

  // - set 7-bit address modeset
  ctrl_reg.set_master_10bitaddr(k7BitAddr);
  ctrl_reg.set_slave_10bitaddr(k7BitAddr);

  // - set speed to fast, master enable
  ctrl_reg.set_max_speed_mode(kFastMode);

  // - set master enable
  ctrl_reg.set_master_mode(1);

  // Write final mask.
  ctrl_reg.WriteTo(&mmio_.value());

  // Write SS/FS LCNT and HCNT.
  StandardSpeedSclHighCountReg::Get()
      .ReadFrom(&mmio_.value())
      .set_ss_scl_hcnt(kSclStandardSpeedHcnt)
      .WriteTo(&mmio_.value());
  StandardSpeedSclLowCountReg::Get()
      .ReadFrom(&mmio_.value())
      .set_ss_scl_lcnt(kSclStandardSpeedLcnt)
      .WriteTo(&mmio_.value());
  FastSpeedSclHighCountReg::Get()
      .ReadFrom(&mmio_.value())
      .set_fs_scl_hcnt(kSclFastSpeedHcnt)
      .WriteTo(&mmio_.value());
  FastSpeedSclLowCountReg::Get()
      .ReadFrom(&mmio_.value())
      .set_fs_scl_lcnt(kSclFastSpeedLcnt)
      .WriteTo(&mmio_.value());

  // Set SDA Hold time.
  // Enable SDA hold for RX as well.
  SdaHoldReg::Get()
      .FromValue(0)
      .set_sda_hold_time_tx(kSdaHoldValue)
      .set_sda_hold_time_rx(kSdaHoldValue)
      .WriteTo(&mmio_.value());

  // Setup TX and RX FIFO Thresholds.
  TxFifoThresholdReg::Get()
      .ReadFrom(&mmio_.value())
      .set_tx_threshold_level(tx_fifo_depth_ / 2)
      .WriteTo(&mmio_.value());
  RxFifoThresholdReg::Get()
      .ReadFrom(&mmio_.value())
      .set_rx_threshold_level(0)
      .WriteTo(&mmio_.value());

  // Disable interrupts.
  DisableInterrupts();

  return zx::ok();
}

void DwI2c::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_i2cimpl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unknown method {}", metadata.method_ordinal);
}

void DwI2c::GetMaxTransferSize(fdf::Arena& arena, GetMaxTransferSizeCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(kMaxTransfer);
}

void DwI2c::SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                       SetBitrateCompleter::Sync& completer) {
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

zx::result<fdf::MmioBuffer> DwI2c::MapMmio(fdf::PDev& pdev) { return pdev.MapMmio(0); }

zx::result<> DwI2c::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result pdev_client_end =
      incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to pdev protocol: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }

  fdf::PDev pdev{std::move(pdev_client_end.value())};

  // Connect and enable power domain.
  {
    zx::result domain_client =
        incoming->Connect<fuchsia_hardware_powerdomain::Service::Domain>("power-domain");
    if (domain_client.is_error()) {
      fdf::error("Failed to connect to power-domain: {}", domain_client.status_string());
      return domain_client.take_error();
    }
    powerdomain_ = fidl::WireSyncClient(std::move(domain_client.value()));
    auto result = powerdomain_->Enable();
    if (!result.ok() || result->is_error()) {
      fdf::error(
          "Failed to enable power domain: {}",
          result.ok() ? zx_status_get_string(result->error_value()) : result.status_string());
      return zx::error(result.ok() ? result->error_value() : result.status());
    }
  }

  // Connect and enable clocks.
  {
    zx::result clock_bus_client = incoming->Connect<fuchsia_hardware_clock::Service::Clock>("bus");
    if (clock_bus_client.is_error()) {
      fdf::error("Failed to connect to bus clock: {}", clock_bus_client.status_string());
      return clock_bus_client.take_error();
    }
    clock_bus_ = fidl::WireSyncClient(std::move(clock_bus_client.value()));
    auto result = clock_bus_->Enable();
    if (!result.ok() || result->is_error()) {
      fdf::error("Failed to enable clock-bus: {}", result.ok()
                                                       ? zx_status_get_string(result->error_value())
                                                       : result.status_string());
      return zx::error(result.ok() ? result->error_value() : result.status());
    }
  }

  {
    zx::result clock_regs_client =
        incoming->Connect<fuchsia_hardware_clock::Service::Clock>("registers");
    if (clock_regs_client.is_error()) {
      fdf::error("Failed to connect to registers clock: {}", clock_regs_client.status_string());
      return clock_regs_client.take_error();
    }
    clock_regs_ = fidl::WireSyncClient(std::move(clock_regs_client.value()));
    auto result = clock_regs_->Enable();
    if (!result.ok() || result->is_error()) {
      fdf::error(
          "Failed to enable clock-registers: {}",
          result.ok() ? zx_status_get_string(result->error_value()) : result.status_string());
      return zx::error(result.ok() ? result->error_value() : result.status());
    }
  }

  // Connect and release reset.
  {
    zx::result reset_client = incoming->Connect<fuchsia_hardware_reset::Service::Reset>("reset");
    if (reset_client.is_error()) {
      fdf::error("Failed to connect to reset: {}", reset_client.status_string());
      return reset_client.take_error();
    }
    reset_ = fidl::WireSyncClient(std::move(reset_client.value()));
    auto result = reset_->Deassert();
    if (!result.ok() || result->is_error()) {
      fdf::error("Failed to deassert reset: {}", result.ok()
                                                     ? zx_status_get_string(result->error_value())
                                                     : result.status_string());
      return zx::error(result.ok() ? result->error_value() : result.status());
    }
  }

  if (zx::result result = metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev);
      result.is_error()) {
    fdf::error("Failed to forward and serve metadata: {}", result);
    return result.take_error();
  }

  {
    zx::result mmio = MapMmio(pdev);
    if (mmio.is_error()) {
      fdf::error("Failed to map mmio: {}", mmio);
      return mmio.take_error();
    }
    mmio_.emplace(std::move(mmio.value()));
  }

  {
    zx::result interrupt = pdev.GetInterrupt(0);
    if (interrupt.is_error()) {
      fdf::error("Failed to get interrupt: {}", interrupt);
      return interrupt.take_error();
    }
    irq_ = std::move(interrupt.value());
  }

  // Initialize i2c host controller.
  if (auto result = HostInit(); result.is_error()) {
    fdf::error("failed to initialize i2c host controller {}", result.status_value());
    return result.take_error();
  }

  if (auto result = ServeI2cImpl(); result.is_error()) {
    fdf::error("Failed to serve i2c impl fidl protocol: {}",
               zx_status_get_string(result.status_value()));
    return result.take_error();
  }

  if (auto result = CreateChildNode(); result.is_error()) {
    fdf::error("Failed to create child node: {}", zx_status_get_string(result.status_value()));
    return result.take_error();
  }

  return zx::ok();
}

void DwI2c::Stop(fdf::StopCompleter completer) { completer(zx::ok()); }

zx::result<> DwI2c::ServeI2cImpl() {
  auto handler = fuchsia_hardware_i2cimpl::Service::InstanceHandler(
      {.device = i2cimpl_bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                                 fidl::kIgnoreBindingClosure)});

  zx::result result = outgoing()->AddService<fuchsia_hardware_i2cimpl::Service>(std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add I2C impl service to outgoing: {}", result);
    return result.take_error();
  }

  return zx::ok();
}

zx::result<> DwI2c::CreateChildNode() {
  zx::result controller_endpoints =
      fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
  if (!controller_endpoints.is_ok()) {
    fdf::error("Failed to create controller endpoints: {}", controller_endpoints);
    return controller_endpoints.take_error();
  }

  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_i2cimpl::Service>(component::kDefaultInstance),
  };
  if (std::optional offer = metadata_server_.CreateOffer(); offer.has_value()) {
    offers.push_back(std::move(offer.value()));
  }

  zx::result child =
      AddChild(kChildNodeName, std::vector<fuchsia_driver_framework::NodeProperty2>{}, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_controller_.Bind(std::move(child.value()));

  return zx::ok();
}

}  // namespace dw_i2c

FUCHSIA_DRIVER_EXPORT2(dw_i2c::DwI2c);
