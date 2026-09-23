// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SERIAL_DRIVERS_USB_CDC_ACM_USB_CDC_ACM_H_
#define SRC_DEVICES_SERIAL_DRIVERS_USB_CDC_ACM_USB_CDC_ACM_H_

#include <fidl/fuchsia.hardware.serialimpl/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/stdcompat/span.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/types.h>

#include <thread>
#include <vector>

#include <ddktl/device.h>
#include <fbl/auto_lock.h>
#include <usb/request-cpp.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

#include "sdk/lib/driver/outgoing/cpp/outgoing_directory.h"

namespace usb_cdc_acm_serial {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

class UsbCdcAcmDevice;
using DeviceType = ddk::Device<UsbCdcAcmDevice, ddk::Unbindable>;
class UsbCdcAcmDevice : public DeviceType,
                        public fdf::WireServer<fuchsia_hardware_serialimpl::Device> {
 public:
  explicit UsbCdcAcmDevice(zx_device_t* parent) : DeviceType(parent), usb_client_(parent) {}
  ~UsbCdcAcmDevice() = default;

  zx_status_t Bind();

  // |ddk::Device| mix-in implementations.
  void DdkRelease();
  void DdkUnbind(ddk::UnbindTxn txn);

 private:
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

  // fdf::WireServer<fuchsia_hardware_serialimpl::Device> implementations.
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;
  void Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request, fdf::Arena& arena,
              ConfigCompleter::Sync& completer) override;
  void Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override;
  void Read(fdf::Arena& arena, ReadCompleter::Sync& completer) override;
  void Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request, fdf::Arena& arena,
             WriteCompleter::Sync& completer) override;
  void CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void ReadComplete(usb_request_t* request);
  void WriteComplete(usb_request_t* request);
  static size_t CopyFromRequest(usb::Request<>& request, size_t request_offset,
                                cpp20::span<uint8_t> buffer);
  // Writes as much data as possible with a single USB request, and returns a subspan pointing to
  // the remaining data that did not fit into the request.
  cpp20::span<const uint8_t> QueueWriteRequest(cpp20::span<const uint8_t> data, usb::Request<> req);
  zx_status_t ConfigureDevice(uint32_t baud_rate, uint32_t flags);
  void CancelAll(std::optional<CancelAllCompleter::Async> completer = std::nullopt);

  fbl::Mutex lock_;

  // USB connection, endpoints address, request size, and current configuration.
  ddk::UsbProtocolClient usb_client_ = {};
  uint8_t bulk_in_addr_ = 0;
  uint8_t bulk_out_addr_ = 0;
  size_t parent_req_size_ = 0;
  uint32_t baud_rate_ = 0;
  uint32_t config_flags_ = 0;

  // Queues of free USB write requests and completed reads not yet read by the upper layer.
  usb::RequestQueue<> free_write_queue_ __TA_GUARDED(lock_);
  usb::RequestQueue<> completed_reads_queue_ __TA_GUARDED(lock_);

  // Current offset into the first completed read request.
  size_t read_offset_ = 0;

  // SerialImpl port info and callback.
  fuchsia_hardware_serial::wire::SerialPortInfo serial_port_info_ __TA_GUARDED(lock_);

  // Thread to cancel requests if the device is unbound.
  std::thread cancel_thread_;

  // USB callback functions.
  usb_request_complete_callback_t read_request_complete_ = {
      .callback =
          [](void* ctx, usb_request_t* request) {
            reinterpret_cast<UsbCdcAcmDevice*>(ctx)->ReadComplete(request);
          },
      .ctx = this,
  };
  usb_request_complete_callback_t write_request_complete_ = {
      .callback =
          [](void* ctx, usb_request_t* request) {
            reinterpret_cast<UsbCdcAcmDevice*>(ctx)->WriteComplete(request);
          },
      .ctx = this,
  };

  std::optional<ReadCompleter::Async> read_completer_ __TA_GUARDED(lock_);

  std::vector<uint8_t> write_buffer_ __TA_GUARDED(lock_);
  std::optional<WriteContext> write_context_ __TA_GUARDED(lock_);

  fdf::ServerBindingGroup<fuchsia_hardware_serialimpl::Device> bindings_;
  fdf::OutgoingDirectory outgoing_;
};

}  // namespace usb_cdc_acm_serial

#endif  // SRC_DEVICES_SERIAL_DRIVERS_USB_CDC_ACM_USB_CDC_ACM_H_
