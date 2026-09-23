// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_FTDI_FTDI_H_
#define SRC_DEVICES_SERIAL_DRIVERS_FTDI_FTDI_H_

#include <fidl/fuchsia.hardware.ftdi/cpp/wire.h>
#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fuchsia/hardware/usb/c/banjo.h>
#include <lib/ddk/device.h>
#include <lib/stdcompat/span.h>
#include <lib/sync/completion.h>
#include <lib/zx/time.h>

#include <optional>
#include <thread>
#include <vector>

#include <ddktl/device.h>
#include <fbl/mutex.h>
#include <usb/request-cpp.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

#include "sdk/lib/driver/outgoing/cpp/outgoing_directory.h"

namespace ftdi_serial {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

constexpr uint16_t kFtdiTypeR = 0x0600;
constexpr uint16_t kFtdiTypeBm = 0x0400;
constexpr uint16_t kFtdiTypeAm = 0x0200;
constexpr uint16_t kFtdiType2232c = 0x0500;
constexpr uint16_t kFtdiType2232h = 0x0700;
constexpr uint16_t kFtdiType4232h = 0x0800;
constexpr uint16_t kFtdiType232h = 0x0900;

// Clock divisors.
constexpr uint32_t kFtdiTypeRDivisor = 16;
constexpr uint32_t kFtdiHClk = 120000000;
constexpr uint32_t kFtdiCClk = 48000000;

// Usb binding rules.
constexpr uint32_t kFtdiUsbVid = 0x0403;
constexpr uint32_t kFtdiUsb232rPid = 0x6001;
constexpr uint32_t kFtdiUsb2232Pid = 0x6010;
constexpr uint32_t kFtdiUsb232hPid = 0x6014;

// Reset the port.
constexpr uint8_t kFtdiSioReset = 0;

// Set the modem control register.
constexpr uint8_t kFtdiSioModemCtrl = 1;

// Set flow control register.
constexpr uint8_t kFtdiSioSetFlowCtrl = 2;

// Set baud rate.
constexpr uint8_t kFtdiSioSetBaudrate = 3;

// Set the data characteristics of the port.
constexpr uint8_t kFtdiSioSetData = 4;

// Set the bitmode.
constexpr uint8_t kFtdiSioSetBitmode = 0x0B;

// Requests.
constexpr uint8_t kFtdiSioResetRequest = kFtdiSioReset;
constexpr uint8_t kFtdiSioSetBaudrateRequest = kFtdiSioSetBaudrate;
constexpr uint8_t kFtdiSioSetDataRequest = kFtdiSioSetData;
constexpr uint8_t kFtdiSioSetFlowCtrlRequest = kFtdiSioSetFlowCtrl;
constexpr uint8_t kFtdiSioSetModemCtrlRequest = kFtdiSioModemCtrl;
constexpr uint8_t kFtdiSioPollModemStatusRequest = 0x05;
constexpr uint8_t kFtdiSioSetEventCharRequest = 0x06;
constexpr uint8_t kFtdiSioSetErrorCharRequest = 0x07;
constexpr uint8_t kFtdiSioSetLatencyTimerRequest = 0x09;
constexpr uint8_t kFtdiSioGetLatencyTimerRequest = 0x0A;
constexpr uint8_t kFtdiSioSetBitmodeRequest = 0x0B;
constexpr uint8_t kFtdiSioReadPinsRequest = 0x0C;
constexpr uint8_t kFtdiSioReadEepromRequest = 0x90;
constexpr uint8_t kFtdiSioWriteEepromRequest = 0x91;
constexpr uint8_t kFtdiSioEraseEepromRequest = 0x92;

class FtdiSerial {
 public:
  // Synchronously read len bytes into buf.
  virtual zx_status_t Read(uint8_t* buf, size_t len) = 0;

  // Synchronously write len bytes from buf.
  virtual zx_status_t Write(uint8_t* buf, size_t len) = 0;
};

class FtdiDevice;
using DeviceType = ddk::Device<FtdiDevice, ddk::Unbindable,
                               ddk::Messageable<fuchsia_hardware_ftdi::Device>::Mixin>;
class FtdiDevice : public DeviceType,
                   public fdf::WireServer<fuchsia_hardware_serialimpl::Device>,
                   public FtdiSerial {
 public:
  explicit FtdiDevice(zx_device_t* parent)
      : DeviceType(parent), usb_client_(parent), outgoing_(fdf::Dispatcher::GetCurrent()->get()) {}
  ~FtdiDevice();

  zx_status_t Bind();

  void DdkUnbind(ddk::UnbindTxn txn);

  void DdkRelease();

  // |FtdiSerial::Read|
  zx_status_t Read(uint8_t* buf, size_t len) override;

  // |FtdiSerial::Write|
  zx_status_t Write(uint8_t* buf, size_t len) override;

 private:
  static constexpr zx::duration kSerialReadWriteTimeout = zx::sec(1);

  struct WriteContext {
    WriteContext(WriteCompleter::Async completer, cpp20::span<const uint8_t> data,
                 size_t pending_requests)
        : completer(std::move(completer)), data(data), pending_requests(pending_requests) {}

    void Complete(fdf::Arena& arena) {
      if (cancel_all_completer) {
        cancel_all_completer->buffer(arena).Reply();
      }
      completer.buffer(arena).Reply(zx::make_result(status));
    }

    WriteCompleter::Async completer;
    cpp20::span<const uint8_t> data;
    size_t pending_requests;
    zx_status_t status = ZX_OK;
    std::optional<CancelAllCompleter::Async> cancel_all_completer;
  };

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::GetInfo|
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::Config|
  void Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override;

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::Enable|
  void Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override;

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::Read|
  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override;

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::Write|
  void Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request, fdf::Arena& arena,
             WriteCompleter::Sync& completer) override;

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::CancelAll|
  void CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) override;

  // |fdf::WireServer<fuchsia_hardware_serialimpl::Device>::handle_unknown_method|
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void CreateI2C(CreateI2CRequestView request, CreateI2CCompleter::Sync& _completer) override;

  static zx_status_t FidlCreateI2c(void* ctx,
                                   const fuchsia_hardware_ftdi::wire::I2cBusLayout* layout,
                                   const fuchsia_hardware_ftdi::wire::I2cDevice* device);
  zx_status_t Reset();
  zx_status_t SetBaudrate(uint32_t baudrate);
  zx_status_t CalcDividers(uint32_t* baudrate, uint32_t clock, uint32_t divisor,
                           uint16_t* integer_div, uint16_t* fraction_div);
  void WriteComplete(usb_request_t* request);
  void ReadComplete(usb_request_t* request);
  // Copies starting at request_offset bytes into request, and returns the number of bytes copied.
  static size_t CopyFromRequest(usb::Request<>& request, size_t request_offset,
                                cpp20::span<uint8_t> buffer);
  // The FIDL Read method reads as many bytes as possible. This helper reads at most len bytes.
  size_t ReadAtMost(uint8_t* buffer, size_t len) __TA_REQUIRES(mutex_);
  // Writes as much data as possible with a single USB request, and returns a subspan pointing to
  // the remaining data that did not fit into the request.
  cpp20::span<const uint8_t> QueueWriteRequest(cpp20::span<const uint8_t> data, usb::Request<> req)
      __TA_REQUIRES(mutex_);
  void CancelAll(std::optional<CancelAllCompleter::Async> completer = std::nullopt);

  zx_status_t SetBitMode(uint8_t line_mask, uint8_t mode);

  ddk::UsbProtocolClient usb_client_ = {};

  uint16_t ftditype_ = 0;
  uint32_t baudrate_ = 0;

  size_t read_offset_ = 0;

  size_t parent_req_size_ = 0;
  uint8_t bulk_in_addr_ = 0;
  uint8_t bulk_out_addr_ = 0;

  fbl::Mutex mutex_ = {};

  // pool of free USB requests
  usb::RequestQueue<> free_read_queue_ __TA_GUARDED(mutex_);
  usb::RequestQueue<> free_write_queue_ __TA_GUARDED(mutex_);
  // list of received packets not yet read by upper layer
  usb::RequestQueue<> completed_reads_queue_ __TA_GUARDED(mutex_);

  std::optional<ReadCompleter::Async> read_completer_ __TA_GUARDED(mutex_);

  std::vector<uint8_t> write_buffer_ __TA_GUARDED(mutex_);
  std::optional<WriteContext> write_context_ __TA_GUARDED(mutex_);

  fuchsia_hardware_serial::wire::SerialPortInfo serial_port_info_ __TA_GUARDED(mutex_);
  bool need_to_notify_cb_ = false;
  std::thread cancel_thread_;

  sync_completion_t serial_readable_;

  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> bindings_;
  // Keep a client so that we can make synchronous Write() calls on ourself when requested by MPSSE.
  fdf::WireSyncClient<fuchsia_hardware_serialimpl::Device> device_client_;
  fdf::OutgoingDirectory outgoing_;
};

}  // namespace ftdi_serial

#endif  // SRC_DEVICES_SERIAL_DRIVERS_FTDI_FTDI_H_
