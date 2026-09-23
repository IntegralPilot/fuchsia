// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_UART16550_UART16550_H_
#define SRC_DEVICES_SERIAL_DRIVERS_UART16550_UART16550_H_

#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/wire.h>
#include <lib/stdcompat/span.h>
#include <zircon/compiler.h>

#include <mutex>
#include <optional>
#include <thread>
#include <variant>
#include <vector>

#include <ddktl/device.h>
#include <hwreg/bitfields.h>
#include <hwreg/pio.h>

#include "sdk/lib/driver/outgoing/cpp/outgoing_directory.h"
#include "src/devices/lib/acpi/client.h"

#if UART16550_TESTING
#include <hwreg/mock.h>
#endif

namespace uart16550 {

class Uart16550;
using DeviceType = ddk::Device<Uart16550>;

class Uart16550 : public DeviceType, public fdf::WireServer<fuchsia_hardware_serialimpl::Device> {
 public:
  Uart16550();

  explicit Uart16550(zx_device_t* parent, acpi::Client acpi);

  size_t FifoDepth() const;

  bool Enabled();

  static zx_status_t Create(void* ctx, zx_device_t* parent);

  zx_status_t Init();

#if UART16550_TESTING
  // test-use only
  zx_status_t Init(zx::interrupt interrupt, hwreg::Mock::RegisterIo port_mock);
#endif

  // test-use only
  zx::unowned_interrupt InterruptHandle();

  // ddk::Releasable
  void DdkRelease();

  fidl::ProtocolHandler<fuchsia_hardware_serialimpl::Device> GetHandler();

 private:
  struct WriteContext {
    WriteContext(WriteCompleter::Async completer, cpp20::span<const uint8_t> data)
        : completer(std::move(completer)), data(data) {}

    WriteCompleter::Async completer;
    cpp20::span<const uint8_t> data;
  };

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override;

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override;

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override;

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request, fdf::Arena& arena,
             WriteCompleter::Sync& completer) override;

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) override;

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device>
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  bool SupportsAutomaticFlowControl() const;

  zx_status_t Config(uint32_t baud_rate, uint32_t flags);

  zx_status_t Enable(bool enable);

  void CancelAll();

  void ResetFifosLocked() __TA_REQUIRES(device_mutex_);

  void InitFifosLocked() __TA_REQUIRES(device_mutex_);

  void HandleInterrupts();

  // Returns the number of bytes read from the RX FIFO.
  size_t DrainRxFifo(cpp20::span<uint8_t> buffer) __TA_REQUIRES(device_mutex_);

  // Fills the TX FIFO with as many bytes as possible, and returns a subspan pointing to the
  // remaining data that did not fit.
  cpp20::span<const uint8_t> FillTxFifo(cpp20::span<const uint8_t> data)
      __TA_REQUIRES(device_mutex_);

  acpi::Client acpi_fidl_;
  std::mutex device_mutex_;

  std::thread interrupt_thread_;
  zx::interrupt interrupt_;

#if UART16550_TESTING
  // This should never be used before Init, but must be default-constructible.
  // The Mock is the default (first) variant so it's default-constructible.
  std::variant<hwreg::Mock::RegisterIo, hwreg::RegisterPio> port_io_ __TA_GUARDED(device_mutex_){
      std::in_place_index<0>};
#else
  std::variant<hwreg::RegisterPio> port_io_ __TA_GUARDED(device_mutex_){nullptr};
#endif

  size_t uart_fifo_len_ = 1;

  bool enabled_ __TA_GUARDED(device_mutex_) = false;

  std::optional<ReadCompleter::Async> read_completer_ __TA_GUARDED(device_mutex_);

  std::vector<uint8_t> write_buffer_ __TA_GUARDED(device_mutex_);
  std::optional<WriteContext> write_context_ __TA_GUARDED(device_mutex_);

  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> bindings_;
  fdf::OutgoingDirectory outgoing_;
};  // namespace uart16550

}  // namespace uart16550

#endif  // SRC_DEVICES_SERIAL_DRIVERS_UART16550_UART16550_H_
