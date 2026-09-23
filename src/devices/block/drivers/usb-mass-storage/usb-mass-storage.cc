// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-mass-storage.h"

#include <endian.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <stdio.h>
#include <string.h>
#include <zircon/assert.h>

#include <mutex>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <safemath/safe_conversions.h>
#include <usb/ums.h>
#include <usb/usb.h>

namespace ums {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace {

constexpr uint8_t kPlaceholderTarget = 0;

void ReqComplete(void* ctx, usb_request_t* req) {
  if (ctx) {
    sync_completion_signal(static_cast<sync_completion_t*>(ctx));
  }
}

}  // namespace

class WaiterImpl : public WaiterInterface {
 public:
  zx_status_t Wait(sync_completion_t* completion, zx_duration_t duration) {
    return sync_completion_wait(completion, duration);
  }
};

void UsbMassStorageDevice::ExecuteCommandsAsync(uint8_t target, uint16_t lun,
                                                std::span<scsi::ScsiRequest> batch) {
  if (dead_) {
    for (auto& req : batch) {
      req.Complete(ZX_ERR_IO_NOT_PRESENT);
    }
    return;
  }

  if (lun > max_lun_) {
    for (auto& req : batch) {
      req.Complete(ZX_ERR_OUT_OF_RANGE);
    }
    return;
  }

  bool fail_requests = false;
  {
    std::lock_guard<std::mutex> l(queue_lock_);
    if (fail_new_requests_[lun]) {
      fail_requests = true;
    } else {
      for (auto& req : batch) {
        queued_txns_.push_back(Transaction{
            .request = std::move(req),
            .lun = static_cast<uint8_t>(lun),
        });
      }
    }
  }

  if (fail_requests) {
    for (auto& req : batch) {
      req.Complete(ZX_ERR_IO_NOT_PRESENT);
    }
    return;
  }
  sync_completion_signal(&txn_completion_);
}

void UsbMassStorageDevice::Stop(fdf::StopCompleter completer) {
  dead_ = true;
  // wait for worker loop to finish before removing devices
  if (worker_dispatcher_.get()) {
    sync_completion_signal(&txn_completion_);
    worker_dispatcher_.ShutdownAsync();
    worker_shutdown_completion_.Wait();
  }

  // Wait for remaining requests to complete
  while (pending_requests_.load()) {
    waiter_->Wait(&txn_completion_, ZX_SEC(1));
  }

  if (cbw_req_) {
    usb_request_release(cbw_req_);
  }
  if (data_req_) {
    usb_request_release(data_req_);
  }
  if (csw_req_) {
    usb_request_release(csw_req_);
  }

  {
    std::lock_guard<std::mutex> lock(luns_lock_);
    for (auto& dev : block_devs_) {
      if (dev) {
        libsync::Completion completion;
        dev->ShutdownAsync([&completion] { completion.Signal(); });
        completion.Wait();
        dev.reset();
      }
    }
    block_devs_.clear();
  }

  if (data_transfer_req_) {
    // release_frees is indirectly cleared by DataTransfer; set it again here so that
    // data_transfer_req_ is freed by usb_request_release.
    data_transfer_req_->release_frees = true;
    usb_request_release(data_transfer_req_);
  }
  completer(zx::ok());
}

void UsbMassStorageDevice::RequestQueue(usb_request_t* request,
                                        const usb_request_complete_callback_t* completion) {
  std::lock_guard<std::mutex> l(txn_lock_);
  pending_requests_++;
  UsbRequestContext context;
  context.completion = *completion;
  usb_request_complete_callback_t complete;
  complete.callback = [](void* ctx, usb_request_t* req) {
    UsbRequestContext context;
    memcpy(&context,
           reinterpret_cast<unsigned char*>(req) +
               reinterpret_cast<UsbMassStorageDevice*>(ctx)->parent_req_size_,
           sizeof(context));
    reinterpret_cast<UsbMassStorageDevice*>(ctx)->pending_requests_--;
    context.completion.callback(context.completion.ctx, req);
  };
  complete.ctx = this;
  memcpy(reinterpret_cast<unsigned char*>(request) + parent_req_size_, &context, sizeof(context));
  usb_.RequestQueue(request, &complete);
}

// Performs the object initialization.
zx_status_t UsbMassStorageDevice::Init() {
  zx::result<ddk::UsbProtocolClient> client =
      compat::ConnectBanjo<ddk::UsbProtocolClient>(incoming());
  if (client.is_error()) {
    fdf::error("Failed to connect USB protocol client: {}", client);
    return client.status_value();
  }

  usb_protocol_t proto;
  client->GetProto(&proto);
  usb::UsbDevice usb(&proto);
  if (!usb.is_valid()) {
    return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
  }

  // find our endpoints
  std::optional<usb::InterfaceList> interfaces;
  zx_status_t status = usb::InterfaceList::Create(usb, true, &interfaces);
  if (status != ZX_OK) {
    return status;
  }
  auto interface = interfaces->begin();
  if (interface == interfaces->end()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  const usb_interface_descriptor_t* interface_descriptor = interface->descriptor();
  // Since interface != interface->end(), interface_descriptor is guaranteed not null.
  ZX_DEBUG_ASSERT(interface_descriptor);
  uint8_t interface_number = interface_descriptor->b_interface_number;
  uint8_t bulk_in_addr = 0;
  uint8_t bulk_out_addr = 0;
  size_t bulk_in_max_packet = 0;
  size_t bulk_out_max_packet = 0;

  if (interface_descriptor->b_num_endpoints < 2) {
    fdf::debug("UMS: ums_bind wrong number of endpoints: {}",
               interface_descriptor->b_num_endpoints);
    return ZX_ERR_NOT_SUPPORTED;
  }

  for (auto ep_itr : interfaces->begin()->GetEndpointList()) {
    const usb_endpoint_descriptor_t* endp = ep_itr.descriptor();
    if (usb_ep_direction(endp) == USB_ENDPOINT_OUT) {
      if (usb_ep_type(endp) == fdescriptor::EndpointType::kBulk) {
        bulk_out_addr = endp->b_endpoint_address;
        bulk_out_max_packet = usb_ep_max_packet(endp);
      }
    } else {
      if (usb_ep_type(endp) == fdescriptor::EndpointType::kBulk) {
        bulk_in_addr = endp->b_endpoint_address;
        bulk_in_max_packet = usb_ep_max_packet(endp);
      }
    }
  }

  if (!bulk_in_max_packet || !bulk_out_max_packet) {
    fdf::debug("UMS: ums_bind could not find endpoints");
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint8_t max_lun;
  size_t out_length;
  status = usb.ControlIn(USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_REQ_GET_MAX_LUN,
                         0x00, 0x00, ZX_TIME_INFINITE, &max_lun, sizeof(max_lun), &out_length);
  if (status == ZX_ERR_IO_REFUSED) {
    // Devices that do not support multiple LUNS may stall this command.
    // See USB Mass Storage Class Spec. 3.2 Get Max LUN.
    // Clear the stall.
    usb.ResetEndpoint(0);
    fdf::info("Device does not support multiple LUNs");
    max_lun = 0;
  } else if (status != ZX_OK) {
    return status;
  } else if (out_length != sizeof(max_lun)) {
    return ZX_ERR_BAD_STATE;
  }
  {
    std::lock_guard<std::mutex> lock(luns_lock_);
    block_devs_ = std::vector<std::unique_ptr<scsi::BlockDevice>>(max_lun + 1);
  }
  fdf::debug("UMS: Max lun is: {}", max_lun);
  max_lun_ = max_lun;

  {
    std::lock_guard<std::mutex> l(queue_lock_);
    queued_txns_.clear();
    fail_new_requests_ = std::vector<bool>(max_lun + 1, false);
  }
  sync_completion_reset(&txn_completion_);

  usb_ = usb;
  bulk_in_addr_ = bulk_in_addr;
  bulk_out_addr_ = bulk_out_addr;
  bulk_in_max_packet_ = bulk_in_max_packet;
  bulk_out_max_packet_ = bulk_out_max_packet;
  interface_number_ = interface_number;

  size_t max_in = usb.GetMaxTransferSize(bulk_in_addr);
  size_t max_out = usb.GetMaxTransferSize(bulk_out_addr);
  // SendCbw() accepts max transfer length of UINT32_MAX.
  max_transfer_bytes_ = static_cast<uint32_t>((max_in < max_out ? max_in : max_out));
  parent_req_size_ = usb.GetRequestSize();
  ZX_DEBUG_ASSERT(parent_req_size_ != 0);
  size_t usb_request_size = parent_req_size_ + sizeof(UsbRequestContext);
  status = usb_request_alloc(&cbw_req_, sizeof(ums_cbw_t), bulk_out_addr, usb_request_size);
  if (status != ZX_OK) {
    return status;
  }
  status = usb_request_alloc(&data_req_, zx_system_get_page_size(), bulk_in_addr, usb_request_size);
  if (status != ZX_OK) {
    return status;
  }
  status = usb_request_alloc(&csw_req_, sizeof(ums_csw_t), bulk_in_addr, usb_request_size);
  if (status != ZX_OK) {
    return status;
  }

  status = usb_request_alloc(&data_transfer_req_, 0, bulk_in_addr, usb_request_size);
  if (status != ZX_OK) {
    return status;
  }

  tag_send_ = tag_receive_ = 8;

  for (uint8_t lun = 0; lun <= max_lun_; lun++) {
    zx::result inquiry_data = Inquiry(kPlaceholderTarget, lun);
    if (inquiry_data.is_error()) {
      return inquiry_data.status_value();
    }
  }

  status = CheckLunsReady();
  if (status != ZX_OK) {
    fdf::error("Failed initial check of whether LUNs are ready: {}", zx_status_get_string(status));
    return status;
  }

  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "ums-worker",
      [&](fdf_dispatcher_t*) { worker_shutdown_completion_.Signal(); });
  if (dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher: {}", zx_status_get_string(dispatcher.status_value()));
    return dispatcher.status_value();
  }
  worker_dispatcher_ = *std::move(dispatcher);

  status = async::PostTask(worker_dispatcher_.async_dispatcher(), [this] { WorkerLoop(); });
  if (status != ZX_OK) {
    fdf::error("Failed to start worker loop: {}", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

zx_status_t UsbMassStorageDevice::Reset() {
  // UMS Reset Recovery. See section 5.3.4 of
  // "Universal Serial Bus Mass Storage Class Bulk-Only Transport"
  fdf::debug("UMS: performing reset recovery");
  // Step 1: Send  Bulk-Only Mass Storage Reset
  zx_status_t status =
      usb_.ControlOut(USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE, USB_REQ_RESET, 0,
                      interface_number_, ZX_TIME_INFINITE, NULL, 0);
  usb_protocol_t usb;
  usb_.GetProto(&usb);
  if (status != ZX_OK) {
    fdf::debug("UMS: USB_REQ_RESET failed: {}", zx_status_get_string(status));
    return status;
  }
  // Step 2: Clear Feature HALT to the Bulk-In endpoint
  constexpr uint8_t request_type = USB_DIR_OUT | USB_RECIP_ENDPOINT;
  status = usb_.ClearFeature(request_type, USB_ENDPOINT_HALT, bulk_in_addr_, ZX_TIME_INFINITE);
  if (status != ZX_OK) {
    fdf::debug("UMS: clear endpoint halt failed: {}", zx_status_get_string(status));
    return status;
  }
  // Step 3: Clear Feature HALT to the Bulk-Out endpoint
  status = usb_.ClearFeature(request_type, USB_ENDPOINT_HALT, bulk_out_addr_, ZX_TIME_INFINITE);
  if (status != ZX_OK) {
    fdf::debug("UMS: clear endpoint halt failed: {}", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

zx_status_t UsbMassStorageDevice::SendCbw(uint8_t lun, uint32_t transfer_length, uint8_t flags,
                                          uint8_t command_len, const void* command) {
  usb_request_t* req = cbw_req_;

  ums_cbw_t* cbw;
  zx_status_t status = usb_request_mmap(req, (void**)&cbw);
  if (status != ZX_OK) {
    fdf::debug("UMS: usb request mmap failed: {}", zx_status_get_string(status));
    return status;
  }

  memset(cbw, 0, sizeof(*cbw));
  cbw->dCBWSignature = htole32(CBW_SIGNATURE);
  cbw->dCBWTag = htole32(tag_send_++);
  cbw->dCBWDataTransferLength = htole32(transfer_length);
  cbw->bmCBWFlags = flags;
  cbw->bCBWLUN = lun;
  cbw->bCBWCBLength = command_len;

  // copy command_len bytes from the command passed in into the command_len
  memcpy(cbw->CBWCB, command, command_len);

  sync_completion_t completion;
  usb_request_complete_callback_t complete = {
      .callback = ReqComplete,
      .ctx = &completion,
  };
  RequestQueue(req, &complete);
  waiter_->Wait(&completion, ZX_TIME_INFINITE);
  return req->response.status;
}

zx_status_t UsbMassStorageDevice::ReadCsw(uint32_t* out_residue, bool retry) {
  sync_completion_t completion;
  usb_request_complete_callback_t complete = {
      .callback = ReqComplete,
      .ctx = &completion,
  };

  usb_request_t* csw_request = csw_req_;
  RequestQueue(csw_request, &complete);
  waiter_->Wait(&completion, ZX_TIME_INFINITE);
  if (csw_request->response.status != ZX_OK) {
    if (csw_request->response.status == ZX_ERR_IO_REFUSED) {
      if (retry) {
        Reset();
        return csw_request->response.status;
      }
      // Stalled. Clear Bulk In endpoint and try to receive CSW again.
      auto status = usb_.ResetEndpoint(bulk_in_addr_);
      if (status != ZX_OK) {
        fdf::error("ResetEndpoint failed {}", zx_status_get_string(status));
        return status;
      }
      constexpr uint8_t request_type = USB_DIR_OUT | USB_RECIP_ENDPOINT;
      status = usb_.ClearFeature(request_type, USB_ENDPOINT_HALT, bulk_in_addr_, ZX_TIME_INFINITE);
      if (status != ZX_OK) {
        fdf::error("UMS: clear endpoint halt failed: {}", zx_status_get_string(status));
        return status;
      }
      return ReadCsw(out_residue, true);
    }
    fdf::error("UMS: ReadCsw failed with status {}",
               zx_status_get_string(csw_request->response.status));
    return csw_request->response.status;
  }
  csw_status_t csw_error = VerifyCsw(csw_request, out_residue);

  if (csw_error == CSW_SUCCESS) {
    return ZX_OK;
  } else if (csw_error == CSW_FAILED) {
    return ZX_ERR_BAD_STATE;
  } else {
    // FIXME - best way to handle this?
    // print error and then reset device due to it
    fdf::debug("UMS: CSW verify returned error. Check ums-hw.h csw_status_t for enum = {}",
               csw_error);
    Reset();
    return ZX_ERR_INTERNAL;
  }
}

csw_status_t UsbMassStorageDevice::VerifyCsw(usb_request_t* csw_request, uint32_t* out_residue) {
  ums_csw_t csw = {};
  [[maybe_unused]] size_t result = usb_request_copy_from(csw_request, &csw, sizeof(csw), 0);

  // check signature is "USBS"
  if (letoh32(csw.dCSWSignature) != CSW_SIGNATURE) {
    fdf::debug("UMS: invalid CSW sig: {:08x}", letoh32(csw.dCSWSignature));
    return CSW_INVALID;
  }

  // check if tag matches the tag of last CBW
  if (letoh32(csw.dCSWTag) != tag_receive_++) {
    fdf::debug("UMS: CSW tag mismatch, expected:{:08x} got in CSW:{:08x}", tag_receive_ - 1,
               letoh32(csw.dCSWTag));
    return CSW_TAG_MISMATCH;
  }
  // check if success is true or not?
  if (csw.bmCSWStatus == CSW_FAILED) {
    return CSW_FAILED;
  } else if (csw.bmCSWStatus == CSW_PHASE_ERROR) {
    return CSW_PHASE_ERROR;
  }

  if (out_residue) {
    *out_residue = letoh32(csw.dCSWDataResidue);
  }
  return CSW_SUCCESS;
}

zx_status_t UsbMassStorageDevice::ReadSync(size_t transfer_length) {
  // Read response code from device
  usb_request_t* read_request = data_req_;
  read_request->header.length = transfer_length;
  sync_completion_t completion;
  usb_request_complete_callback_t complete = {
      .callback = ReqComplete,
      .ctx = &completion,
  };
  RequestQueue(read_request, &complete);
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
  return read_request->response.status;
}

zx_status_t UsbMassStorageDevice::ExecuteCommandSync(uint8_t target, uint16_t lun, iovec cdb,
                                                     bool is_write, iovec data) {
  if (lun > UINT8_MAX) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  if (data.iov_len > max_transfer_bytes_) {
    fdf::error("Request exceeding max transfer size.");
    return ZX_ERR_INVALID_ARGS;
  }
  if (is_write && data.iov_len > 0) {
    fdf::error("Write data transfers are not supported in ExecuteCommandSync.");
    return ZX_ERR_NOT_SUPPORTED;
  }
  const bool read_data_transfer = !is_write && data.iov_base != nullptr;
  if (read_data_transfer && data.iov_len > zx_system_get_page_size()) {
    // data_req_ has a size of zx_system_get_page_size().
    fdf::error("Read data transfer request exceeding page size.");
    return ZX_ERR_INVALID_ARGS;
  }

  // Per section 6.5 of UMS specification version 1.0
  // the device should report any errors in the CSW stage,
  // which seems to suggest that stalling here is out-of-spec.
  // Some devices that we tested with do stall the CBW or data transfer stage,
  // so to accommodate those devices we consider the transfer to have ended (with an error)
  // when we receive a stall condition from the device.
  zx_status_t status =
      SendCbw(static_cast<uint8_t>(lun), static_cast<uint32_t>(data.iov_len),
              is_write ? USB_DIR_OUT : USB_DIR_IN, static_cast<uint8_t>(cdb.iov_len), cdb.iov_base);
  if (status != ZX_OK) {
    fdf::warn("UMS: SendCbw failed with status {}", zx_status_get_string(status));
    return status;
  }

  if (read_data_transfer) {
    // read response
    status = ReadSync(data.iov_len);
    if (status != ZX_OK) {
      fdf::warn("UMS: ReadSync failed with status {}", zx_status_get_string(status));
      return status;
    }
  }

  // wait for CSW
  status = ReadCsw(NULL);
  if (status == ZX_OK && read_data_transfer) {
    memset(data.iov_base, 0, data.iov_len);
    [[maybe_unused]] auto result = usb_request_copy_from(data_req_, data.iov_base, data.iov_len, 0);
  }
  return status;
}

zx_status_t UsbMassStorageDevice::DataTransfer(zx_handle_t vmo_handle, zx_off_t offset,
                                               size_t length, uint8_t ep_address) {
  usb_request_t* req = data_transfer_req_;

  zx_status_t status = usb_request_init(req, vmo_handle, offset, length, ep_address);
  if (status != ZX_OK) {
    return status;
  }

  sync_completion_t completion;
  usb_request_complete_callback_t complete = {
      .callback = ReqComplete,
      .ctx = &completion,
  };
  RequestQueue(req, &complete);
  waiter_->Wait(&completion, ZX_TIME_INFINITE);

  status = req->response.status;
  if (status == ZX_OK && req->response.actual != length) {
    status = ZX_ERR_IO;
  }

  usb_request_release(req);
  return status;
}

zx_status_t UsbMassStorageDevice::DoTransaction(scsi::ScsiRequest& req, uint8_t lun,
                                                std::string_view action) {
  const size_t num_bytes =
      req.immediate_data().size() > 0 ? req.immediate_data().size() : req.transfer_length_bytes();

  zx::vmo data_vmo;
  fzl::VmoMapper mapper;
  zx_handle_t vmo_handle = ZX_HANDLE_INVALID;
  zx_off_t vmo_offset = 0;

  if (num_bytes) {
    if (req.immediate_data().size() > 0) {
      // For requests with immediate data (e.g. UNMAP), allocate a temporary buffer before
      // sending CBW so we don't leave the USB device waiting for data on failure.
      if (zx::result<> result = AllocatePages(data_vmo, mapper, num_bytes); result.is_error()) {
        fdf::error("Failed to allocate data buffer: {}", result);
        return result.status_value();
      }
      memcpy(mapper.start(), req.immediate_data().data(), num_bytes);
      vmo_handle = data_vmo.get();
      vmo_offset = 0;
    } else {
      vmo_handle = req.data_vmo()->get();
      vmo_offset = req.vmo_offset();
    }
  }

  const uint8_t flags = req.is_write() ? USB_DIR_OUT : USB_DIR_IN;
  const uint8_t ep_address = req.is_write() ? bulk_out_addr_ : bulk_in_addr_;

  zx_status_t status = SendCbw(lun, static_cast<uint32_t>(num_bytes), flags,
                               static_cast<uint8_t>(req.cdb().size()), req.cdb().data());
  if (status != ZX_OK) {
    fdf::warn("UMS: SendCbw during {} failed with status {}", action, zx_status_get_string(status));
    return status;
  }

  if (num_bytes) {
    status = DataTransfer(vmo_handle, vmo_offset, num_bytes, ep_address);
    if (status != ZX_OK) {
      return status;
    }
  }

  // receive CSW
  uint32_t residue;
  status = ReadCsw(&residue);
  if (status == ZX_OK && residue) {
    fdf::error("unexpected residue in {}", action);
    status = ZX_ERR_IO;
  }

  return status;
}

zx_status_t UsbMassStorageDevice::CheckLunsReady() {
  // If the device is marked as dead (e.g. disconnected or shutting down),
  // stop checking and return an appropriate error.
  if (dead_) {
    return ZX_ERR_IO_NOT_PRESENT;
  }
  std::lock_guard<std::mutex> lock(luns_lock_);

  zx_status_t final_status = ZX_OK;
  for (uint8_t lun = 0; lun <= max_lun_; lun++) {
    bool ready = false;
    zx_status_t status = TestUnitReady(kPlaceholderTarget, lun);
    if (status == ZX_OK) {
      ready = true;
    } else if (status == ZX_ERR_BAD_STATE) {
      // command returned CSW_FAILED. device is there but media is not ready.
      uint8_t request_sense_data[UMS_REQUEST_SENSE_TRANSFER_LENGTH];
      zx_status_t sense_status = RequestSense(
          kPlaceholderTarget, lun, {request_sense_data, UMS_REQUEST_SENSE_TRANSFER_LENGTH});
      if (sense_status != ZX_OK) {
        fdf::warn("RequestSense failed with status {}", zx_status_get_string(sense_status));
        final_status = sense_status;
        continue;
      }
    } else if (status == ZX_ERR_IO_NOT_PRESENT) {
      dead_ = true;
      return status;
    } else {
      // LUN check failed, record the error but continue to ensure any
      // other healthy LUNs are initialized.
      fdf::warn("TestUnitReady returned error - {}", zx_status_get_string(status));
      final_status = status;
      continue;
    }

    if (ready && !block_devs_[lun]) {
      scsi::DeviceOptions options(/*check_unmap_support*/ true, /*use_mode_sense_6*/ true,
                                  /*use_read_write_12*/ false);
      zx::result result =
          scsi::BlockDevice::Bind(this, kPlaceholderTarget, lun, max_transfer_bytes_, options);
      if (result.is_ok() && result.value()->block_size_bytes() != 0) {
        block_devs_[lun] = std::move(result.value());
        {
          std::lock_guard<std::mutex> l(queue_lock_);
          fail_new_requests_[lun] = false;
        }
        scsi::BlockDevice* dev = block_devs_[lun].get();
        fdf::debug("UMS: block size is: {:#010x}", dev->block_size_bytes());
        fdf::debug("UMS: total blocks is: {}", dev->block_count());
        fdf::debug("UMS: total size is: {}", dev->block_count() * dev->block_size_bytes());
        fdf::debug("UMS: read-only: {} removable: {}", dev->write_protected(), dev->removable());
      } else {
        zx_status_t error = result.status_value();
        if (error == ZX_OK) {
          fdf::error("UMS zero block size");
          error = ZX_ERR_INVALID_ARGS;
        }
        fdf::error("UMS: device_add for block device failed: {}", zx_status_get_string(error));
        final_status = error;
      }
    } else if (!ready && block_devs_[lun]) {
      // Failing new requests and draining queued transactions before ShutdownAsync is
      // required because BlockServer::DestroyAsync waits for all active requests to complete
      // (via Complete/SendReply) before terminating sessions; failing to complete them before
      // ShutdownAsync causes a deadlock.
      std::deque<Transaction> txns_to_complete;
      {
        std::lock_guard<std::mutex> l(queue_lock_);
        fail_new_requests_[lun] = true;
        for (auto it = queued_txns_.begin(); it != queued_txns_.end();) {
          if (it->lun == lun) {
            txns_to_complete.push_back(std::move(*it));
            it = queued_txns_.erase(it);
          } else {
            ++it;
          }
        }
      }
      for (auto& txn : txns_to_complete) {
        txn.request.Complete(ZX_ERR_IO_NOT_PRESENT);
      }

      if (on_pre_shutdown_) {
        on_pre_shutdown_(lun);
      }

      libsync::Completion completion;
      block_devs_[lun]->ShutdownAsync([&completion] { completion.Signal(); });
      completion.Wait();

      block_devs_[lun].reset();
    }
  }

  return final_status;
}

zx::result<> UsbMassStorageDevice::AllocatePages(zx::vmo& vmo, fzl::VmoMapper& mapper,
                                                 size_t size) {
  const uint32_t data_size =
      fbl::round_up(safemath::checked_cast<uint32_t>(size), zx_system_get_page_size());
  if (zx_status_t status = zx::vmo::create(data_size, 0, &vmo); status != ZX_OK) {
    return zx::error(status);
  }

  if (zx_status_t status = mapper.Map(vmo, 0, data_size); status != ZX_OK) {
    fdf::error("Failed to map IO buffer: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

void UsbMassStorageDevice::WorkerLoop() {
  bool wait = true;
  while (1) {
    if (wait) {
      waiter_->Wait(&txn_completion_, ZX_SEC(1));
      sync_completion_reset(&txn_completion_);
      std::lock_guard<std::mutex> l(queue_lock_);
      if (queued_txns_.empty() && !dead_) {
        async::PostTask(dispatcher(), [&] {
          // This must be done in the default dispatcher because it accesses
          // fdf::DriverBase::outgoing().
          zx_status_t status = CheckLunsReady();
          if (status != ZX_OK) {
            // keep going
            fdf::error("Failed to periodically check whether LUNs are ready: {}",
                       zx_status_get_string(status));
          }
        });
        continue;
      }
    }
    std::lock_guard<std::mutex> lock(luns_lock_);
    if (dead_) {
      break;
    }
    Transaction txn;
    {
      std::lock_guard<std::mutex> l(queue_lock_);
      if (queued_txns_.empty()) {
        wait = true;
        continue;
      }
      wait = false;
      txn = std::move(queued_txns_.front());
      queued_txns_.pop_front();
    }

    // Note: luns_lock_ held above guards block_devs_ against concurrent reset in CheckLunsReady.
    // Pending transactions for this LUN are drained before resetting block_devs_[txn.lun],
    // so dev is guaranteed to be valid here.
    scsi::BlockDevice* dev = block_devs_[txn.lun].get();
    ZX_ASSERT(dev != nullptr);

    std::string_view action = "IO";
    switch (txn.request.opcode()) {
      case scsi::Opcode::READ_10:
      case scsi::Opcode::READ_12:
      case scsi::Opcode::READ_16:
        action = "Read";
        break;
      case scsi::Opcode::WRITE_10:
      case scsi::Opcode::WRITE_12:
      case scsi::Opcode::WRITE_16:
        action = "Write";
        break;
      case scsi::Opcode::SYNCHRONIZE_CACHE_10:
      case scsi::Opcode::SYNCHRONIZE_CACHE_16:
        action = "Flush";
        break;
      case scsi::Opcode::UNMAP:
        action = "Trim";
        break;
      default:
        action = "Command";
        break;
    }
    zx_status_t status = DoTransaction(txn.request, txn.lun, action);
    if (status != ZX_OK) {
      if (txn.request.transfer_length() > 0) {
        fdf::error("UMS: {} of {} blocks @ {} failed: {}", action, txn.request.transfer_length(),
                   txn.request.device_offset(), zx_status_get_string(status));
      } else {
        fdf::error("UMS: {} failed: {}", action, zx_status_get_string(status));
      }
    }
    txn.request.Complete(status);
  }

  // complete any pending txns
  std::deque<Transaction> txns;
  {
    std::lock_guard<std::mutex> l(queue_lock_);
    txns.swap(queued_txns_);
  }

  for (auto& txn : txns) {
    txn.request.Complete(ZX_ERR_IO_NOT_PRESENT);
  }
}

zx::result<> UsbMassStorageDevice::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_name_ = context.node_name();

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  node_controller_.Bind(std::move(controller_client_end));
  root_node_.Bind(std::move(node_client_end));

  fidl::Arena arena;

  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, name()).Build();

  // Add root device, which will contain block devices for logical units
  auto result = fidl::WireCall(node().borrow())
                    ->AddChild(args, std::move(controller_server_end), std::move(node_server_end));
  if (!result.ok()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return zx::error(result.status());
  }

  waiter_ = fbl::MakeRefCounted<WaiterImpl>();
  zx_status_t status = Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

}  // namespace ums
