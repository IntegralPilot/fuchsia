
// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ums-function/ums-function.h"

#include <endian.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.request/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/scsi/controller.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <stdint.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <atomic>
#include <optional>
#include <vector>

#include <usb/descriptors.h>
#include <usb/request-fidl.h>
#include <usb/ums.h>

#include "src/devices/block/lib/common/include/common.h"

namespace ums {

namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace ffunction = fuchsia_hardware_usb_function;
namespace ffdf = fuchsia_driver_framework;
namespace frequest = fuchsia_hardware_usb_request;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

void UmsFunction::Control(ControlRequest& req, ControlCompleter::Sync& completer) {
  if (req.setup().bm_request_type() == (USB_DIR_IN | USB_TYPE_CLASS | USB_RECIP_INTERFACE) &&
      req.setup().b_request() == USB_REQ_GET_MAX_LUN && req.setup().w_value() == 0 &&
      req.setup().w_index() == 0 && req.setup().w_length() >= sizeof(uint8_t)) {
    completer.Reply(zx::ok(std::vector<uint8_t>{0}));
    return;
  }

  if (req.setup().bm_request_type() == (USB_DIR_OUT | USB_TYPE_CLASS | USB_RECIP_INTERFACE) &&
      req.setup().b_request() == USB_REQ_RESET && req.setup().w_value() == 0 &&
      req.setup().w_index() == 0 && req.setup().w_length() == 0) {
    // Cancel all pending requests
    zx::result result = CancelAll(in_ep_);
    if (result.is_error()) {
      fdf::error("Error canceling existing in-type requests: {}", result);
    }

    result = CancelAll(out_ep_);
    if (result.is_error()) {
      fdf::error("Error canceling existing out-type requests: {}", result);
    }

    csw_in_flight_ = false;
    completer.Reply(zx::ok(std::vector<uint8_t>{}));
    return;
  }

  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

zx::result<> UmsFunction::ConfigureEndpoint(const usb_endpoint_info_descriptor_t& desc) {
  ffunction::EndpointConfiguration ep_cfg;
  ffunction::EndpointDescriptor f_desc;
  f_desc.b_interval(desc.b_interval);
  f_desc.bm_attributes(desc.bm_attributes);
  f_desc.w_max_packet_size(le16toh(desc.w_max_packet_size));
  ep_cfg.descriptor(std::move(f_desc));

  fidl::Result result = function_->ConfigureEndpoint({{
      .endpoint_address = desc.b_endpoint_address,
      .endpoint_configuration = std::move(ep_cfg),
  }});
  if (result.is_error()) {
    fdf::error("Could not configure endpoint: {}", result.error_value().FormatDescription());

    if (result.error_value().is_domain_error()) {
      return zx::error(result.error_value().domain_error());
    }
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

void UmsFunction::SetConfigured(SetConfiguredRequest& req,
                                SetConfiguredCompleter::Sync& completer) {
  zx::result<> result = zx::ok();
  configured_ = req.configured();

  // TODO(voydanoff) fullspeed and superspeed support
  if (req.configured()) {
    result = ConfigureEndpoint(config_.in_ep);
    if (result.is_error()) {
      fdf::error("SetConfigured: ConfigureEndpoint(in): {}", result);
    }

    result = ConfigureEndpoint(config_.out_ep);
    if (result.is_error()) {
      fdf::error("SetConfigured: ConfigureEndpoint(out): {}", result);
    }
  } else {
    fidl::Result disable =
        function_->DisableEndpoint({{.endpoint_address = config_.in_ep.b_endpoint_address}});
    if (disable.is_error()) {
      fdf::error("SetConfigured: DisableEndpoint(in) fails: {}",
                 disable.error_value().FormatDescription());
      result =
          zx::error(disable.error_value().is_domain_error() ? disable.error_value().domain_error()
                                                            : ZX_ERR_INTERNAL);
    }

    disable = function_->DisableEndpoint({{.endpoint_address = config_.out_ep.b_endpoint_address}});
    if (disable.is_error()) {
      fdf::error("SetConfigured: DisableEndpoint(out) fails: {}",
                 disable.error_value().FormatDescription());
      result =
          zx::error(disable.error_value().is_domain_error() ? disable.error_value().domain_error()
                                                            : ZX_ERR_INTERNAL);
    }

    // Reset state flags on disconnect.
    csw_in_flight_ = false;
  }

  if (result.is_error()) {
    fdf::error("SetConfigured fails overall: {}", result.status_value());
    completer.Reply(result.take_error());
    return;
  }

  if (req.configured()) {
    RequestQueue(&cbw_req_.value());
  }

  completer.Reply(zx::ok());
}

void UmsFunction::SetInterface(SetInterfaceRequest& req, SetInterfaceCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void UmsFunction::RequestQueue(usb::FidlRequest* req) {
  usb::EndpointClient<UmsFunction>* ep = nullptr;

  if (req == &cbw_req_.value()) {
    ep = &out_ep_;
  } else if (req == &csw_req_.value()) {
    ep = &in_ep_;
    csw_in_flight_ = true;
  } else if (req == &data_in_req_.value()) {
    ep = &in_ep_;
  } else if (req == &data_out_req_.value()) {
    ep = &out_ep_;
  }

  atomic_fetch_add(&pending_request_count_, 1);
  std::vector<frequest::Request> reqs;
  reqs.emplace_back(req->take_request());

  ZX_ASSERT(ep->client()->QueueRequests(std::move(reqs)).is_ok());
}

void UmsFunction::InEpCallback(std::vector<fendpoint::Completion> completion) {
  // CSW and in-data.
  ZX_ASSERT(completion.size() == 1);
  ZX_ASSERT(completion[0].transfer_size().has_value());
  ZX_ASSERT(completion[0].request().has_value());

  atomic_fetch_add(&pending_request_count_, -1);

  // Identify if this is a CSW or Data IN completion by comparing the VMO ID.
  if (completion[0].request()->data()->at(0).buffer()->vmo_id().value() == csw_vmo_id_) {
    csw_req_.emplace(std::move(*completion[0].request()));
    csw_in_flight_ = false;

    if (!csw_q_.empty()) {
      QueueCsw(csw_q_.front(), false);
      csw_q_.pop();
    }
    CswComplete(std::move(completion[0]));
  } else {
    data_in_req_.emplace(std::move(*completion[0].request()));
    DataComplete(std::move(completion[0]));
  }

  if (!active_ && pending_request_count_ == 0 && stop_completer_) {
    dispatcher_.ShutdownAsync();
    (*stop_completer_)(zx::ok());
    stop_completer_.reset();
  }
}

void UmsFunction::OutEpCallback(std::vector<fendpoint::Completion> completion) {
  // CBW and out-data.
  ZX_ASSERT(completion.size() == 1);
  ZX_ASSERT(completion[0].transfer_size().has_value());
  ZX_ASSERT(completion[0].request().has_value());

  atomic_fetch_add(&pending_request_count_, -1);

  // Identify if this is a CBW or Data OUT completion by comparing the VMO ID.
  if (completion[0].request()->data()->at(0).buffer()->vmo_id().value() == cbw_vmo_id_) {
    cbw_req_.emplace(std::move(*completion[0].request()));
    CbwComplete(std::move(completion[0]));
  } else {
    data_out_req_.emplace(std::move(*completion[0].request()));
    DataComplete(std::move(completion[0]));
  }

  if (!active_ && pending_request_count_ == 0 && stop_completer_) {
    dispatcher_.ShutdownAsync();
    (*stop_completer_)(zx::ok());
    stop_completer_.reset();
  }
}

void UmsFunction::QueueData(usb::FidlRequest* req) {
  data_length_ += (*req)->data()->at(0).size().value();
  RequestQueue(req);
}

void UmsFunction::QueueCsw(uint8_t status, bool also_cbw) {
  if (also_cbw) {
    // first queue next cbw so it is ready to go
    RequestQueue(&cbw_req_.value());
  }

  if (csw_in_flight_) {
    csw_q_.push(status);
    return;
  }

  ums_csw_t* csw;
  std::optional<zx_vaddr_t> vaddr = in_ep_.GetMappedAddr(csw_req_->request(), 0);
  if (!vaddr.has_value()) {
    fdf::error("QueueCsw: GetMappedAddr failed");
    return;
  }
  csw = reinterpret_cast<ums_csw_t*>(*vaddr);

  csw->dCSWSignature = htole32(CSW_SIGNATURE);
  csw->dCSWTag = current_cbw_.dCBWTag;
  uint32_t cbw_len = le32toh(current_cbw_.dCBWDataTransferLength);
  uint32_t residue = 0;
  if (cbw_len > data_length_) {
    residue = static_cast<uint32_t>(cbw_len - data_length_);
  }
  csw->dCSWDataResidue = htole32(residue);
  csw->bmCSWStatus = status;

  zx::result<std::vector<size_t>> actual =
      csw_req_->CachedCopyTo(0, csw, sizeof(ums_csw_t), in_ep_.GetMapped());
  if (actual.is_error() || actual->size() != 1 || (*actual)[0] != sizeof(ums_csw_t)) {
    fdf::error("QueueCsw: failed to copy CSW data");
    return;
  }
  (*csw_req_)->data()->at(0).size(sizeof(ums_csw_t));

  RequestQueue(&csw_req_.value());
}

void UmsFunction::ContinueTransfer() {
  usb::FidlRequest* req = IsInData() ? &data_in_req_.value() : &data_out_req_.value();

  size_t length = std::min(data_remaining_, kDataReqSize);
  (*req)->data()->at(0).size(length);

  if (data_state_ == DATA_STATE_READ) {
    zx::result<std::vector<size_t>> result = req->CachedCopyTo(
        0, static_cast<char*>(storage_) + data_offset_, length, in_ep_.GetMapped());
    if (result.is_error() || result->size() != 1 || (*result)[0] != length) {
      fdf::error("ContinueTransfer: failed to copy read data");
      data_state_ = DATA_STATE_NONE;
      QueueCsw(CSW_FAILED);
      return;
    }
    QueueData(req);
  } else if (data_state_ == DATA_STATE_WRITE || data_state_ == DATA_STATE_UNMAP) {
    QueueData(req);
  } else {
    fdf::error("ContinueTransfer: bad data state {}", data_state_);
  }
}

void UmsFunction::StartTransferBlocks(DataState state, uint32_t transfer_blocks, uint64_t lba) {
  if (zx_status_t status = block::CheckIoRange(lba, transfer_blocks, kBlockCount, logger());
      status != ZX_OK) {
    fdf::error("StartTransfer: transfer out of range state: {}, lba: {} transfer_blocks: {}",
               static_cast<uint32_t>(state), lba, transfer_blocks);
    QueueCsw(CSW_FAILED);
    return;
  }

  data_offset_ = lba * kBlockSize;  // Not applicable for the DATA_STATE_UNMAP case.
  StartTransferBytes(state, static_cast<size_t>(transfer_blocks) * kBlockSize);
}

void UmsFunction::StartTransferBytes(DataState state, size_t transfer_bytes) {
  data_state_ = state;
  data_remaining_ =
      std::min(transfer_bytes, static_cast<size_t>(le32toh(current_cbw_.dCBWDataTransferLength)));

  ContinueTransfer();
}

zx::result<> UmsFunction::CancelAll(usb::EndpointClient<UmsFunction>& ep) {
  // For convenience, wrap the various FIDL error cases with a zx::result.
  fidl::WireResult result = ep.client().wire_sync()->CancelAll();
  if (!result.ok()) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (result->is_error()) {
    return result->take_error();
  }
  return zx::ok();
}

void UmsFunction::HandleInquiry(ums_cbw_t* cbw) {
  scsi::InquiryCDB cmd;
  memcpy(&cmd, cbw->CBWCB, sizeof(cmd));

  usb::FidlRequest* req = &data_in_req_.value();

  std::optional<zx_vaddr_t> vaddr = in_ep_.GetMappedAddr(req->request(), 0);
  if (!vaddr.has_value()) {
    fdf::error("{} GetMappedAddr failed", __func__);
    QueueCsw(CSW_FAILED);
    return;
  }
  auto* data = reinterpret_cast<void*>(*vaddr);

  uint32_t length = std::min(static_cast<uint32_t>(UMS_INQUIRY_TRANSFER_LENGTH),
                             le32toh(cbw->dCBWDataTransferLength));
  memset(data, 0, length);
  (*req)->data()->at(0).size(length);

  // fill in inquiry result
  if (!cmd.reserved_and_evpd) {
    scsi::InquiryData* inquiry_data = reinterpret_cast<scsi::InquiryData*>(data);
    inquiry_data->peripheral_device_type = 0;  // Peripheral Device Type: Direct access block device
    inquiry_data->removable = 0x80;            // Removable
    inquiry_data->version = 6;                 // Version SPC-4
    inquiry_data->response_data_format_and_control = 0x12;  // Response Data Format
    memcpy(inquiry_data->t10_vendor_id, "Google  ", 8);
    memcpy(inquiry_data->product_id, "Zircon UMS      ", 16);
    memcpy(inquiry_data->product_revision, "1.00", 4);
  } else {
    if (cmd.page_code == scsi::InquiryCDB::kPageListVpdPageCode) {
      auto vpd_page_list = reinterpret_cast<scsi::VPDPageList*>(data);
      vpd_page_list->peripheral_qualifier_device_type = 0;
      vpd_page_list->page_code = 0x00;
      vpd_page_list->page_length = 2;
      vpd_page_list->pages[0] = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
      vpd_page_list->pages[1] = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
    } else if (cmd.page_code == scsi::InquiryCDB::kBlockLimitsVpdPageCode) {
      auto block_limits = reinterpret_cast<scsi::VPDBlockLimits*>(data);
      block_limits->peripheral_qualifier_device_type = 0;
      block_limits->page_code = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
      block_limits->maximum_unmap_lba_count = htobe32(UINT32_MAX);
    } else if (cmd.page_code == scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode) {
      auto provisioning = reinterpret_cast<scsi::VPDLogicalBlockProvisioning*>(data);
      provisioning->peripheral_qualifier_device_type = 0;
      provisioning->page_code = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
      provisioning->set_lbpu(true);
      provisioning->set_provisioning_type(0x02);  // The logical unit is thin provisioned
    } else {
      fdf::error("Unsupported Inquiry page code=0x{:x}", cmd.page_code);
      return;
    }
  }

  req->CacheFlush(in_ep_.GetMapped());
  QueueData(req);
}

void UmsFunction::HandleTestUnitReady(ums_cbw_t* cbw) {
  // no data phase here. Just return status OK
  QueueCsw(CSW_SUCCESS);
}

void UmsFunction::HandleRequestSense(ums_cbw_t* cbw) {
  usb::FidlRequest* req = &data_in_req_.value();

  std::optional<zx_vaddr_t> vaddr = in_ep_.GetMappedAddr(req->request(), 0);
  if (!vaddr.has_value()) {
    fdf::error("{} GetMappedAddr failed", __func__);
    QueueCsw(CSW_FAILED);
    return;
  }
  auto* data = reinterpret_cast<scsi::SenseDataHeader*>(*vaddr);

  uint32_t length = std::min(static_cast<uint32_t>(UMS_REQUEST_SENSE_TRANSFER_LENGTH),
                             le32toh(cbw->dCBWDataTransferLength));
  memset(data, 0, length);
  (*req)->data()->at(0).size(length);

  data->response_code =
      static_cast<uint8_t>(scsi::SenseDataResponseCodes::kFixedCurrentInformation);
  data->additional_sense_code = 0x20;
  data->additional_sense_length = 10;

  req->CacheFlush(in_ep_.GetMapped());
  QueueData(req);
}

void UmsFunction::HandleReadCapacity10(ums_cbw_t* cbw) {
  usb::FidlRequest* req = &data_in_req_.value();

  std::optional<zx_vaddr_t> vaddr = in_ep_.GetMappedAddr(req->request(), 0);
  if (!vaddr.has_value()) {
    fdf::error("{} GetMappedAddr failed", __func__);
    QueueCsw(CSW_FAILED);
    return;
  }
  auto* data = reinterpret_cast<scsi::ReadCapacity10ParameterData*>(*vaddr);

  uint64_t lba = kBlockCount - 1;
  if (lba > UINT32_MAX) {
    data->returned_logical_block_address = htobe32(UINT32_MAX);
  } else {
    data->returned_logical_block_address = htobe32(lba);
  }
  data->block_length_in_bytes = htobe32(kBlockSize);

  uint32_t length =
      std::min(static_cast<uint32_t>(sizeof(*data)), le32toh(cbw->dCBWDataTransferLength));
  (*req)->data()->at(0).size(length);
  req->CacheFlush(in_ep_.GetMapped());
  QueueData(req);
}

void UmsFunction::HandleReadCapacity16(ums_cbw_t* cbw) {
  usb::FidlRequest* req = &data_in_req_.value();

  std::optional<zx_vaddr_t> vaddr = in_ep_.GetMappedAddr(req->request(), 0);
  if (!vaddr.has_value()) {
    fdf::error("{} GetMappedAddr failed", __func__);
    QueueCsw(CSW_FAILED);
    return;
  }
  auto* data = reinterpret_cast<scsi::ReadCapacity16ParameterData*>(*vaddr);

  memset(data, 0, sizeof(*data));
  data->returned_logical_block_address = htobe64(kBlockCount - 1);
  data->block_length_in_bytes = htobe32(kBlockSize);

  uint32_t length =
      std::min(static_cast<uint32_t>(sizeof(*data)), le32toh(cbw->dCBWDataTransferLength));
  (*req)->data()->at(0).size(length);

  req->CacheFlush(in_ep_.GetMapped());
  QueueData(req);
}

void UmsFunction::HandleModeSense6(ums_cbw_t* cbw) {
  usb::FidlRequest* req = &data_in_req_.value();

  std::optional<zx_vaddr_t> vaddr = in_ep_.GetMappedAddr(req->request(), 0);
  if (!vaddr.has_value()) {
    fdf::error("{} GetMappedAddr failed", __func__);
    QueueCsw(CSW_FAILED);
    return;
  }
  auto* data = reinterpret_cast<scsi::Mode6ParameterHeader*>(*vaddr);

  if (le32toh(cbw->dCBWDataTransferLength) < sizeof(scsi::Mode6ParameterHeader)) {
    fdf::error("HandleModeSense6: buffer too small");
    QueueCsw(CSW_FAILED);
    return;
  }

  auto* cdb = reinterpret_cast<scsi::ModeSense6CDB*>(cbw->CBWCB);
  uint8_t page_code = static_cast<uint8_t>(cdb->page_code());
  uint32_t allocation_length = le32toh(cbw->dCBWDataTransferLength);
  memset(data, 0, sizeof(*data));

  (*req)->data()->at(0).size(sizeof(scsi::Mode6ParameterHeader));

  if ((page_code == static_cast<uint8_t>(scsi::PageCode::kCachingPageCode) ||
       page_code == static_cast<uint8_t>(scsi::PageCode::kAllPageCode)) &&
      allocation_length >= sizeof(scsi::Mode6ParameterHeader) + sizeof(scsi::CachingModePage)) {
    auto caching_page = reinterpret_cast<scsi::CachingModePage*>(
        reinterpret_cast<uint8_t*>(data) + sizeof(scsi::Mode6ParameterHeader));
    memset(caching_page, 0, sizeof(*caching_page));
    caching_page->ps_spf_and_page_code = static_cast<uint8_t>(scsi::PageCode::kCachingPageCode);
    caching_page->page_length = sizeof(*caching_page) - 2;
    caching_page->set_write_cache_enabled(true);
    (*req)->data()->at(0).size(sizeof(scsi::Mode6ParameterHeader) + sizeof(scsi::CachingModePage));
  }
  data->mode_data_length = static_cast<uint8_t>((*req)->data()->at(0).size().value() - 1);

  req->CacheFlush(in_ep_.GetMapped());
  QueueData(req);
}

void UmsFunction::HandleRead10(ums_cbw_t* cbw) {
  scsi::Read10CDB* command = reinterpret_cast<scsi::Read10CDB*>(cbw->CBWCB);
  uint64_t lba = be32toh(command->logical_block_address);
  uint32_t blocks = be16toh(command->transfer_length);
  StartTransferBlocks(DATA_STATE_READ, blocks, lba);
}

void UmsFunction::HandleRead12(ums_cbw_t* cbw) {
  scsi::Read12CDB* command = reinterpret_cast<scsi::Read12CDB*>(cbw->CBWCB);
  uint64_t lba = be32toh(command->logical_block_address);
  uint32_t blocks = be32toh(command->transfer_length);
  StartTransferBlocks(DATA_STATE_READ, blocks, lba);
}

void UmsFunction::HandleRead16(ums_cbw_t* cbw) {
  scsi::Read16CDB* command = reinterpret_cast<scsi::Read16CDB*>(cbw->CBWCB);
  uint64_t lba = be64toh(command->logical_block_address);
  uint32_t blocks = be32toh(command->transfer_length);
  StartTransferBlocks(DATA_STATE_READ, blocks, lba);
}

void UmsFunction::HandleWrite10(ums_cbw_t* cbw) {
  scsi::Write10CDB* command = reinterpret_cast<scsi::Write10CDB*>(cbw->CBWCB);
  uint64_t lba = be32toh(command->logical_block_address);
  uint32_t blocks = be16toh(command->transfer_length);
  StartTransferBlocks(DATA_STATE_WRITE, blocks, lba);
}

void UmsFunction::HandleWrite12(ums_cbw_t* cbw) {
  scsi::Write12CDB* command = reinterpret_cast<scsi::Write12CDB*>(cbw->CBWCB);
  uint64_t lba = be32toh(command->logical_block_address);
  uint32_t blocks = be32toh(command->transfer_length);
  StartTransferBlocks(DATA_STATE_WRITE, blocks, lba);
}

void UmsFunction::HandleWrite16(ums_cbw_t* cbw) {
  scsi::Write16CDB* command = reinterpret_cast<scsi::Write16CDB*>(cbw->CBWCB);
  uint64_t lba = be64toh(command->logical_block_address);
  uint32_t blocks = be32toh(command->transfer_length);
  StartTransferBlocks(DATA_STATE_WRITE, blocks, lba);
}

void UmsFunction::HandleUnmap(ums_cbw_t* cbw) {
  scsi::UnmapCDB* command = reinterpret_cast<scsi::UnmapCDB*>(cbw->CBWCB);
  uint16_t unmap_data_length =
      sizeof(scsi::UnmapParameterListHeader) + sizeof(scsi::UnmapBlockDescriptor);
  if (betoh16(command->parameter_list_length) != unmap_data_length) {
    fdf::error("Command parameter list length is invalid: {} != {}",
               betoh16(command->parameter_list_length), unmap_data_length);
    QueueCsw(CSW_FAILED);
    return;
  }

  StartTransferBytes(DATA_STATE_UNMAP, unmap_data_length);
}

void UmsFunction::HandleCbw(ums_cbw_t* cbw) {
  if (le32toh(cbw->dCBWSignature) != CBW_SIGNATURE) {
    fdf::error("HandleCbw: bad dCBWSignature 0x{:x}", le32toh(cbw->dCBWSignature));
    return;
  }

  // reset data length and state for computing residue
  data_length_ = 0;
  data_state_ = DATA_STATE_NONE;

  // all SCSI commands have opcode in the first byte.
  auto opcode = static_cast<scsi::Opcode>(cbw->CBWCB[0]);
  switch (opcode) {
    case scsi::Opcode::INQUIRY:
      HandleInquiry(cbw);
      break;
    case scsi::Opcode::TEST_UNIT_READY:
      HandleTestUnitReady(cbw);
      break;
    case scsi::Opcode::REQUEST_SENSE:
      HandleRequestSense(cbw);
      break;
    case scsi::Opcode::READ_CAPACITY_10:
      HandleReadCapacity10(cbw);
      break;
    case scsi::Opcode::READ_CAPACITY_16:
      HandleReadCapacity16(cbw);
      break;
    case scsi::Opcode::MODE_SENSE_6:
      HandleModeSense6(cbw);
      break;
    case scsi::Opcode::READ_10:
      HandleRead10(cbw);
      break;
    case scsi::Opcode::READ_12:
      HandleRead12(cbw);
      break;
    case scsi::Opcode::READ_16:
      HandleRead16(cbw);
      break;
    case scsi::Opcode::WRITE_10:
      HandleWrite10(cbw);
      break;
    case scsi::Opcode::WRITE_12:
      HandleWrite12(cbw);
      break;
    case scsi::Opcode::WRITE_16:
      HandleWrite16(cbw);
      break;
    case scsi::Opcode::SYNCHRONIZE_CACHE_10:
      // TODO: This is presently untestable.
      // Implement this once we have a means of testing this.
      QueueCsw(CSW_SUCCESS);
      break;
    case scsi::Opcode::UNMAP:
      HandleUnmap(cbw);
      break;
    default:
      fdf::error("HandleCbw: unsupported opcode {:02X}h", cbw->CBWCB[0]);
      if (cbw->dCBWDataTransferLength) {
        // queue zero length packet to satisfy data phase
        usb::FidlRequest* req = &data_in_req_.value();
        (*req)->data()->at(0).size(0);
        data_state_ = DATA_STATE_FAILED;
        req->CacheFlush(in_ep_.GetMapped());
        QueueData(req);
      }
      QueueCsw(CSW_FAILED);
      break;
  }
}

void UmsFunction::CbwComplete(fendpoint::Completion completion) {
  bool online = configured_ && active_.load();
  if (!online) {
    fdf::error("UmsFunction: Not online, dropping CBW");
    return;
  }

  if (*completion.status() != ZX_OK) {
    fdf::error("UmsFunction: CBW receive failed: {}", zx_status_get_string(*completion.status()));
    return;
  }

  if (*completion.transfer_size() != sizeof(ums_cbw_t)) {
    fdf::error("Malformed CBW, size {} != {}", *completion.transfer_size(), sizeof(ums_cbw_t));
    return;
  }

  usb::FidlRequest* req = &cbw_req_.value();

  ums_cbw_t* cbw = &current_cbw_;
  memset(cbw, 0, sizeof(*cbw));
  zx::result<std::vector<size_t>> result =
      req->CachedCopyFrom(0, cbw, sizeof(*cbw), out_ep_.GetMapped());
  if (result.is_error() || result->size() != 1 || (*result)[0] != sizeof(*cbw)) {
    fdf::error("Failed to copy CBW: {}", result.status_string());
    QueueCsw(CSW_FAILED);
    return;
  }
  HandleCbw(cbw);
}

void UmsFunction::DataComplete(fendpoint::Completion completion) {
  bool online = configured_ && active_.load();
  if (!online) {
    return;
  }

  size_t actual = *completion.transfer_size();

  if (*completion.status() != ZX_OK) {
    fdf::error("UmsFunction: Data transfer failed: {}", zx_status_get_string(*completion.status()));
    data_state_ = DATA_STATE_NONE;
    QueueCsw(CSW_FAILED);
    return;
  }

  usb::FidlRequest* req = IsInData() ? &data_in_req_.value() : &data_out_req_.value();

  if (data_state_ == DATA_STATE_WRITE) {
    zx::result<std::vector<size_t>> result = req->CachedCopyFrom(
        0, static_cast<char*>(storage_) + data_offset_, actual, out_ep_.GetMapped());
    if (result.is_error() || result->size() != 1 || (*result)[0] != *completion.transfer_size()) {
      fdf::error("DataComplete: failed to copy write data");
      data_state_ = DATA_STATE_NONE;
      QueueCsw(CSW_FAILED);
      return;
    }
  } else if (data_state_ == DATA_STATE_UNMAP) {
    if (actual < sizeof(scsi::UnmapParameterListHeader) + sizeof(scsi::UnmapBlockDescriptor)) {
      fdf::error("DataComplete: UNMAP actual size {} too small", actual);
      data_state_ = DATA_STATE_NONE;
      QueueCsw(CSW_FAILED);
      return;
    }

    // Overwrite the unmapped blocks with zeros.
    req->CacheFlushInvalidate(out_ep_.GetMapped());

    std::optional<zx_vaddr_t> vaddr = out_ep_.GetMappedAddr(req->request(), 0);
    if (!vaddr.has_value()) {
      fdf::error("DataComplete: GetMappedAddr");
      QueueCsw(CSW_FAILED);
      return;
    }
    auto* data = reinterpret_cast<uint8_t*>(*vaddr);

    scsi::UnmapBlockDescriptor* block_descriptor = reinterpret_cast<scsi::UnmapBlockDescriptor*>(
        data + sizeof(scsi::UnmapParameterListHeader));
    uint32_t block_count = betoh32(block_descriptor->blocks);
    uint64_t start_lba = betoh64(block_descriptor->logical_block_address);
    if (zx_status_t status = block::CheckIoRange(start_lba, block_count, kBlockCount, logger());
        status != ZX_OK) {
      fdf::error("DataComplete: UNMAP out of bounds: start_lba={} block_count={}", start_lba,
                 block_count);
      data_state_ = DATA_STATE_NONE;
      QueueCsw(CSW_FAILED);
      return;
    }
    memset(static_cast<char*>(storage_) + (start_lba * kBlockSize), 0,
           static_cast<size_t>(block_count) * kBlockSize);
  } else if (data_state_ == DATA_STATE_FAILED) {
    data_state_ = DATA_STATE_NONE;
    QueueCsw(CSW_FAILED);
    return;
  } else {
    data_state_ = DATA_STATE_NONE;
    QueueCsw(CSW_SUCCESS);
    return;
  }

  data_offset_ += actual;
  if (data_remaining_ > actual) {
    data_remaining_ -= actual;
  } else {
    data_remaining_ = 0;
  }

  if (data_remaining_ > 0) {
    ContinueTransfer();
  } else {
    data_state_ = DATA_STATE_NONE;
    QueueCsw(CSW_SUCCESS);
  }
}

void UmsFunction::CswComplete(fendpoint::Completion completion) {
  zx_status_t status = *completion.status();
  if (status != ZX_OK) {
    fdf::error("UmsFunction: CSW send failed: {}", zx_status_get_string(status));
  }
}

void UmsFunction::Stop(fdf::StopCompleter completer) {
  zx::result cancel = CancelAll(in_ep_);
  if (cancel.is_error()) {
    fdf::error("Error canceling existing in-type requests: {}", cancel);
  }

  cancel = CancelAll(out_ep_);
  if (cancel.is_error()) {
    fdf::error("Error canceling existing out-type requests: {}", cancel);
  }

  active_ = false;

  if (storage_) {
    zx_vmar_unmap(zx_vmar_root_self(), (uintptr_t)storage_, kStorageSize);
    storage_ = nullptr;
  }

  if (pending_request_count_ == 0) {
    dispatcher_.ShutdownAsync();
    completer(zx::ok());
  } else {
    stop_completer_.emplace(std::move(completer));
  }
}
zx::vmo UmsFunction::vmo_ = zx::vmo();

zx::result<> UmsFunction::Start(fdf::DriverContext context) {
  zx_status_t status = Init(context);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  ffdf::DevfsAddArgs devfs_args{};
  std::vector<ffdf::NodeProperty> props{};
  std::vector<ffdf::Offer> offers{};

  zx::result start = AddChild(name(), props, offers);
  if (start.is_error()) {
    fdf::error("AddChild(): {}", start);
    return start.take_error();
  }

  return zx::ok();
}

zx_status_t UmsFunction::Init(fdf::DriverContext& context) {
  zx::result function =
      context.incoming().Connect<fuchsia_hardware_usb_function::UsbFunctionService::Device>();
  if (function.is_error()) {
    fdf::error("Could not connect to UsbFunction service: {}", function.error_value());
    return function.error_value();
  }
  function_.Bind(std::move(*function));

  data_state_ = DATA_STATE_NONE;
  active_ = true;
  pending_request_count_ = 0;

  // Server-end consumed by AllocResources(), client-end consumed by ep client Init().
  auto [in_client, in_server] = fidl::Endpoints<fendpoint::Endpoint>::Create();
  auto [out_client, out_server] = fidl::Endpoints<fendpoint::Endpoint>::Create();

  std::vector<ffunction::EndpointResource> ep_resources;
  ep_resources.emplace_back(fdescriptor::EndpointDirection::kIn, std::move(in_server),
                            fendpoint::EndpointInfo::WithBulk({}), kBulkMaxPacket);
  ep_resources.emplace_back(fdescriptor::EndpointDirection::kOut, std::move(out_server),
                            fendpoint::EndpointInfo::WithBulk({}), kBulkMaxPacket);

  fidl::Result alloc = function_->AllocResources({{
      .interface_count = 1,
      .endpoints = std::move(ep_resources),
      .strings = std::vector<std::string>{},
  }});
  if (alloc.is_error()) {
    fdf::error("Unable to allocate USB resources: {}", alloc.error_value());
    return alloc.error_value().is_domain_error() ? alloc.error_value().domain_error()
                                                 : ZX_ERR_INTERNAL;
  }

  if (alloc.value().interface_nums().size() != 1) {
    fdf::error("Unable to allocate USB interface");
    return ZX_ERR_INTERNAL;
  }

  if (alloc.value().endpoint_addrs().size() != 2) {
    fdf::error("Unable to allocate USB endpoints");
    return ZX_ERR_INTERNAL;
  }

  config_.intf.b_interface_number = alloc.value().interface_nums()[0];
  config_.in_ep.b_endpoint_address = alloc.value().endpoint_addrs()[0];
  config_.out_ep.b_endpoint_address = alloc.value().endpoint_addrs()[1];

  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "ums-fidl-dispatcher",
      [](fdf_dispatcher_t*) {}, "");
  if (dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher: {}", dispatcher.status_string());
    return dispatcher.error_value();
  }
  dispatcher_ = std::move(*dispatcher);

  zx_status_t status = in_ep_.Init(std::move(in_client), dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init IN endpoint: {}", zx_status_get_string(status));
    return status;
  }

  status = out_ep_.Init(std::move(out_client), dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init OUT endpoint: {}", zx_status_get_string(status));
    return status;
  }

  size_t actual = out_ep_.AddRequests(1, kBulkMaxPacket, frequest::Buffer::Tag::kVmoId);
  if (actual != 1) {
    fdf::error("Failed to allocate CBW request");
    return ZX_ERR_INTERNAL;
  }
  cbw_req_.emplace(*out_ep_.GetRequest());
  cbw_vmo_id_ = (*cbw_req_)->data()->at(0).buffer()->vmo_id().value();

  actual = in_ep_.AddRequests(1, kDataReqSize, frequest::Buffer::Tag::kVmoId);
  if (actual != 1) {
    fdf::error("Failed to allocate in-data request");
    return ZX_ERR_INTERNAL;
  }
  data_in_req_.emplace(*in_ep_.GetRequest());

  actual = out_ep_.AddRequests(1, kDataReqSize, frequest::Buffer::Tag::kVmoId);
  if (actual != 1) {
    fdf::error("Failed to allocate out-data request");
    return ZX_ERR_INTERNAL;
  }
  data_out_req_.emplace(*out_ep_.GetRequest());

  actual = in_ep_.AddRequests(1, kBulkMaxPacket, frequest::Buffer::Tag::kVmoId);
  if (actual != 1) {
    fdf::error("Failed to allocate CSW request");
    return ZX_ERR_INTERNAL;
  }
  csw_req_.emplace(*in_ep_.GetRequest());
  csw_vmo_id_ = (*csw_req_)->data()->at(0).buffer()->vmo_id().value();

  // create and map a VMO
  if (!vmo_.is_valid()) {
    status = vmo_.create(kStorageSize, 0, &vmo_);
    if (status != ZX_OK) {
      return status;
    }
  }
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo_, 0, kStorageSize,
                                      (zx_vaddr_t*)&storage_);
  if (status != ZX_OK) {
    return status;
  }

  const auto* descriptors_ptr = reinterpret_cast<const uint8_t*>(&config_);
  std::vector<uint8_t> config_descriptor(descriptors_ptr, descriptors_ptr + sizeof(config_));

  zx::result iface = fidl::CreateEndpoints<ffunction::UsbFunctionInterface>();
  if (iface.is_error()) {
    fdf::error("Could not create interface endpoints: {}", iface);
    return iface.error_value();
  }
  bindings_.AddBinding(this->dispatcher(), std::move(iface->server), this,
                       fidl::kIgnoreBindingClosure);

  fidl::Result cfg = function_->Configure(
      {{.configuration = std::move(config_descriptor), .iface = std::move(iface->client)}});
  if (cfg.is_error()) {
    fdf::error("Could not Configure(): {}", cfg.error_value());
    return cfg.error_value().is_domain_error() ? cfg.error_value().domain_error() : ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

}  // namespace ums

// clang-format off
FUCHSIA_DRIVER_EXPORT2(ums::UmsFunction);
