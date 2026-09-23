// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ftdi.h"

#include <fidl/fuchsia.hardware.serial/cpp/wire.h>
#include <fuchsia/hardware/usb/c/banjo.h>
#include <inttypes.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/listnode.h>

#include <ddktl/device.h>
#include <ddktl/fidl.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

#include "ftdi-i2c.h"

#define FTDI_STATUS_SIZE 2
#define FTDI_RX_HEADER_SIZE 4

#define READ_REQ_COUNT 8
#define WRITE_REQ_COUNT 4
#define INTR_REQ_COUNT 4
#define USB_BUF_SIZE 2048
#define INTR_REQ_SIZE 4

#define FIFOSIZE 256
#define FIFOMASK (FIFOSIZE - 1)

namespace {

static zx_status_t FtdiBindFail(zx_status_t status) {
  zxlogf(ERROR, "ftdi_bind failed: %d", status);
  return status;
}

}  // namespace

namespace ftdi_serial {

void FtdiDevice::ReadComplete(usb_request_t* request) {
  usb::Request<> req(request, parent_req_size_);
  if (req.request()->response.status == ZX_ERR_IO_NOT_PRESENT) {
    zxlogf(INFO, "FTDI: remote closed");
    return;
  }

  uint8_t buffer[USB_BUF_SIZE];
  fuchsia_hardware_serialimpl::wire::DeviceReadResponse response;
  std::optional<ReadCompleter::Async> read_completer;
  bool signal_readable = false;

  zx_status_t status = req.request()->response.status;
  // Check that we read at least the FTDI status bytes.
  if (status == ZX_OK && req.request()->response.actual < FTDI_STATUS_SIZE) {
    status = ZX_ERR_IO_INVALID;
  }

  {
    fbl::AutoLock lock(&mutex_);

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
      signal_readable = true;
    } else {
      // The USB request did not succeed, or there is a pending read that will consume the entire
      // buffer. Re-queue the request to receive the next batch of serial data.
      usb_request_complete_callback_t complete = {
          .callback =
              [](void* ctx, usb_request_t* request) {
                static_cast<FtdiDevice*>(ctx)->ReadComplete(request);
              },
          .ctx = this,
      };
      usb_client_.RequestQueue(req.take(), &complete);
    }

    // Swap out the completer so that we can reply after releasing the lock.
    read_completer_.swap(read_completer);
  }

  if (read_completer) {
    fdf::Arena arena('FTDI');
    read_completer->buffer(arena).Reply(zx::make_result(status, &response));
  }

  if (signal_readable) {
    sync_completion_signal(&serial_readable_);
  }
}

void FtdiDevice::WriteComplete(usb_request_t* request) {
  usb::Request<> req(request, parent_req_size_);
  if (req.request()->response.status == ZX_ERR_IO_NOT_PRESENT) {
    return;
  }

  std::optional<WriteContext> write_context;

  {
    fbl::AutoLock lock(&mutex_);

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

zx_status_t FtdiDevice::CalcDividers(uint32_t* baudrate, uint32_t clock, uint32_t divisor,
                                     uint16_t* integer_div, uint16_t* fraction_div) {
  static constexpr uint8_t kFractionLookup[8] = {0, 3, 2, 4, 1, 5, 6, 7};

  uint32_t base_clock = clock / divisor;

  // Integer dividers of 1 and 0 are special cases.
  // 0 = base_clock and 1 = 2/3 of base clock.
  if (*baudrate >= base_clock) {
    // Return with max baud rate achievable.
    *fraction_div = 0;
    *integer_div = 0;
    *baudrate = base_clock;
  } else if (*baudrate >= (base_clock * 2) / 3) {
    *integer_div = 1;
    *fraction_div = 0;
    *baudrate = (base_clock * 2) / 3;
  } else {
    // Create a 28.4 fractional integer.
    uint32_t ratio = (base_clock * 16) / *baudrate;

    // Round up if needed.
    ratio++;
    ratio = ratio & 0xfffffffe;

    *baudrate = (base_clock << 4) / ratio;
    *integer_div = static_cast<uint16_t>(ratio >> 4);
    *fraction_div = kFractionLookup[(ratio >> 1) & 0x07];
  }
  return ZX_OK;
}

cpp20::span<const uint8_t> FtdiDevice::QueueWriteRequest(cpp20::span<const uint8_t> data,
                                                         usb::Request<> req) {
  const ssize_t actual = req.CopyTo(data.data(), data.size(), 0);
  req.request()->header.length = data.size();

  usb_request_complete_callback_t complete = {
      .callback =
          [](void* ctx, usb_request_t* request) {
            static_cast<FtdiDevice*>(ctx)->WriteComplete(request);
          },
      .ctx = this,
  };
  usb_client_.RequestQueue(req.take(), &complete);

  return data.subspan(actual);
}

void FtdiDevice::Write(fuchsia_hardware_serialimpl::wire::DeviceWriteRequest* request,
                       fdf::Arena& arena, WriteCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;

  {
    fbl::AutoLock lock(&mutex_);

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

size_t FtdiDevice::CopyFromRequest(usb::Request<>& request, size_t request_offset,
                                   cpp20::span<uint8_t> buffer) {
  ZX_ASSERT(request.request()->response.actual >= request_offset + FTDI_STATUS_SIZE);
  const size_t to_copy = std::min(
      request.request()->response.actual - request_offset - FTDI_STATUS_SIZE, buffer.size());

  const size_t result = request.CopyFrom(buffer.data(), to_copy, request_offset + FTDI_STATUS_SIZE);
  ZX_ASSERT(result == to_copy);
  return to_copy;
}

size_t FtdiDevice::ReadAtMost(uint8_t* buffer, size_t len) {
  size_t bytes_copied = 0;
  size_t offset = read_offset_;

  usb_request_complete_callback_t complete = {
      .callback =
          [](void* ctx, usb_request_t* request) {
            static_cast<FtdiDevice*>(ctx)->ReadComplete(request);
          },
      .ctx = this,
  };

  while (bytes_copied < len) {
    std::optional<usb::Request<>> req = completed_reads_queue_.pop();

    if (!req) {
      sync_completion_reset(&serial_readable_);
      break;
    }

    size_t to_copy = CopyFromRequest(*req, offset, {&buffer[bytes_copied], len - bytes_copied});
    bytes_copied = bytes_copied + to_copy;

    // If we aren't reading the whole request then put it in the front of the queue
    // and return.
    if ((to_copy + offset + FTDI_STATUS_SIZE) < req->request()->response.actual) {
      offset = offset + to_copy;
      completed_reads_queue_.push_next(*std::move(req));
      break;
    }

    // Requeue the read request.
    usb_client_.RequestQueue(req->take(), &complete);
    offset = 0;
  }

  read_offset_ = offset;
  return bytes_copied;
}

void FtdiDevice::Read(fdf::Arena& arena, ReadCompleter::Sync& completer) {
  // This was the maximum size for reads from the serial core driver at the time of our conversion
  // from Banjo to FIDL.
  uint8_t buffer[fuchsia_io::wire::kMaxBuf];
  fuchsia_hardware_serialimpl::wire::DeviceReadResponse response;
  zx_status_t status = ZX_OK;

  {
    fbl::AutoLock lock(&mutex_);

    // Per the serialimpl protocol, ZX_ERR_ALREADY_BOUNDD should be returned if the client makes a
    // read request when one was already in progress.
    if (read_completer_) {
      status = ZX_ERR_ALREADY_BOUND;
    } else {
      size_t actual = ReadAtMost(buffer, std::size(buffer));
      if (actual == 0) {
        // No data is available to read, respond to this request asynchronously.
        read_completer_.emplace(completer.ToAsync());
        return;
      }
      response.data = fidl::VectorView<uint8_t>::FromExternal(buffer, actual);
    }
  }

  completer.buffer(arena).Reply(zx::make_result(status, &response));
}

zx_status_t FtdiDevice::SetBaudrate(uint32_t baudrate) {
  uint16_t whole, fraction, value, index;
  zx_status_t status;

  switch (ftditype_) {
    case kFtdiTypeR:
    case kFtdiType2232c:
    case kFtdiTypeBm:
      CalcDividers(&baudrate, kFtdiCClk, 16, &whole, &fraction);
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }
  value = static_cast<uint16_t>((whole & 0x3fff) | (fraction << 14));
  index = static_cast<uint16_t>(fraction >> 2);
  status = usb_client_.ControlOut(USB_DIR_OUT | USB_TYPE_VENDOR | USB_RECIP_DEVICE,
                                  kFtdiSioSetBaudrate, value, index, ZX_TIME_INFINITE, NULL, 0);
  if (status == ZX_OK) {
    baudrate_ = baudrate;
  }
  return status;
}

zx_status_t FtdiDevice::Reset() {
  if (!usb_client_.is_valid()) {
    return ZX_ERR_INVALID_ARGS;
  }
  return usb_client_.ControlOut(USB_DIR_OUT | USB_TYPE_VENDOR | USB_RECIP_DEVICE,
                                kFtdiSioResetRequest, kFtdiSioReset, 0, ZX_TIME_INFINITE, NULL, 0);
}

zx_status_t FtdiDevice::SetBitMode(uint8_t line_mask, uint8_t mode) {
  uint16_t val = static_cast<uint16_t>(line_mask | (mode << 8));
  zx_status_t status =
      usb_client_.ControlOut(USB_DIR_OUT | USB_TYPE_VENDOR | USB_RECIP_DEVICE, kFtdiSioSetBitmode,
                             val, 0, ZX_TIME_INFINITE, NULL, 0);
  if (status != ZX_OK) {
    zxlogf(ERROR, "FTDI set bitmode failed with %d", status);
    return status;
  }

  return status;
}

void FtdiDevice::Config(fuchsia_hardware_serialimpl::wire::DeviceConfigRequest* request,
                        fdf::Arena& arena, ConfigCompleter::Sync& completer) {
  if (request->baud_rate != baudrate_) {
    return completer.buffer(arena).Reply(zx::make_result(SetBaudrate(request->baud_rate)));
  }

  return completer.buffer(arena).ReplySuccess();
}

void FtdiDevice::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  fbl::AutoLock lock(&mutex_);
  completer.buffer(arena).ReplySuccess(serial_port_info_);
}

void FtdiDevice::Enable(fuchsia_hardware_serialimpl::wire::DeviceEnableRequest* request,
                        fdf::Arena& arena, EnableCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess();
}

void FtdiDevice::CancelAll(std::optional<CancelAllCompleter::Async> completer) {
  std::optional<ReadCompleter::Async> read_completer;
  std::optional<WriteContext> write_context;

  {
    fbl::AutoLock lock(&mutex_);

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

void FtdiDevice::CancelAll(fdf::Arena& arena, CancelAllCompleter::Sync& completer) {
  usb_client_.CancelAll(bulk_in_addr_);
  usb_client_.CancelAll(bulk_out_addr_);
  CancelAll(completer.ToAsync());
}

void FtdiDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_serialimpl::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  zxlogf(ERROR, "Unknown method ordinal %lu", metadata.method_ordinal);
}

zx_status_t FtdiDevice::Read(uint8_t* buf, size_t len) {
  size_t read_len = 0;
  uint8_t* buf_index = buf;

  while (read_len < len) {
    size_t actual;

    {
      fbl::AutoLock lock(&mutex_);
      actual = ReadAtMost(buf_index, len - read_len);
    }

    if (actual == 0) {
      zx_status_t status = sync_completion_wait_deadline(
          &serial_readable_, zx::deadline_after(kSerialReadWriteTimeout).get());
      if (status != ZX_OK) {
        return status;
      }
    }

    read_len += actual;
    buf_index += actual;
  }
  return ZX_OK;
}

zx_status_t FtdiDevice::Write(uint8_t* buf, size_t len) {
  fdf::Arena arena('FTDI');
  auto result =
      device_client_.buffer(arena)->Write(fidl::VectorView<uint8_t>::FromExternal(buf, len));
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

FtdiDevice::~FtdiDevice() {}

void FtdiDevice::DdkUnbind(ddk::UnbindTxn txn) {
  CancelAll();

  cancel_thread_ = std::thread([this, unbind_txn = std::move(txn)]() mutable {
    usb_client_.CancelAll(bulk_in_addr_);
    usb_client_.CancelAll(bulk_out_addr_);
    unbind_txn.Reply();
  });
}

void FtdiDevice::DdkRelease() {
  cancel_thread_.join();
  delete this;
}

void FtdiDevice::CreateI2C(CreateI2CRequestView request, CreateI2CCompleter::Sync& completer) {
  // Set the chip to run in MPSSE mode.
  zx_status_t status = this->SetBitMode(0, 0);
  if (status != ZX_OK) {
    zxlogf(ERROR, "FTDI: setting bitmode 0 failed");
    return;
  }
  status = this->SetBitMode(0, 2);
  if (status != ZX_OK) {
    zxlogf(ERROR, "FTDI: setting bitmode 2 failed");
    return;
  }

  ftdi_mpsse::FtdiI2c::Create(this->zxdev(), this, &request->layout, &request->device);
}

zx_status_t ftdi_bind_fail(zx_status_t status) {
  zxlogf(ERROR, "ftdi_bind failed: %d", status);
  return status;
}

zx_status_t FtdiDevice::Bind() {
  zx_status_t status = ZX_OK;

  {
    auto [client, server] = fdf::Endpoints<fuchsia_hardware_serialimpl::Device>::Create();
    device_client_.Bind(std::move(client));
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->get(), std::move(server), this,
                         fidl::kIgnoreBindingClosure);
  }

  if (!usb_client_.is_valid()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  fbl::AutoLock lock(&mutex_);

  // Find our endpoints.
  std::optional<usb::InterfaceList> usb_interface_list;
  status = usb::InterfaceList::Create(usb_client_, true, &usb_interface_list);
  if (status != ZX_OK) {
    return status;
  }

  uint8_t bulk_in_addr = 0;
  uint8_t bulk_out_addr = 0;

  for (auto& interface : *usb_interface_list) {
    for (auto ep_itr : interface.GetEndpointList()) {
      if (usb_ep_direction(ep_itr.descriptor()) == USB_ENDPOINT_OUT) {
        if (usb_ep_type(ep_itr.descriptor()) == fdescriptor::EndpointType::kBulk) {
          bulk_out_addr = ep_itr.descriptor()->b_endpoint_address;
        }
      } else {
        if (usb_ep_type(ep_itr.descriptor()) == fdescriptor::EndpointType::kBulk) {
          bulk_in_addr = ep_itr.descriptor()->b_endpoint_address;
        }
      }
    }
  }

  if (!bulk_in_addr || !bulk_out_addr) {
    zxlogf(ERROR, "FTDI: could not find all endpoints");
    return ZX_ERR_NOT_SUPPORTED;
  }

  ftditype_ = kFtdiTypeR;

  parent_req_size_ = usb_client_.GetRequestSize();
  for (int i = 0; i < READ_REQ_COUNT; i++) {
    std::optional<usb::Request<>> req;
    status = usb::Request<>::Alloc(&req, USB_BUF_SIZE, bulk_in_addr, parent_req_size_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "FTDI allocating reads failed %d", status);
      return FtdiBindFail(status);
    }
    free_read_queue_.push(*std::move(req));
  }
  for (int i = 0; i < WRITE_REQ_COUNT; i++) {
    std::optional<usb::Request<>> req;
    status = usb::Request<>::Alloc(&req, USB_BUF_SIZE, bulk_out_addr, parent_req_size_);
    if (status != ZX_OK) {
      zxlogf(ERROR, "FTDI allocating writes failed %d", status);
      return FtdiBindFail(status);
    }
    free_write_queue_.push(*std::move(req));
  }

  status = Reset();
  if (status != ZX_OK) {
    zxlogf(ERROR, "FTDI reset failed %d", status);
    return FtdiBindFail(status);
  }

  status = SetBaudrate(115200);
  if (status != ZX_OK) {
    zxlogf(ERROR, "FTDI: set baudrate failed");
    return FtdiBindFail(status);
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
  status = DdkAdd(ddk::DeviceAddArgs("ftdi-uart")
                      .set_outgoing_dir(directory_client.TakeChannel())
                      .set_runtime_service_offers(fidl_service_offers));
  if (status != ZX_OK) {
    zxlogf(ERROR, "ftdi_uart: device_add failed");
    return FtdiBindFail(status);
  }

  usb_request_complete_callback_t complete = {
      .callback =
          [](void* ctx, usb_request_t* request) {
            static_cast<FtdiDevice*>(ctx)->ReadComplete(request);
          },
      .ctx = this,
  };

  // Queue the read requests.
  std::optional<usb::Request<>> req;
  while ((req = free_read_queue_.pop())) {
    usb_client_.RequestQueue(req->take(), &complete);
  }

  bulk_in_addr_ = bulk_in_addr;
  bulk_out_addr_ = bulk_out_addr;

  zxlogf(INFO, "ftdi bind successful");
  return status;
}

}  // namespace ftdi_serial

namespace {

zx_status_t ftdi_bind(void* ctx, zx_device_t* device) {
  fbl::AllocChecker ac;
  auto dev = fbl::make_unique_checked<ftdi_serial::FtdiDevice>(&ac, device);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  auto status = dev->Bind();
  if (status == ZX_OK) {
    // Devmgr is now in charge of the memory for dev.
    [[maybe_unused]] auto ptr = dev.release();
  }
  return status;
}

static constexpr zx_driver_ops_t ftdi_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = ftdi_bind;
  return ops;
}();

}  // namespace

ZIRCON_DRIVER(ftdi, ftdi_driver_ops, "zircon", "0.1");
