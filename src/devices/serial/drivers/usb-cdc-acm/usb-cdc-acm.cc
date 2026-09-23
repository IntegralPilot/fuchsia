// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-cdc-acm.h"

#include <assert.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>

#include <fbl/alloc_checker.h>
#include <usb/cdc.h>
#include <usb/request-cpp.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

namespace usb_cdc_acm_serial {

namespace {

constexpr int32_t kReadRequestCount = 8;
constexpr int32_t kWriteRequestCount = 8;

constexpr uint32_t kDefaultBaudRate = 115200;
constexpr uint32_t kDefaultConfig = fuchsia_hardware_serialimpl::wire::kSerialDataBits8 |
                                    fuchsia_hardware_serialimpl::wire::kSerialStopBits1 |
                                    fuchsia_hardware_serialimpl::wire::kSerialParityNone;

constexpr uint32_t kUsbBufferSize = 2048;

constexpr uint32_t kUsbCdcAcmSetLineCoding = 0x20;
constexpr uint32_t kUsbCdcAcmGetLineCoding = 0x21;

}  // namespace

struct usb_cdc_acm_line_coding_t {
  uint32_t dwDTERate;
  uint8_t bCharFormat;
  uint8_t bParityType;
  uint8_t bDataBits;
} __PACKED;

void UsbCdcAcmDevice::DdkUnbind(ddk::UnbindTxn txn) {
  CancelAll();

  cancel_thread_ = std::thread([this, unbind_txn = std::move(txn)]() mutable {
    usb_client_.CancelAll(bulk_in_addr_);
    usb_client_.CancelAll(bulk_out_addr_);
    unbind_txn.Reply();
  });
}

void UsbCdcAcmDevice::DdkRelease() {
  cancel_thread_.join();
  delete this;
}

void UsbCdcAcmDevice::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  completer.buffer(arena).ReplySuccess(serial_port_info_);
}

void UsbCdcAcmDevice::Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request,
                             fdf::Arena& arena, ConfigCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;
  if (baud_rate_ != request->baud_rate || request->flags != config_flags_) {
    status = ConfigureDevice(request->baud_rate, request->flags);
  }
  completer.buffer(arena).Reply(zx::make_result(status));
}

void UsbCdcAcmDevice::Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request,
                             fdf::Arena& arena, EnableCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess();
}

size_t UsbCdcAcmDevice::CopyFromRequest(usb::Request<>& request, size_t request_offset,
                                        cpp20::span<uint8_t> buffer) {
  ZX_ASSERT(request.request()->response.actual >= request_offset);
  const size_t to_copy =
      std::min(request.request()->response.actual - request_offset, buffer.size());

  const size_t result = request.CopyFrom(buffer.data(), to_copy, request_offset);
  ZX_ASSERT(result == to_copy);
  return to_copy;
}

void UsbCdcAcmDevice::Read(fdf::Arena& arena, ReadCompleter::Sync& completer) {
  // This was the maximum size for reads from the serial core driver at the time of our conversion
  // from Banjo to FIDL.
  uint8_t buffer[fuchsia_io::wire::kMaxBuf];
  size_t bytes_copied = 0;
  size_t offset = read_offset_;
  zx_status_t status = ZX_OK;

  fbl::AutoLock lock(&lock_);

  // Per the serialimpl protocol, ZX_ERR_ALREADY_BOUND should be returned if the client makes a
  // read request when one was already in progress.
  if (read_completer_) {
    status = ZX_ERR_ALREADY_BOUND;
  } else {
    while (bytes_copied < std::size(buffer)) {
      std::optional<usb::Request<>> req = completed_reads_queue_.pop();
      if (!req) {
        break;
      }

      size_t to_copy =
          CopyFromRequest(*req, offset, {&buffer[bytes_copied], std::size(buffer) - bytes_copied});
      bytes_copied += to_copy;

      // If we aren't reading the whole request, put it back on the front of the completed queue and
      // mark the offset into it for the next read.
      if ((to_copy + offset) < req->request()->response.actual) {
        offset = offset + to_copy;
        completed_reads_queue_.push_next(*std::move(req));
        break;
      }

      usb_client_.RequestQueue(req->take(), &read_request_complete_);
      offset = 0;
    }

    // Store the offset into the current request for the next read.
    read_offset_ = offset;

    if (bytes_copied == 0) {
      read_completer_.emplace(completer.ToAsync());
      return;
    }
  }

  lock.release();

  if (status == ZX_OK) {
    completer.buffer(arena).ReplySuccess(
        fidl::VectorView<uint8_t>::FromExternal(buffer, bytes_copied));
  } else {
    completer.buffer(arena).ReplyError(status);
  }
}

void UsbCdcAcmDevice::Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request,
                            fdf::Arena& arena, WriteCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;

  {
    fbl::AutoLock lock(&lock_);

    if (write_context_) {
      // Per the serialimpl protocol, ZX_ERR_ALREADY_BOUND should be returned if the client makes a
      // write request when one was already in progress.
      status = ZX_ERR_ALREADY_BOUND;
    } else if (request->data.size() > 0) {
      cpp20::span<const uint8_t> data = request->data.get();
      size_t pending_write_requests = 0;
      while (!data.empty()) {
        if (std::optional<usb::Request<>> req = free_write_queue_.pop(); req) {
          data = QueueWriteRequest(data, *std::move(req));
          pending_write_requests++;
        } else {
          break;
        }
      }

      // Copy the remaining write data to the vector, resizing if necessary.
      write_buffer_.clear();
      write_buffer_.insert(write_buffer_.begin(), data.begin(), data.end());
      write_context_.emplace(completer.ToAsync(), write_buffer_, pending_write_requests);
      return;
    }
  }

  completer.buffer(arena).Reply(zx::make_result(status));
}

void UsbCdcAcmDevice::CancelAll(std::optional<CancelAllCompleter::Async> completer) {
  std::optional<ReadCompleter::Async> read_completer;
  std::optional<WriteContext> write_context;

  {
    fbl::AutoLock lock(&lock_);

    // Clients should not call CancelAll() when a previous request is still pending.
    if (completer && write_context_ && write_context_->cancel_all_completer) {
      completer->Close(ZX_ERR_BAD_STATE);
      return;
    }

    read_completer_.swap(read_completer);

    if (!completer) {
      // If completer is not set, we are unbinding and need to abort the write immediately.
      write_context_.swap(write_context);
    } else if (write_context_) {
      // Otherwise complete the CancelAll request after all outstanding writes have completed.
      write_context_->cancel_all_completer.swap(completer);
    }
  }

  fdf::Arena arena('FTDI');

  if (read_completer) {
    read_completer->buffer(arena).ReplyError(ZX_ERR_CANCELED);
  }

  if (write_context) {
    write_context->status = ZX_ERR_CANCELED;
    write_context->Complete(arena);
  }

  if (completer) {
    completer->buffer(arena).Reply();
  }
}

void UsbCdcAcmDevice::CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) {
  usb_client_.CancelAll(bulk_in_addr_);
  usb_client_.CancelAll(bulk_out_addr_);
  CancelAll();
  completer.buffer(arena).Reply();
}

void UsbCdcAcmDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "Unknown method ordinal %lu", metadata.method_ordinal);
}

void UsbCdcAcmDevice::ReadComplete(usb_request_t* request) {
  usb::Request<> req(request, parent_req_size_);
  if (req.request()->response.status == ZX_ERR_IO_NOT_PRESENT) {
    zxlogf(INFO, "usb-cdc-acm: remote closed");
    return;
  }

  uint8_t buffer[kUsbBufferSize];
  fuchsia_hardware_serialimpl::wire::DeviceReadResponse response;
  std::optional<ReadCompleter::Async> read_completer;

  zx_status_t status = req.request()->response.status;

  {
    fbl::AutoLock lock(&lock_);

    if (read_completer_ && status == ZX_OK) {
      // The USB request succeeded and there is a pending read. Copy the serial data out and use it
      // to populate the response.
      ZX_ASSERT(request->response.actual <= std::size(buffer));
      size_t actual = CopyFromRequest(req, 0, {buffer, std::size(buffer)});
      response.data = fidl::VectorView<uint8_t>::FromExternal(buffer, actual);
    }

    if (!read_completer_ && status == ZX_OK) {
      // The USB request succeeded and there is no pending read. Add the request to the queue to be
      // read by the client.
      completed_reads_queue_.push(std::move(req));
    } else {
      // The USB request did not succeed, or there is a pending read that will consume the entire
      // buffer. Re-queue the request to receive the next batch of serial data.
      usb_client_.RequestQueue(req.take(), &read_request_complete_);
    }

    // Swap out the completer so that we can reply after releasing the lock.
    read_completer_.swap(read_completer);
  }

  if (read_completer) {
    fdf::Arena arena('FTDI');
    read_completer->buffer(arena).Reply(zx::make_result(status, &response));
  }
}

void UsbCdcAcmDevice::WriteComplete(usb_request_t* request) {
  usb::Request<> req(request, parent_req_size_);
  if (req.request()->response.status == ZX_ERR_IO_NOT_PRESENT) {
    zxlogf(INFO, "usb-cdc-acm: remote closed");
    return;
  }

  std::optional<WriteContext> write_context;

  {
    fbl::AutoLock lock(&lock_);

    if (!write_context_) {
      // This should only happen while we're unbinding.
      free_write_queue_.push(std::move(req));
      return;
    }

    ZX_ASSERT(write_context_->pending_requests > 0);

    if (req.request()->response.status != ZX_OK && write_context_->status == ZX_OK) {
      write_context_->data = {};
      write_context_->status = req.request()->response.status;
      usb_client_.CancelAll(bulk_out_addr_);
    }

    if (!write_context_->data.empty()) {
      // There is more data to write, refill the request and queue it up to be sent.
      write_context_->data = QueueWriteRequest(write_context->data, std::move(req));
      return;
    }

    free_write_queue_.push(std::move(req));
    write_context_->pending_requests--;
    if (write_context_->pending_requests == 0) {
      // All requests have been completed, possibly with errors. Swap out the completer so that we
      // can reply after releasing the lock.
      write_context_.swap(write_context);
    }
  }

  if (write_context) {
    fdf::Arena arena('FTDI');
    write_context->Complete(arena);
  }
}

cpp20::span<const uint8_t> UsbCdcAcmDevice::QueueWriteRequest(cpp20::span<const uint8_t> data,
                                                              usb::Request<> req) {
  const ssize_t actual = req.CopyTo(data.data(), data.size(), 0);
  req.request()->header.length = data.size();

  usb_client_.RequestQueue(req.take(), &write_request_complete_);

  return data.subspan(actual);
}

zx_status_t UsbCdcAcmDevice::ConfigureDevice(uint32_t baud_rate, uint32_t flags) {
  if (!usb_client_.is_valid()) {
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t status = ZX_OK;

  usb_cdc_acm_line_coding_t coding;
  const bool baud_rate_only = flags & fuchsia_hardware_serialimpl::wire::kSerialSetBaudRateOnly;
  if (baud_rate_only) {
    size_t coding_length;
    status = usb_client_.ControlIn(
        USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE, kUsbCdcAcmGetLineCoding, 0, 0,
        ZX_TIME_INFINITE, reinterpret_cast<uint8_t*>(&coding), sizeof(coding), &coding_length);
    if (coding_length != sizeof(coding)) {
      zxlogf(TRACE, "usb-cdc-acm: failed to fetch line coding");
    }
    if (status != ZX_OK) {
      return status;
    }
  } else {
    switch (flags & fuchsia_hardware_serialimpl::wire::kSerialStopBitsMask) {
      case fuchsia_hardware_serialimpl::wire::kSerialStopBits1:
        coding.bCharFormat = 0;
        break;
      case fuchsia_hardware_serialimpl::wire::kSerialStopBits2:
        coding.bCharFormat = 2;
        break;
      default:
        return ZX_ERR_INVALID_ARGS;
    }
    switch (flags & fuchsia_hardware_serialimpl::wire::kSerialParityMask) {
      case fuchsia_hardware_serialimpl::wire::kSerialParityNone:
        coding.bParityType = 0;
        break;
      case fuchsia_hardware_serialimpl::wire::kSerialParityEven:
        coding.bParityType = 2;
        break;
      case fuchsia_hardware_serialimpl::wire::kSerialParityOdd:
        coding.bParityType = 1;
        break;
      default:
        return ZX_ERR_INVALID_ARGS;
    }
    switch (flags & fuchsia_hardware_serialimpl::wire::kSerialDataBitsMask) {
      case fuchsia_hardware_serialimpl::wire::kSerialDataBits5:
        coding.bDataBits = 5;
        break;
      case fuchsia_hardware_serialimpl::wire::kSerialDataBits6:
        coding.bDataBits = 6;
        break;
      case fuchsia_hardware_serialimpl::wire::kSerialDataBits7:
        coding.bDataBits = 7;
        break;
      case fuchsia_hardware_serialimpl::wire::kSerialDataBits8:
        coding.bDataBits = 8;
        break;
      default:
        return ZX_ERR_INVALID_ARGS;
    }
  }

  coding.dwDTERate = baud_rate;

  status = usb_client_.ControlOut(USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE,
                                  kUsbCdcAcmSetLineCoding, 0, 0, ZX_TIME_INFINITE,
                                  reinterpret_cast<uint8_t*>(&coding), sizeof(coding));

  if (status == ZX_OK) {
    baud_rate_ = baud_rate;
    if (!baud_rate_only) {
      config_flags_ = flags;
    }
  }
  return status;
}

zx_status_t UsbCdcAcmDevice::Bind() {
  zx_status_t status = ZX_OK;

  if (!usb_client_.is_valid()) {
    return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
  }

  // Enumerate available interfaces and find bulk-in and bulk-out endpoints.
  std::optional<usb::InterfaceList> usb_interface_list;
  status = usb::InterfaceList::Create(usb_client_, true, &usb_interface_list);
  if (status != ZX_OK) {
    return status;
  }

  fbl::AutoLock lock(&lock_);
  uint8_t bulk_in_address = 0;
  uint8_t bulk_out_address = 0;

  for (auto interface : *usb_interface_list) {
    if (interface.descriptor()->b_num_endpoints > 1) {
      for (auto& endpoint : interface.GetEndpointList()) {
        if (usb_ep_type(endpoint.descriptor()) == fdescriptor::EndpointType::kBulk) {
          if (usb_ep_direction(endpoint.descriptor()) == USB_ENDPOINT_IN) {
            bulk_in_address = endpoint.descriptor()->b_endpoint_address;
          } else if (usb_ep_direction(endpoint.descriptor()) == USB_ENDPOINT_OUT) {
            bulk_out_address = endpoint.descriptor()->b_endpoint_address;
          }
        }
      }
    }
  }

  if (!bulk_in_address || !bulk_out_address) {
    zxlogf(ERROR, "usb-cdc-acm: Bind() could not find bulk-in and bulk-out endpoints");
    return ZX_ERR_NOT_SUPPORTED;
  }

  bulk_in_addr_ = bulk_in_address;
  bulk_out_addr_ = bulk_out_address;
  parent_req_size_ = usb_client_.GetRequestSize();

  status = ConfigureDevice(kDefaultBaudRate, kDefaultConfig);
  if (status != ZX_OK) {
    zxlogf(ERROR, "usb-cdc-acm: failed to set default baud rate: %d", status);
    return status;
  }

  serial_port_info_.serial_class = fuchsia_hardware_serial::Class::kGeneric;

  {
    fuchsia_hardware_serialimpl::Service::InstanceHandler handler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                          fidl::kIgnoreBindingClosure),
    });
    auto result = outgoing_.AddService<fuchsia_hardware_serialimpl::Service>(std::move(handler));
    if (result.is_error()) {
      zxlogf(ERROR, "AddService failed: %s", result.status_string());
      return result.error_value();
    }
  }

  auto [directory_client, directory_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  {
    auto result = outgoing_.Serve(std::move(directory_server));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to serve the outgoing directory: %s", result.status_string());
      return result.error_value();
    }
  }

  std::array<const char*, 1> fidl_service_offers{fuchsia_hardware_serialimpl::Service::Name};
  status = DdkAdd(ddk::DeviceAddArgs("usb-cdc-acm")
                      .set_outgoing_dir(directory_client.TakeChannel())
                      .set_runtime_service_offers(fidl_service_offers));
  if (status != ZX_OK) {
    zxlogf(ERROR, "usb-cdc-acm: failed to create device: %d", status);
    return status;
  }

  // Create and immediately queue read requests after successfully adding the device.
  for (int i = 0; i < kReadRequestCount; i++) {
    std::optional<usb::Request<>> request;
    status = usb::Request<>::Alloc(&request, kUsbBufferSize, bulk_in_addr_, parent_req_size_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "usb-cdc-acm: allocating reads failed %d", status);
      return status;
    }
    usb_client_.RequestQueue(request->take(), &read_request_complete_);
  }

  for (int i = 0; i < kWriteRequestCount; i++) {
    std::optional<usb::Request<>> request;
    status = usb::Request<>::Alloc(&request, kUsbBufferSize, bulk_out_addr_, parent_req_size_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "usb-cdc-acm: allocating writes failed %d", status);
      return status;
    }
    free_write_queue_.push(*std::move(request));
  }

  return ZX_OK;
}

}  // namespace usb_cdc_acm_serial

namespace {

zx_status_t cdc_acm_bind(void* /*ctx*/, zx_device_t* device) {
  fbl::AllocChecker ac;
  auto dev = fbl::make_unique_checked<usb_cdc_acm_serial::UsbCdcAcmDevice>(&ac, device);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  auto status = dev->Bind();
  if (status != ZX_OK) {
    zxlogf(INFO, "usb-cdc-acm: failed to add serial driver %d", status);
  }

  // Devmgr is now in charge of the memory for dev.
  [[maybe_unused]] auto ptr = dev.release();
  return status;
}

constexpr zx_driver_ops_t cdc_acm_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = cdc_acm_bind;
  return ops;
}();

}  // namespace

// clang-format off
ZIRCON_DRIVER(cdc_acm, cdc_acm_driver_ops, "zircon", "0.1");
// clang-form   at on
