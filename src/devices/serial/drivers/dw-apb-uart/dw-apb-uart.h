// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_DW_APB_UART_DW_APB_UART_H_
#define SRC_DEVICES_SERIAL_DRIVERS_DW_APB_UART_DW_APB_UART_H_

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.powerdomain/cpp/wire.h>
#include <fidl/fuchsia.hardware.reset/cpp/wire.h>
#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/fidl.h>
#include <lib/async/cpp/irq.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/fidl_driver/cpp/server.h>
#include <lib/uart/dw8250.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <memory>
#include <optional>

#include <fbl/ring_buffer.h>
#include <hwreg/bitfields.h>

namespace serial {

// Divisor Latch Fraction (DLF) register (offset 0xc0) for fractional baud rate divisor.
class DivisorLatchFractionRegister
    : public hwreg::RegisterBase<DivisorLatchFractionRegister, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 4);
  DEF_FIELD(3, 0, fractional_divisor);
  static auto Get() { return hwreg::RegisterAddr<DivisorLatchFractionRegister>(0xc0 / 4); }
};

namespace internal {

class DriverTransportReadOperation {
  using ReadCompleter = fidl::internal::WireCompleter<fuchsia_hardware_serialimpl::Device::Read>;

 public:
  DriverTransportReadOperation(fdf::Arena arena, ReadCompleter::Async completer)
      : arena_(std::move(arena)), completer_(std::move(completer)) {}

  void Reply(zx_status_t status, fidl::VectorView<uint8_t> data = {});

  fdf::Arena& arena() { return arena_; }

 private:
  fdf::Arena arena_;
  ReadCompleter::Async completer_;
};

class DriverTransportWriteOperation {
  using WriteCompleter = fidl::internal::WireCompleter<fuchsia_hardware_serialimpl::Device::Write>;

 public:
  DriverTransportWriteOperation(fdf::Arena arena, WriteCompleter::Async completer)
      : arena_(std::move(arena)), completer_(std::move(completer)) {}

  void Reply(zx_status_t status);

 private:
  fdf::Arena arena_;
  WriteCompleter::Async completer_;
};

}  // namespace internal

// MmioBufferIoProvider adapts fdf::MmioBuffer to the uart::dw8250::Driver IO interface.
// Note: `offset` represents a 32-bit register index (word offset), scaled internally
// by sizeof(uint32_t) (4 bytes) to compute the target MMIO byte offset.
class MmioBufferIoProvider {
 public:
  explicit MmioBufferIoProvider(fdf::MmioBuffer& mmio) : mmio_(mmio) {}

  template <typename T>
  T Read(size_t offset) const {
    static_assert(sizeof(T) == 4, "Only 32-bit accesses are supported");
    return mmio_.Read32(offset * 4);
  }

  template <typename T>
  void Write(T value, size_t offset) {
    static_assert(sizeof(T) == 4, "Only 32-bit accesses are supported");
    mmio_.Write32(value, offset * 4);
  }

 private:
  fdf::MmioBuffer& mmio_;
};

class DriverIoProvider {
 public:
  explicit DriverIoProvider(fdf::MmioBuffer& mmio) : io_(mmio) {}
  auto* io() { return &io_; }

 private:
  MmioBufferIoProvider io_;
};

class DwApbUart : public fdf::WireServer<fuchsia_hardware_serialimpl::Device> {
 public:
  explicit DwApbUart(const fuchsia_hardware_serial::wire::SerialPortInfo& serial_port_info,
                     fdf::MmioBuffer mmio, zx::interrupt irq,
                     fidl::ClientEnd<fuchsia_hardware_clock::Clock> baud_clock);
  ~DwApbUart() override;

  void Bind(fdf_dispatcher_t* dispatcher,
            fdf::ServerEnd<fuchsia_hardware_serialimpl::Device> server_end);

  zx_status_t Config(uint32_t baud_rate, uint32_t flags);
  zx_status_t Enable(bool enable);

  // fuchsia_hardware_serialimpl::Device FIDL implementation.
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;
  void Config(ConfigRequestView request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override;
  void Enable(EnableRequestView request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override;
  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override;
  void Write(WriteRequestView request, fdf::Arena& arena, WriteCompleter::Sync& completer) override;
  void CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  const fuchsia_hardware_serial::wire::SerialPortInfo& serial_port_info() const {
    return serial_port_info_;
  }

  static constexpr size_t kRxBufferSize = 16384;
  static constexpr size_t kMaxIrqLoopIterations = 128;
  static constexpr zx::duration kYieldWarningInterval = zx::sec(5);

 private:
  void EnableInner(bool enable);
  void HandleRX();
  void HandleTX();
  void CompleteRead(zx_status_t status, fidl::VectorView<uint8_t> data = {});
  void CompleteWrite(zx_status_t status);

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);

  const fuchsia_hardware_serial::wire::SerialPortInfo serial_port_info_;
  fdf::MmioBuffer mmio_;
  uart::dw8250::Driver uart_;
  DriverIoProvider io_;

  bool enabled_ = false;

  // Reads
  std::optional<internal::DriverTransportReadOperation> read_operation_;
  // 16KB buffer is selected to handle high baud rates (3M/6M) on target hardware.
  // At 6M baud, 16KB holds ~27ms of data. Since this is a userspace driver,
  // scheduler latency can exceed 10ms under load, so a larger buffer is
  // required to prevent RX FIFO overflows and subsequent packet drops.
  fbl::RingBuffer<uint8_t, kRxBufferSize> rx_buffer_;

  // Writes
  std::optional<internal::DriverTransportWriteOperation> write_operation_;
  std::vector<uint8_t> tx_buffer_;
  size_t tx_index_ = 0;

  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> baud_clock_;

  zx::interrupt irq_;
  async::IrqMethod<DwApbUart, &DwApbUart::HandleIrq> irq_handler_{this};
  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> bindings_;

  zx::time last_yield_warning_time_ = zx::time::infinite_past();
  uint64_t yield_warnings_suppressed_ = 0;
};

class DwApbUartDriver : public fdf::DriverBase2 {
 public:
  explicit DwApbUartDriver() : fdf::DriverBase2("dw-apb-uart") {}

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;

  DwApbUart& dw_apb_uart_for_testing();

 private:
  fuchsia_hardware_serial::wire::SerialPortInfo serial_port_info_;
  std::optional<DwApbUart> dw_apb_uart_;
  fidl::WireSyncClient<fuchsia_hardware_powerdomain::Domain> power_domain_client_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> apb_pclk_client_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> baud_clock_client_;
  fidl::WireSyncClient<fuchsia_hardware_reset::Reset> reset_client_;

  std::unique_ptr<fdf::Namespace> incoming_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> node_controller_;
};

}  // namespace serial

#endif  // SRC_DEVICES_SERIAL_DRIVERS_DW_APB_UART_DW_APB_UART_H_
