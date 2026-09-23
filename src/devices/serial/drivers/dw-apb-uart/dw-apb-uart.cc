// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/serial/drivers/dw-apb-uart/dw-apb-uart.h"

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.reset/cpp/wire.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fit/defer.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/serial/cpp/bind.h>

namespace fhsi = fuchsia_hardware_serialimpl;

namespace serial {

namespace {
constexpr std::string_view kPdevName = "pdev";
constexpr std::string_view kChildName = "dw-apb-uart";
}  // namespace

namespace internal {

void DriverTransportReadOperation::Reply(zx_status_t status, fidl::VectorView<uint8_t> data) {
  if (status == ZX_OK) {
    completer_.buffer(arena_).ReplySuccess(data);
  } else {
    completer_.buffer(arena_).ReplyError(status);
  }
}

void DriverTransportWriteOperation::Reply(zx_status_t status) {
  if (status == ZX_OK) {
    completer_.buffer(arena_).ReplySuccess();
  } else {
    completer_.buffer(arena_).ReplyError(status);
  }
}

}  // namespace internal

DwApbUart::DwApbUart(const fuchsia_hardware_serial::wire::SerialPortInfo& serial_port_info,
                     fdf::MmioBuffer mmio, zx::interrupt irq,
                     fidl::ClientEnd<fuchsia_hardware_clock::Clock> baud_clock)
    : serial_port_info_(serial_port_info),
      mmio_(std::move(mmio)),
      uart_(zbi_dcfg_simple_t{.mmio_phys = 0, .irq = 0}),
      io_(mmio_),
      baud_clock_(std::move(baud_clock)),
      irq_(std::move(irq)) {}

// DwApbUart destructor cancels any pending read/write operations and disables the baud clock.
DwApbUart::~DwApbUart() {
  if (read_operation_) {
    CompleteRead(ZX_ERR_CANCELED);
  }
  if (write_operation_) {
    CompleteWrite(ZX_ERR_CANCELED);
  }
  if (baud_clock_.is_valid()) {
    auto result = baud_clock_->Disable();
    if (!result.ok()) {
      fdf::error("Failed to disable baud_clock in destructor: {}", result.status_string());
    }
  }
}

// GetInfo returns static serial port configuration (class, VID, PID).
void DwApbUart::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(serial_port_info_);
}

// Config configures UART line settings (data bits, parity, stop bits) and baud rate.
// It maps FIDL configuration flags to the internal driver representations.
zx_status_t DwApbUart::Config(uint32_t baud_rate, uint32_t flags) {
  std::optional<uart::DataBits> data_bits;
  std::optional<uart::Parity> parity;
  std::optional<uart::StopBits> stop_bits;

  if ((flags & fhsi::kSerialSetBaudRateOnly) == 0) {
    if ((flags & fhsi::kSerialFlowCtrlMask) != fhsi::kSerialFlowCtrlNone) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    switch (flags & fhsi::kSerialDataBitsMask) {
      case fhsi::kSerialDataBits5:
        data_bits = uart::DataBits::k5;
        break;
      case fhsi::kSerialDataBits6:
        data_bits = uart::DataBits::k6;
        break;
      case fhsi::kSerialDataBits7:
        data_bits = uart::DataBits::k7;
        break;
      case fhsi::kSerialDataBits8:
        data_bits = uart::DataBits::k8;
        break;
      default:
        return ZX_ERR_INVALID_ARGS;
    }

    switch (flags & fhsi::kSerialStopBitsMask) {
      case fhsi::kSerialStopBits1:
        stop_bits = uart::StopBits::k1;
        break;
      case fhsi::kSerialStopBits2:
        stop_bits = uart::StopBits::k2;
        break;
      default:
        return ZX_ERR_INVALID_ARGS;
    }

    switch (flags & fhsi::kSerialParityMask) {
      case fhsi::kSerialParityNone:
        parity = uart::Parity::kNone;
        break;
      case fhsi::kSerialParityEven:
        parity = uart::Parity::kEven;
        break;
      case fhsi::kSerialParityOdd:
        parity = uart::Parity::kOdd;
        break;
      default:
        return ZX_ERR_INVALID_ARGS;
    }

    // Note: SetLineControl redundantly writes a default divisor of 1 to DLL/DLH.
    // This is overwritten below if baud_rate > 0.
    // TODO(https://fxbug.dev/42053694): Update when the uart library separates baud rate
    // configuration.
    uart_.SetLineControl(io_, data_bits, parity, stop_bits);
  }

  if (baud_rate == 0) {
    return ZX_OK;
  }

  if (!baud_clock_.is_valid()) {
    fdf::error("Baud clock is invalid.");
    return ZX_ERR_BAD_STATE;
  }
  auto result = baud_clock_->GetRate();
  if (!result.ok()) {
    fdf::error("Failed to call GetRate on baudclk: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("GetRate on baudclk returned error: {}", result->error_value());
    return result->error_value();
  }
  uint64_t clock_freq = result->value()->hz;

  // Ensure UART is idle before modifying divisor latch. The DesignWare UART
  // busy detect feature may ignore writes to LCR or divisor registers if the
  // UART is actively transmitting or receiving.
  if (!uart_.WaitDuringBusy(io_)) {
    fdf::error(
        "UART remains busy before modifying divisor latch (requested baud_rate={}, clock_freq={}). "
        "Timing out configuration.",
        baud_rate, clock_freq);
    return ZX_ERR_TIMED_OUT;
  }

  // Set DLAB = 1 in LCR to access divisor latches (DLL, DLH, DLF)
  auto lcr = uart::dw8250::LineControlRegister::Get().ReadFrom(io_.io());
  lcr.set_divisor_latch_access(true).WriteTo(io_.io());

  // Test if DLF (Fractional Divisor) register is supported. The Component
  // Parameter Register (CPR) indicates if the fractional divisor extension is
  // implemented in this hardware configuration. If supported, we can achieve
  // more accurate baud rates using fractional division.
  auto cpr = uart::dw8250::ComponentParameterRegister::Get().ReadFrom(io_.io());
  bool dlf_supported = cpr.uart_add_encoded_params() && cpr.additional_feat();

  uint32_t divisor = 0;
  uint32_t dlf_val = 0;

  if (dlf_supported) {
    // Calculate divisor in 16ths (4-bit fractional part)
    // D_16th = clock_freq / baud_rate
    uint64_t d_16th =
        (clock_freq + (static_cast<uint64_t>(baud_rate) / 2)) / static_cast<uint64_t>(baud_rate);
    divisor = static_cast<uint32_t>(d_16th / 16);
    dlf_val = static_cast<uint32_t>(d_16th % 16);
  } else {
    uint64_t denom = 16 * static_cast<uint64_t>(baud_rate);
    divisor = static_cast<uint32_t>((clock_freq + (denom / 2)) / denom);
  }

  if (divisor == 0 || divisor > 0xFFFF) {
    // Clear DLAB in LCR on error after waiting for UART to not be busy
    if (!uart_.WaitDuringBusy(io_)) {
      fdf::warn("DwApbUart::Config UART busy while clearing DLAB on invalid divisor error");
    }
    lcr.set_divisor_latch_access(false).WriteTo(io_.io());
    fdf::error("DwApbUart::Config invalid divisor={} for baud_rate={} (clock_freq={})", divisor,
               baud_rate, clock_freq);
    return ZX_ERR_INVALID_ARGS;
  }

  // Per Synopsys Databook Section 6.4, DLF must be programmed before DLL and DLH.
  if (dlf_supported) {
    DivisorLatchFractionRegister::Get().FromValue(0).set_fractional_divisor(dlf_val).WriteTo(
        io_.io());
    fdf::info("DwApbUart::Config set baud_rate={} divisor={}.{:03} (clock_freq={}, DLF supported)",
              baud_rate, divisor, (dlf_val * 1000) / 16, clock_freq);
  } else {
    fdf::info("DwApbUart::Config set baud_rate={} divisor={} (clock_freq={}, DLF not supported)",
              baud_rate, divisor, clock_freq);
  }

  // Write divisor integer part to DLL (offset 0) and DLH (offset 1)
  io_.io()->Write<uint32_t>(divisor & 0xFF, 0);
  io_.io()->Write<uint32_t>((divisor >> 8) & 0xFF, 1);

  // Clear DLAB in LCR after waiting for UART to not be busy
  if (!uart_.WaitDuringBusy(io_)) {
    fdf::warn("DwApbUart::Config UART busy while clearing DLAB");
  }
  lcr.set_divisor_latch_access(false).WriteTo(io_.io());

  // Flush any line errors / junk bytes created during the baud rate switch
  (void)uart::dw8250::LineStatusRegister::Get().ReadFrom(io_.io());
  (void)uart::dw8250::RxBufferRegister::Get().ReadFrom(io_.io());
  (void)uart::dw8250::UartStatusRegister::Get().ReadFrom(io_.io());
  auto fcr = uart::dw8250::FifoControlRegister::Get().FromValue(0);
  fcr.set_fifo_enable(true)
      .set_rx_fifo_reset(true)
      .set_tx_fifo_reset(true)
      .set_receiver_trigger(uart::dw8250::FifoControlRegister::kReceiveTriggerLevel1Char)
      .WriteTo(io_.io());

  return ZX_OK;
}

void DwApbUart::EnableInner(bool enable) {
  if (enable) {
    uart_.Init(io_);
    // Drain any leftover status/bytes and reset FIFOs before enabling interrupts
    (void)uart::dw8250::LineStatusRegister::Get().ReadFrom(io_.io());
    (void)uart::dw8250::RxBufferRegister::Get().ReadFrom(io_.io());
    (void)uart::dw8250::UartStatusRegister::Get().ReadFrom(io_.io());
    auto fcr = uart::dw8250::FifoControlRegister::Get().FromValue(0);
    fcr.set_fifo_enable(true)
        .set_rx_fifo_reset(true)
        .set_tx_fifo_reset(true)
        .set_receiver_trigger(uart::dw8250::FifoControlRegister::kReceiveTriggerLevel1Char)
        .WriteTo(io_.io());
    auto ier = uart::dw8250::InterruptEnableRegister::Get().ReadFrom(io_.io());
    ier.set_rx_available(true)
        .set_line_status(true)
        .set_lsr_clear_status_on_read(true)
        .set_tx_empty(false)
        .WriteTo(io_.io());
  } else {
    uart::dw8250::InterruptEnableRegister::Get().FromValue(0).WriteTo(io_.io());
  }
}

// Enable manages the driver lifecycle state. On enable, it obtains the IRQ,
// binds the IRQ handler, and starts the hardware. On disable, it stops the
// hardware and cancels the IRQ handler.
zx_status_t DwApbUart::Enable(bool enable) {
  fdf::info("DwApbUart::Enable {}", enable);
  if (enable && !enabled_) {
    if (!irq_.is_valid()) {
      fdf::error("DwApbUart::Enable: interrupt is invalid.");
      return ZX_ERR_BAD_STATE;
    }

    EnableInner(true);

    irq_handler_.set_object(irq_.get());
    zx_status_t status = irq_handler_.Begin(fdf::Dispatcher::GetCurrent()->async_dispatcher());
    if (status != ZX_OK) {
      fdf::error("Failed to begin IRQ handler: {}", zx_status_get_string(status));
      EnableInner(false);
      return status;
    }
  } else if (!enable && enabled_) {
    irq_handler_.Cancel();
    EnableInner(false);
  }

  enabled_ = enable;
  return ZX_OK;
}

// CancelAll aborts all pending asynchronous read and write transactions,
// completing them with ZX_ERR_CANCELED.
void DwApbUart::CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) {
  fdf::info("DwApbUart::CancelAll");
  if (read_operation_) {
    CompleteRead(ZX_ERR_CANCELED);
  }
  if (write_operation_) {
    CompleteWrite(ZX_ERR_CANCELED);
  }
  completer.buffer(arena).Reply();
  fdf::info("DwApbUart::CancelAll done");
}

// HandleRX satisfies a pending read operation using data from the local ring buffer.
// If a read is active and the buffer has data, we copy it to the reader, complete the FIDL call,
// and re-enable RX interrupts if the buffer was previously full.
void DwApbUart::HandleRX() {
  if (!read_operation_) {
    return;
  }
  if (rx_buffer_.empty()) {
    return;
  }
  fdf::Arena& arena = read_operation_->arena();
  size_t read_size = rx_buffer_.size();
  fidl::VectorView<uint8_t> buf(arena, read_size);

  for (size_t i = 0; i < read_size; ++i) {
    buf[i] = rx_buffer_.front();
    rx_buffer_.pop();
  }

  uart_.EnableRxInterrupt(io_, true);

  fdf::debug("DwApbUart::HandleRX read {} bytes from ring buffer: '{:x}'", read_size, buf[0]);
  CompleteRead(ZX_OK, buf);
}

// HandleTX writes pending transmit buffer data to the hardware FIFO.
// It loops writing characters as long as there is data and the UART TX FIFO is ready.
// Completes the write FIDL call when all data is written.
void DwApbUart::HandleTX() {
  if (!write_operation_) {
    uart_.EnableTxInterrupt(io_, false);
    return;
  }
  const auto* bufptr = tx_buffer_.data() + tx_index_;
  const uint8_t* const end = tx_buffer_.data() + tx_buffer_.size();
  size_t write_size = tx_buffer_.size() - tx_index_;

  while (write_size > 0 && uart_.TxReady(io_)) {
    auto next = uart_.Write(io_, false, bufptr, end);
    const size_t written = next - bufptr;
    if (written == 0) {
      fdf::error("DwApbUart::HandleTX: uart_.Write made no progress.");
      break;
    }
    tx_index_ += written;
    write_size -= written;
    bufptr = next;
  }

  if (tx_index_ == tx_buffer_.size()) {
    CompleteWrite(ZX_OK);
  } else {
    uart_.EnableTxInterrupt(io_, true);
  }
}

void DwApbUart::CompleteRead(zx_status_t status, fidl::VectorView<uint8_t> data) {
  if (read_operation_) {
    read_operation_->Reply(status, data);
    read_operation_.reset();
    return;
  }
  fdf::error("DwApbUart::CompleteRead invalid state. No active Read operation.");
}

void DwApbUart::CompleteWrite(zx_status_t status) {
  if (write_operation_) {
    uart_.EnableTxInterrupt(io_, false);
    tx_buffer_.clear();
    tx_index_ = 0;
    write_operation_->Reply(status);
    write_operation_.reset();
    return;
  }
  fdf::error("DwApbUart::CompleteWrite invalid state. No active Write operation.");
}

void DwApbUart::Config(ConfigRequestView request, fdf::Arena& arena,
                       ConfigCompleter::Sync& completer) {
  zx_status_t status = Config(request->baud_rate, request->flags);
  if (status == ZX_OK) {
    completer.buffer(arena).ReplySuccess();
  } else {
    completer.buffer(arena).ReplyError(status);
  }
}

void DwApbUart::Enable(EnableRequestView request, fdf::Arena& arena,
                       EnableCompleter::Sync& completer) {
  zx_status_t status = Enable(request->enable);
  if (status == ZX_OK) {
    completer.buffer(arena).ReplySuccess();
  } else {
    completer.buffer(arena).ReplyError(status);
  }
}

// Read initiates an asynchronous read operation. We only support one active read
// operation at a time.
void DwApbUart::Read(fdf::Arena& arena, ReadCompleter::Sync& completer) {
  if (read_operation_) {
    fdf::warn("DwApbUart::Read: ALREADY_BOUND");
    completer.buffer(arena).ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  read_operation_.emplace(std::move(arena), completer.ToAsync());
  HandleRX();
}

// Write initiates an asynchronous write operation. We only support one active write
// operation at a time.
void DwApbUart::Write(WriteRequestView request, fdf::Arena& arena,
                      WriteCompleter::Sync& completer) {
  if (write_operation_) {
    fdf::warn("DwApbUart::Write: ALREADY_BOUND");
    completer.buffer(arena).ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  tx_buffer_.assign(request->data.begin(), request->data.end());
  tx_index_ = 0;
  write_operation_.emplace(std::move(arena), completer.ToAsync());
  HandleTX();
}

void DwApbUart::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("handle_unknown_method in fuchsia_hardware_serialimpl::Device server.");
}

void DwApbUart::HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                          const zx_packet_interrupt_t* interrupt) {
  if (status != ZX_OK) {
    fdf::info("DwApbUart::HandleIrq status={}", status);
    return;
  }

  // Loop to process all pending interrupts reported by the hardware.
  // The 8250/DesignWare IIR reports the highest-priority pending interrupt
  // (Line Status > RX Available/Timeout > TX Empty > Modem Status > Busy Detect).
  // We iterate through the loop servicing each interrupt source until IIR
  // returns kNone, allowing multiple pending conditions to be cleared in a
  // single IRQ dispatch. We cap iterations at kMaxIrqLoopIterations to prevent
  // the driver host from hanging if a hardware condition cannot be cleared.
  size_t loop_count = 0;
  for (; loop_count < kMaxIrqLoopIterations; loop_count++) {
    auto iir = uart::dw8250::InterruptIdentRegister::Get().ReadFrom(io_.io());
    auto id = iir.interrupt_id();
    if (id == uart::dw8250::InterruptType::kNone) {
      break;
    }

    fdf::debug("DwApbUart::HandleIrq iir={:x} id={}", iir.reg_value(), static_cast<uint32_t>(id));

    switch (id) {
      case uart::dw8250::InterruptType::kRxLineStatus: {
        // Reading LSR logs error flags and clears the line status interrupt condition in hardware.
        auto lsr = uart::dw8250::LineStatusRegister::Get().ReadFrom(io_.io());
        fdf::warn(
            "DwApbUart::HandleIrq RX Line Status Error: LSR={:02x} (OE={} PE={} FE={} BI={} FIFO_ERR={})",
            lsr.reg_value(), lsr.overrun_error(), lsr.parity_error(), lsr.framing_error(),
            lsr.break_interrupt(), lsr.error_in_rx_fifo());
        [[fallthrough]];
      }
      case uart::dw8250::InterruptType::kRxDataAvailable:
      case uart::dw8250::InterruptType::kCharTimeout: {
        // Drain all available bytes from the hardware RX FIFO into our software ring buffer.
        // Draining before completing the read prevents hardware FIFO overruns and preserves
        // valid bytes preceding any error condition.
        while (!rx_buffer_.full()) {
          auto char_opt = uart_.Read(io_);
          if (!char_opt) {
            break;
          }
          rx_buffer_.push(*char_opt);
        }

        if (rx_buffer_.full()) {
          fdf::error("DwApbUart::HandleIrq: RX buffer full, disabling RX interrupt.");
          uart_.EnableRxInterrupt(io_, false);
        }

        HandleRX();
        break;
      }
      case uart::dw8250::InterruptType::kTxEmpty: {
        // TX FIFO / holding register is empty. Feed pending data if a write operation is active,
        // otherwise mask TX interrupts to prevent continuous interrupt firing while idle.
        if (uart_.TxReady(io_) && write_operation_) {
          HandleTX();
        } else {
          uart_.EnableTxInterrupt(io_, false);
        }
        break;
      }
      case uart::dw8250::InterruptType::kModemStatus: {
        // Reading MSR clears modem status interrupt.
        uart::dw8250::ModemStatusRegister::Get().ReadFrom(io_.io());
        break;
      }
      case uart::dw8250::InterruptType::kDwBusyDetect: {
        // Reading USR clears DesignWare busy detect interrupt.
        uart::dw8250::UartStatusRegister::Get().ReadFrom(io_.io());
        break;
      }
      default: {
        fdf::error(
            "DwApbUart::HandleIrq unexpected interrupt type: {}. Disabling interrupts to prevent storm.",
            static_cast<uint32_t>(id));
        uart_.EnableRxInterrupt(io_, false);
        uart_.EnableTxInterrupt(io_, false);
        auto ier = uart::dw8250::InterruptEnableRegister::Get().ReadFrom(io_.io());
        ier.set_line_status(false)
            .set_rx_available(false)
            .set_tx_empty(false)
            .set_modem_status(false)
            .WriteTo(io_.io());
        break;
      }
    }
  }

  if (loop_count >= kMaxIrqLoopIterations) {
    const zx::time now = zx::clock::get_monotonic();
    if (now - last_yield_warning_time_ >= kYieldWarningInterval) {
      if (yield_warnings_suppressed_ > 0) {
        fdf::warn(
            "DwApbUart::HandleIrq reached iteration limit ({}), yielding to dispatcher "
            "(suppressed {} similar warnings in the last {}s).",
            kMaxIrqLoopIterations, yield_warnings_suppressed_, kYieldWarningInterval.to_secs());
      } else {
        fdf::warn("DwApbUart::HandleIrq reached iteration limit ({}), yielding to dispatcher.",
                  kMaxIrqLoopIterations);
      }
      last_yield_warning_time_ = now;
      yield_warnings_suppressed_ = 0;
    } else {
      yield_warnings_suppressed_++;
    }
  }

  irq_.ack();
}

void DwApbUart::Bind(fdf_dispatcher_t* dispatcher,
                     fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

zx::result<> DwApbUartDriver::Start(fdf::DriverContext context) {
  incoming_ = context.take_incoming();

  zx::result pdev_client_end =
      incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>(kPdevName);
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end.status_string());
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  // Acquire the interrupt before enabling power, clocks, or deasserting reset.
  // If the interrupt is already claimed (e.g., by the early Zircon kernel serial console),
  // starting the userspace driver must fail without resetting or reconfiguring the
  // hardware actively owned by the kernel console.
  zx::result irq = pdev.GetInterrupt(0, 0);
  if (irq.is_error()) {
    if (irq.status_value() == ZX_ERR_ALREADY_BOUND) {
      fdf::info(
          "UART interrupt is already bound (kernel serial console active). "
          "Skipping userspace driver initialization.");
    } else {
      fdf::error("Failed to get interrupt: {}", irq.status_string());
    }
    return irq.take_error();
  }

  auto power_client =
      incoming_->Connect<fuchsia_hardware_powerdomain::Service::Domain>("power-domain");
  if (power_client.is_error()) {
    fdf::error("Failed to connect to power-domain service: {}", power_client.status_string());
    return power_client.take_error();
  }
  fdf::debug("Found power-domain parent. Enabling it...");
  power_domain_client_.Bind(std::move(power_client.value()));
  {
    auto result = power_domain_client_->Enable();
    if (!result.ok()) {
      fdf::error("Failed to call Enable on power domain: {}", result.status_string());
      return zx::error(result.status());
    } else if (result->is_error()) {
      fdf::error("Power domain enable returned error: {}", result->error_value());
      return zx::error(result->error_value());
    }
    fdf::debug("Successfully enabled power domain.");
  }
  auto disable_power = fit::defer([this]() {
    if (power_domain_client_.is_valid()) {
      auto result = power_domain_client_->Disable();
      if (!result.ok()) {
        fdf::error("Failed to disable power domain on error path: {}", result.status_string());
      }
    }
  });

  // Enable apb_pclk clock (required for register access)
  auto apb_pclk_client = incoming_->Connect<fuchsia_hardware_clock::Service::Clock>("apb_pclk");
  if (apb_pclk_client.is_error()) {
    fdf::error("Failed to connect to apb_pclk service: {}", apb_pclk_client.status_string());
    return apb_pclk_client.take_error();
  }
  apb_pclk_client_.Bind(std::move(apb_pclk_client.value()));
  {
    auto result = apb_pclk_client_->Enable();
    if (!result.ok()) {
      fdf::error("Failed to enable apb_pclk: {}", result.status_string());
      return zx::error(result.status());
    } else if (result->is_error()) {
      fdf::error("apb_pclk enable returned error: {}", result->error_value());
      return zx::error(result->error_value());
    }
    fdf::debug("Successfully enabled apb_pclk.");
  }
  auto disable_apb_pclk = fit::defer([this]() {
    if (apb_pclk_client_.is_valid()) {
      auto result = apb_pclk_client_->Disable();
      if (!result.ok()) {
        fdf::error("Failed to disable apb_pclk on error path: {}", result.status_string());
      }
    }
  });

  // Connect and enable baudclk (baud rate generator clock)
  auto baudclk_client = incoming_->Connect<fuchsia_hardware_clock::Service::Clock>("baudclk");
  if (baudclk_client.is_error()) {
    fdf::error("Failed to connect to clock-baudclk service: {}", baudclk_client.status_string());
    return baudclk_client.take_error();
  }
  baud_clock_client_.Bind(std::move(baudclk_client.value()));
  {
    auto result = baud_clock_client_->Enable();
    if (!result.ok()) {
      fdf::error("Failed to enable baudclk: {}", result.status_string());
      return zx::error(result.status());
    } else if (result->is_error()) {
      fdf::error("baudclk enable returned error: {}", result->error_value());
      return zx::error(result->error_value());
    }
    fdf::debug("Successfully enabled baudclk.");
  }
  auto disable_baud_clock = fit::defer([this]() {
    if (baud_clock_client_.is_valid()) {
      auto result = baud_clock_client_->Disable();
      if (!result.ok()) {
        fdf::error("Failed to disable baud_clock on error path: {}", result.status_string());
      }
    }
  });

  // Deassert reset
  auto reset_client = incoming_->Connect<fuchsia_hardware_reset::Service::Reset>("reset");
  if (reset_client.is_error()) {
    fdf::error("Failed to connect to reset service: {}", reset_client.status_string());
    return reset_client.take_error();
  }
  reset_client_.Bind(std::move(reset_client.value()));
  {
    auto result = reset_client_->Deassert();
    if (!result.ok()) {
      fdf::error("Failed to deassert reset: {}", result.status_string());
      return zx::error(result.status());
    } else if (result->is_error()) {
      fdf::error("reset deassert returned error: {}", result->error_value());
      return zx::error(result->error_value());
    }
    fdf::debug("Successfully deasserted reset.");
  }
  auto assert_reset = fit::defer([this]() {
    if (reset_client_.is_valid()) {
      auto result = reset_client_->Assert();
      if (!result.ok()) {
        fdf::error("Failed to assert reset on error path: {}", result.status_string());
      }
    }
  });

  zx::result metadata = pdev.GetFidlMetadata<fuchsia_hardware_serial::SerialPortInfo>(
      fuchsia_hardware_serial::SerialPortInfo::kSerializableName);
  if (metadata.is_error()) {
    if (metadata.status_value() == ZX_ERR_NOT_FOUND) {
      fdf::debug("Serial port info metadata not found.");
    } else {
      fdf::error("Failed to get metadata: {}", metadata);
      return metadata.take_error();
    }
    serial_port_info_ = {
        .serial_class = fuchsia_hardware_serial::Class::kGeneric,
        .serial_vid = 0,
        .serial_pid = 0,
    };
  } else {
    serial_port_info_ = {
        .serial_class = metadata->serial_class(),
        .serial_vid = metadata->serial_vid(),
        .serial_pid = metadata->serial_pid(),
    };
  }

  zx::result mmio = pdev.MapMmio(0);
  if (mmio.is_error()) {
    fdf::error("Failed to map mmio: {}", mmio.status_string());
    return mmio.take_error();
  }

  dw_apb_uart_.emplace(serial_port_info_, std::move(mmio.value()), std::move(irq.value()),
                       baud_clock_client_.TakeClientEnd());
  disable_baud_clock.cancel();

  fuchsia_hardware_serialimpl::Service::InstanceHandler handler({
      .device =
          [this](fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server_end) {
            dw_apb_uart_->Bind(driver_dispatcher()->get(), std::move(server_end));
          },
  });
  zx::result<> add_result =
      outgoing()->AddService<fuchsia_hardware_serialimpl::Service>(std::move(handler), kChildName);
  if (add_result.is_error()) {
    fdf::error("Failed to add fuchsia_hardware_serialimpl::Service: {}",
               add_result.status_string());
    return add_result.take_error();
  }

  std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(kChildName),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {{
      fdf::MakeProperty2(bind_fuchsia::SERIAL_CLASS,
                         static_cast<uint32_t>(dw_apb_uart_->serial_port_info().serial_class)),
  }};

  auto result = AddChild(std::string(kChildName), properties, offers);
  if (result.is_error()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return result.take_error();
  }
  node_controller_ = std::move(result.value());

  fdf::info("Successfully started dw-apb-uart driver.");

  // Cancel all cleanups on success.
  disable_power.cancel();
  disable_apb_pclk.cancel();
  assert_reset.cancel();

  return zx::ok();
}

void DwApbUartDriver::Stop(fdf::StopCompleter completer) {
  fdf::info("Stopping dw-apb-uart driver...");
  if (dw_apb_uart_.has_value()) {
    dw_apb_uart_->Enable(false);
    // Destroy DwApbUart (which disables the baud clock via RAII and closes bindings)
    dw_apb_uart_.reset();
  }

  if (reset_client_.is_valid()) {
    auto result = reset_client_->Assert();
    if (!result.ok()) {
      fdf::error("Failed to assert reset in Stop: {}", result.status_string());
    } else if (result->is_error()) {
      fdf::error("reset assert returned error in Stop: {}", result->error_value());
    }
  }

  if (apb_pclk_client_.is_valid()) {
    auto result = apb_pclk_client_->Disable();
    if (!result.ok()) {
      fdf::error("Failed to disable apb_pclk in Stop: {}", result.status_string());
    } else if (result->is_error()) {
      fdf::error("apb_pclk disable returned error in Stop: {}", result->error_value());
    }
  }

  if (power_domain_client_.is_valid()) {
    auto result = power_domain_client_->Disable();
    if (!result.ok()) {
      fdf::error("Failed to disable power domain in Stop: {}", result.status_string());
    } else if (result->is_error()) {
      fdf::error("power domain disable returned error in Stop: {}", result->error_value());
    }
  }

  completer(zx::ok());
}

DwApbUart& DwApbUartDriver::dw_apb_uart_for_testing() {
  ZX_ASSERT(dw_apb_uart_.has_value());
  return dw_apb_uart_.value();
}

}  // namespace serial

FUCHSIA_DRIVER_EXPORT2(serial::DwApbUartDriver);
