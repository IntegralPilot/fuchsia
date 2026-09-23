// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_AML_UART_AML_UART_H_
#define SRC_DEVICES_SERIAL_DRIVERS_AML_UART_AML_UART_H_

#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/driver/power/cpp/wake-lease.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <fbl/mutex.h>

namespace serial {

namespace internal {

class DriverTransportReadOperation {
  using ReadCompleter = fidl::internal::WireCompleter<fuchsia_hardware_serialimpl::Device::Read>;

 public:
  DriverTransportReadOperation(fdf::Arena arena, ReadCompleter::Async completer)
      : arena_(std::move(arena)), completer_(std::move(completer)) {}

  fit::closure MakeCallback(zx_status_t status, void* buf, size_t len);

 private:
  fdf::Arena arena_;
  ReadCompleter::Async completer_;
};

class DriverTransportWriteOperation {
  using WriteCompleter = fidl::internal::WireCompleter<fuchsia_hardware_serialimpl::Device::Write>;

 public:
  DriverTransportWriteOperation(fdf::Arena arena, WriteCompleter::Async completer)
      : arena_(std::move(arena)), completer_(std::move(completer)) {}

  fit::closure MakeCallback(zx_status_t status);

 private:
  fdf::Arena arena_;
  WriteCompleter::Async completer_;
};

}  // namespace internal

class AmlUart : public fdf::WireServer<fuchsia_hardware_serialimpl::Device> {
 public:
  explicit AmlUart(fdf::PDev pdev,
                   const fuchsia_hardware_serial::wire::SerialPortInfo& serial_port_info,
                   fdf::MmioBuffer mmio,
                   fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag = {});

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

  // Test functions: simulate a data race where the HandleTX / HandleRX functions get called twice.
  void HandleTXRaceForTest();
  void HandleRXRaceForTest();

  const fuchsia_hardware_serial::wire::SerialPortInfo& serial_port_info() const {
    return serial_port_info_;
  }

 private:
  bool Readable();
  bool Writable();
  void EnableInner(bool enable);
  void HandleRX();
  void HandleTX();
  fit::closure MakeReadCallback(zx_status_t status, void* buf, size_t len);
  fit::closure MakeWriteCallback(zx_status_t status);

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);

  fdf::PDev pdev_;
  const fuchsia_hardware_serial::wire::SerialPortInfo serial_port_info_;
  fdf::MmioBuffer mmio_;

  bool enabled_ = false;

  // Reads
  std::optional<internal::DriverTransportReadOperation> read_operation_;

  // Writes
  std::optional<internal::DriverTransportWriteOperation> write_operation_;
  const uint8_t* write_buffer_ = nullptr;
  size_t write_size_ = 0;

  zx::interrupt irq_;
  async::IrqMethod<AmlUart, &AmlUart::HandleIrq> irq_handler_{this};

  std::optional<fdf_power::TimeoutWakeLease> wake_lease_;
};

}  // namespace serial

#endif  // SRC_DEVICES_SERIAL_DRIVERS_AML_UART_AML_UART_H_
