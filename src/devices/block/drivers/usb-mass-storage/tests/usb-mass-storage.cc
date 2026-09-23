// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../usb-mass-storage.h"

#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>

#include <thread>
#include <variant>

#include <fbl/array.h>
#include <fbl/intrusive_double_list.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

constexpr uint8_t kBlockSize = 5;

// Mock device based on code from ums-function.c

using usb_descriptor = std::variant<usb_interface_descriptor_t, usb_endpoint_descriptor_t>;

const usb_descriptor kDescriptors[] = {
    // Interface descriptor
    usb_interface_descriptor_t{sizeof(usb_descriptor), USB_DT_INTERFACE, 0, 0, 2, 8, 7, 0x50, 0},
    // IN endpoint
    usb_endpoint_descriptor_t{sizeof(usb_descriptor), USB_DT_ENDPOINT, USB_DIR_IN,
                              static_cast<uint8_t>(fdescriptor::EndpointType::kBulk), 64, 0},
    // OUT endpoint
    usb_endpoint_descriptor_t{sizeof(usb_descriptor), USB_DT_ENDPOINT, USB_DIR_OUT,
                              static_cast<uint8_t>(fdescriptor::EndpointType::kBulk), 64, 0}};

struct Packet;
struct Packet : fbl::DoublyLinkedListable<fbl::RefPtr<Packet>>, fbl::RefCounted<Packet> {
  explicit Packet(fbl::Array<unsigned char>&& source) { data = std::move(source); }
  explicit Packet() { stall = true; }
  bool stall = false;
  fbl::Array<unsigned char> data;
};

class FakeTimer : public ums::WaiterInterface {
 public:
  zx_status_t Wait(sync_completion_t* completion, zx_duration_t duration) override {
    return timeout_handler_(completion, duration);
  }

  void set_timeout_handler(fit::function<zx_status_t(sync_completion_t*, zx_duration_t)> handler) {
    timeout_handler_ = std::move(handler);
  }

 private:
  fit::function<zx_status_t(sync_completion_t*, zx_duration_t)> timeout_handler_;
};

enum ErrorInjection {
  NoFault,
  RejectCacheCbw,
  RejectCacheDataStage,
};

constexpr auto kInitialTagValue = 8;

struct Context {
  fbl::DoublyLinkedList<fbl::RefPtr<Packet>> pending_packets;
  ums_csw_t csw;
  const usb_descriptor* descs;
  size_t desc_length;
  uint64_t transfer_offset;
  uint64_t transfer_blocks;
  scsi::Opcode transfer_type;
  uint8_t transfer_lun;
  size_t pending_write;
  ErrorInjection failure_mode;
  fbl::RefPtr<Packet> last_transfer;
  uint32_t tag = kInitialTagValue;
  fit::function<void(usb_request_t*)> on_request_queue;
  fit::function<void(const ums_cbw_t&)> on_cbw;
  std::optional<uint8_t> tur_csw_status;
};

class UsbBanjoServer : public ddk::UsbProtocol<UsbBanjoServer> {
 public:
  void SetContext(Context* context) { context_ = context; }

  compat::DeviceServer::BanjoConfig GetBanjoConfig() {
    compat::DeviceServer::BanjoConfig config{ZX_PROTOCOL_USB};
    config.callbacks[ZX_PROTOCOL_USB] = banjo_server_.callback();
    return config;
  }

  size_t UsbGetDescriptorsLength() { return context_->desc_length; }

  void UsbGetDescriptors(uint8_t* buffer, size_t size, size_t* outsize) {
    *outsize = context_->desc_length > size ? size : context_->desc_length;
    memcpy(buffer, context_->descs, *outsize);
  }

  zx_status_t UsbControlIn(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                           int64_t timeout, uint8_t* out_read_buffer, size_t read_size,
                           size_t* out_read_actual) {
    switch (request) {
      case USB_REQ_GET_MAX_LUN: {
        if (!read_size) {
          *out_read_actual = 0;
          return ZX_OK;
        }
        *reinterpret_cast<unsigned char*>(out_read_buffer) = 1;  // Max lun number
        *out_read_actual = 1;
        return ZX_OK;
      }
      default:
        return ZX_ERR_IO_REFUSED;
    }
  }

  size_t UsbGetMaxTransferSize(uint8_t ep) {
    switch (ep) {
      case USB_DIR_OUT:
        __FALLTHROUGH;
      case USB_DIR_IN:
        // 10MB transfer size (to test large transfers)
        // (is this even possible in real hardware?)
        return 1000 * 1000 * 10;
      default:
        return 0;
    }
  }
  size_t UsbGetRequestSize() { return sizeof(usb_request_t); }

  void UsbRequestQueue(usb_request_t* usb_request,
                       const usb_request_complete_callback_t* complete_cb) {
    if (context_->on_request_queue) {
      context_->on_request_queue(usb_request);
    }
    if (context_->pending_write) {
      void* data;
      usb_request_mmap(usb_request, &data);
      memcpy(context_->last_transfer->data.data(), data, context_->pending_write);
      usb_request->response.actual = context_->pending_write;
      context_->pending_write = 0;
      usb_request->response.status = ZX_OK;
      complete_cb->callback(complete_cb->ctx, usb_request);
      return;
    }
    if ((usb_request->header.ep_address & USB_ENDPOINT_DIR_MASK) == USB_ENDPOINT_IN) {
      if (context_->pending_packets.begin() == context_->pending_packets.end()) {
        usb_request->response.status = ZX_OK;
        complete_cb->callback(complete_cb->ctx, usb_request);
      } else {
        auto packet = context_->pending_packets.pop_front();
        if (packet->stall) {
          usb_request->response.actual = 0;
          usb_request->response.status = ZX_ERR_IO_REFUSED;
          complete_cb->callback(complete_cb->ctx, usb_request);
          return;
        }
        size_t len =
            usb_request->size < packet->data.size() ? usb_request->size : packet->data.size();
        size_t result = usb_request_copy_to(usb_request, packet->data.data(), len, 0);
        ZX_ASSERT(result == len);
        usb_request->response.actual = len;
        usb_request->response.status = ZX_OK;
        complete_cb->callback(complete_cb->ctx, usb_request);
      }
      return;
    }
    void* data;
    usb_request_mmap(usb_request, &data);
    uint32_t header;
    memcpy(&header, data, sizeof(header));
    header = le32toh(header);
    switch (header) {
      case CBW_SIGNATURE: {
        ums_cbw_t cbw;
        memcpy(&cbw, data, sizeof(cbw));
        if (context_->on_cbw) {
          context_->on_cbw(cbw);
        }
        if (cbw.bCBWLUN > 3) {
          usb_request->response.status = ZX_OK;
          complete_cb->callback(complete_cb->ctx, usb_request);
          return;
        }
        auto DataTransfer = [&]() {
          if ((context_->transfer_offset == cbw.bCBWLUN) &&
              (context_->transfer_blocks == (cbw.bCBWLUN + 1U) * 65534U)) {
            auto opcode = static_cast<scsi::Opcode>(cbw.CBWCB[0]);
            size_t transfer_length = context_->transfer_blocks * kBlockSize;
            fbl::Array<unsigned char> transfer(new unsigned char[transfer_length], transfer_length);
            context_->last_transfer = fbl::MakeRefCounted<Packet>(std::move(transfer));
            context_->transfer_lun = cbw.bCBWLUN;
            if ((opcode == scsi::Opcode::READ_10) || (opcode == scsi::Opcode::READ_12) ||
                (opcode == scsi::Opcode::READ_16)) {
              // Push reply
              context_->pending_packets.push_back(context_->last_transfer);
            } else {
              if ((opcode == scsi::Opcode::WRITE_10) || (opcode == scsi::Opcode::WRITE_12) ||
                  (opcode == scsi::Opcode::WRITE_16)) {
                context_->pending_write = transfer_length;
              }
            }
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            context_->csw.bmCSWStatus = CSW_SUCCESS;
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
          }
        };
        auto opcode = static_cast<scsi::Opcode>(cbw.CBWCB[0]);
        switch (opcode) {
          case scsi::Opcode::WRITE_16:
            __FALLTHROUGH;
          case scsi::Opcode::READ_16: {
            scsi::Read16CDB cmd;  // struct-wise equivalent to scsi::Write16CDB here.
            memcpy(&cmd, cbw.CBWCB, sizeof(cmd));
            context_->transfer_blocks = be32toh(cmd.transfer_length);
            context_->transfer_offset = be64toh(cmd.logical_block_address);
            context_->transfer_type = opcode;
            DataTransfer();
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::WRITE_12:
            __FALLTHROUGH;
          case scsi::Opcode::READ_12: {
            scsi::Read12CDB cmd;  // struct-wise equivalent to scsi::Write12CDB here.
            memcpy(&cmd, cbw.CBWCB, sizeof(cmd));
            context_->transfer_blocks = be32toh(cmd.transfer_length);
            context_->transfer_offset = be32toh(cmd.logical_block_address);
            context_->transfer_type = opcode;
            DataTransfer();
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::WRITE_10:
            __FALLTHROUGH;
          case scsi::Opcode::READ_10: {
            scsi::Read10CDB cmd;  // struct-wise equivalent to scsi::Write10CDB here.
            memcpy(&cmd, cbw.CBWCB, sizeof(cmd));
            context_->transfer_blocks = be16toh(cmd.transfer_length);
            context_->transfer_offset = be32toh(cmd.logical_block_address);
            context_->transfer_type = opcode;
            DataTransfer();
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::SYNCHRONIZE_CACHE_10: {
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            context_->csw.bmCSWStatus = CSW_SUCCESS;
            context_->transfer_lun = cbw.bCBWLUN;
            context_->transfer_type = opcode;
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::UNMAP: {
            fbl::Array<unsigned char> transfer(new unsigned char[24], 24);
            context_->last_transfer = fbl::MakeRefCounted<Packet>(std::move(transfer));
            context_->pending_write = 24;
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            context_->csw.bmCSWStatus = CSW_SUCCESS;
            context_->transfer_lun = cbw.bCBWLUN;
            context_->transfer_type = opcode;
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::INQUIRY: {
            scsi::InquiryCDB cmd;
            memcpy(&cmd, cbw.CBWCB, sizeof(cmd));

            // Push reply
            fbl::Array<unsigned char> reply(new unsigned char[UMS_INQUIRY_TRANSFER_LENGTH](),
                                            UMS_INQUIRY_TRANSFER_LENGTH);
            if (!cmd.reserved_and_evpd) {
              reply[0] = 0;     // Peripheral Device Type: Direct access block device
              reply[1] = 0x80;  // Removable
              reply[2] = 6;     // Version SPC-4
              reply[3] = 0x12;  // Response Data Format
              memcpy(reply.data() + 8, "Google  ", 8);
              memcpy(reply.data() + 16, "Zircon UMS      ", 16);
              memcpy(reply.data() + 32, "1.00", 4);
            } else {
              if (cmd.page_code == scsi::InquiryCDB::kPageListVpdPageCode) {
                auto vpd_page_list = reinterpret_cast<scsi::VPDPageList*>(reply.data());
                vpd_page_list->peripheral_qualifier_device_type = 0;
                vpd_page_list->page_code = 0x00;
                vpd_page_list->page_length = 2;
                vpd_page_list->pages[0] = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
                vpd_page_list->pages[1] = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
              } else if (cmd.page_code == scsi::InquiryCDB::kBlockLimitsVpdPageCode) {
                auto block_limits = reinterpret_cast<scsi::VPDBlockLimits*>(reply.data());
                block_limits->peripheral_qualifier_device_type = 0;
                block_limits->page_code = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
                block_limits->maximum_unmap_lba_count = htobe32(UINT32_MAX);
              } else if (cmd.page_code == scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode) {
                auto provisioning =
                    reinterpret_cast<scsi::VPDLogicalBlockProvisioning*>(reply.data());
                provisioning->peripheral_qualifier_device_type = 0;
                provisioning->page_code = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
                provisioning->set_lbpu(true);
                provisioning->set_provisioning_type(0x02);  // The logical unit is thin provisioned
              } else {
                usb_request->response.status = ZX_ERR_NOT_SUPPORTED;
                usb_request->response.actual = 0;
                complete_cb->callback(complete_cb->ctx, usb_request);
                return;
              }
            }
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(reply)));
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            context_->csw.bmCSWStatus = CSW_SUCCESS;
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));

            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::TEST_UNIT_READY: {
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            if (context_->tur_csw_status.has_value()) {
              context_->csw.bmCSWStatus = *context_->tur_csw_status;
            } else {
              context_->csw.bmCSWStatus = CSW_SUCCESS;
            }
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::READ_CAPACITY_16: {
            if (cbw.bCBWLUN == 1) {
              // Push reply
              fbl::Array<unsigned char> reply(
                  new unsigned char[sizeof(scsi::ReadCapacity16ParameterData)],
                  sizeof(scsi::ReadCapacity16ParameterData));
              scsi::ReadCapacity16ParameterData scsi;
              scsi.block_length_in_bytes = htobe32(kBlockSize);
              scsi.returned_logical_block_address =
                  htobe64((976562L * (1 + cbw.bCBWLUN)) + UINT32_MAX);
              memcpy(reply.data(), &scsi, sizeof(scsi));
              context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(reply)));
              // Push CSW
              fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)],
                                            sizeof(ums_csw_t));
              context_->csw.dCSWDataResidue = 0;
              context_->csw.dCSWTag = context_->tag++;
              context_->csw.bmCSWStatus = CSW_SUCCESS;
              memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
              context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
            }
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::READ_CAPACITY_10: {
            // Push reply
            fbl::Array<unsigned char> reply(
                new unsigned char[sizeof(scsi::ReadCapacity10ParameterData)],
                sizeof(scsi::ReadCapacity10ParameterData));
            scsi::ReadCapacity10ParameterData scsi;
            scsi.block_length_in_bytes = htobe32(kBlockSize);
            scsi.returned_logical_block_address = htobe32(976562 * (1 + cbw.bCBWLUN));
            if (cbw.bCBWLUN == 1) {
              scsi.returned_logical_block_address = -1;
            }
            memcpy(reply.data(), &scsi, sizeof(scsi));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(reply)));
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            context_->csw.bmCSWStatus = CSW_SUCCESS;
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          case scsi::Opcode::MODE_SENSE_6: {
            scsi::ModeSense6CDB cmd;
            memcpy(&cmd, cbw.CBWCB, sizeof(cmd));
            // Push reply
            switch (cmd.page_code()) {
              case scsi::PageCode::kAllPageCode: {
                fbl::Array<unsigned char> reply(
                    new unsigned char[sizeof(scsi::Mode6ParameterHeader)],
                    sizeof(scsi::Mode6ParameterHeader));
                scsi::Mode6ParameterHeader mode_page = {};
                mode_page.set_dpo_fua_available(true);
                mode_page.set_write_protected(false);
                memcpy(reply.data(), &mode_page, sizeof(mode_page));
                context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(reply)));
                // Push CSW
                fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)],
                                              sizeof(ums_csw_t));
                context_->csw.dCSWDataResidue = 0;
                context_->csw.dCSWTag = context_->tag++;
                context_->csw.bmCSWStatus = CSW_SUCCESS;
                memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
                context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
                usb_request->response.status = ZX_OK;
                complete_cb->callback(complete_cb->ctx, usb_request);
                break;
              }
              case scsi::PageCode::kCachingPageCode: {
                if (context_->failure_mode == RejectCacheCbw) {
                  usb_request->response.status = ZX_ERR_IO_REFUSED;
                  usb_request->response.actual = 0;
                  complete_cb->callback(complete_cb->ctx, usb_request);
                  return;
                }
                if (context_->failure_mode == RejectCacheDataStage) {
                  complete_cb->callback(complete_cb->ctx, usb_request);
                  context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>());
                  return;
                } else {
                  size_t mode_page_size =
                      sizeof(scsi::Mode6ParameterHeader) + sizeof(scsi::CachingModePage);
                  fbl::Array<unsigned char> reply(new unsigned char[mode_page_size],
                                                  mode_page_size);
                  scsi::CachingModePage mode_page = {};
                  mode_page.set_page_code(static_cast<uint8_t>(scsi::PageCode::kCachingPageCode));
                  mode_page.set_write_cache_enabled(true);
                  memset(reply.data(), 0, mode_page_size);
                  memcpy(reply.data() + sizeof(scsi::Mode6ParameterHeader), &mode_page,
                         sizeof(mode_page));
                  context_->pending_packets.push_back(
                      fbl::MakeRefCounted<Packet>(std::move(reply)));
                }
                // Push CSW
                fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)],
                                              sizeof(ums_csw_t));
                context_->csw.dCSWDataResidue = 0;
                context_->csw.dCSWTag = context_->tag++;
                context_->csw.bmCSWStatus = CSW_SUCCESS;
                memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
                context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
                usb_request->response.status = ZX_OK;
                complete_cb->callback(complete_cb->ctx, usb_request);
                break;
              }
              default:
                usb_request->response.status = ZX_OK;
                complete_cb->callback(complete_cb->ctx, usb_request);
            }
            break;
          }
          case scsi::Opcode::REQUEST_SENSE: {
            scsi::RequestSenseCDB cmd;
            memcpy(&cmd, cbw.CBWCB, sizeof(cmd));
            // Push reply
            fbl::Array<unsigned char> reply(new unsigned char[cmd.allocation_length],
                                            cmd.allocation_length);
            scsi::FixedFormatSenseDataHeader sense_data;
            sense_data.set_response_code(scsi::SenseDataResponseCodes::kFixedCurrentInformation);
            sense_data.set_valid(0);
            sense_data.set_sense_key(scsi::SenseKey::NO_SENSE);
            memcpy(reply.data(), &sense_data, sizeof(sense_data));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(reply)));
            // Push CSW
            fbl::Array<unsigned char> csw(new unsigned char[sizeof(ums_csw_t)], sizeof(ums_csw_t));
            context_->csw.dCSWDataResidue = 0;
            context_->csw.dCSWTag = context_->tag++;
            context_->csw.bmCSWStatus = CSW_SUCCESS;
            memcpy(csw.data(), &context_->csw, sizeof(context_->csw));
            context_->pending_packets.push_back(fbl::MakeRefCounted<Packet>(std::move(csw)));
            usb_request->response.status = ZX_OK;
            complete_cb->callback(complete_cb->ctx, usb_request);
            break;
          }
          default:
            // Unexpected SCSI command
            ADD_FAILURE();
        }
        break;
      }
      default:
        context_->csw.bmCSWStatus = CSW_FAILED;
        usb_request->response.status = ZX_ERR_IO;
        complete_cb->callback(complete_cb->ctx, usb_request);
    }
  }

  // Unimplemented methods.
  zx_status_t UsbControlOut(uint8_t request_type, uint8_t request, uint16_t value, uint16_t index,
                            zx_time_t timeout, const uint8_t* write_buffer, size_t write_size) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  usb_speed_t UsbGetSpeed() {
    ZX_ASSERT_MSG(0, "Unimplemented.");
    return 0;
  }
  zx_status_t UsbSetInterface(uint8_t interface_number, uint8_t alt_setting) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  uint8_t UsbGetConfiguration() {
    ZX_ASSERT_MSG(0, "Unimplemented.");
    return 0;
  }
  zx_status_t UsbSetConfiguration(uint8_t configuration) { return ZX_ERR_NOT_SUPPORTED; }
  zx_status_t UsbEnableEndpoint(const usb_endpoint_descriptor_t* ep_desc,
                                const usb_ss_ep_comp_descriptor_t* ss_com_desc, bool enable) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t UsbResetEndpoint(uint8_t ep_address) { return ZX_ERR_NOT_SUPPORTED; }
  zx_status_t UsbResetDevice() { return ZX_ERR_NOT_SUPPORTED; }
  uint32_t UsbGetDeviceId() {
    ZX_ASSERT_MSG(0, "Unimplemented.");
    return 0;
  }
  void UsbGetDeviceDescriptor(usb_device_descriptor_t* out_desc) {
    ZX_ASSERT_MSG(0, "Unimplemented.");
  }
  zx_status_t UsbGetConfigurationDescriptorLength(uint8_t configuration, uint64_t* out_length) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t UsbGetConfigurationDescriptor(uint8_t configuration, uint8_t* out_desc_buffer,
                                            size_t desc_size, size_t* out_desc_actual) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t UsbGetStringDescriptor(uint8_t desc_id, uint16_t lang_id, uint16_t* out_lang_id,
                                     uint8_t* out_string_buffer, size_t string_size,
                                     size_t* out_string_actual) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t UsbCancelAll(uint8_t ep_address) { return ZX_ERR_NOT_SUPPORTED; }
  uint64_t UsbGetCurrentFrame() {
    ZX_ASSERT_MSG(0, "Unimplemented.");
    return 0;
  }

 private:
  Context* context_;
  compat::BanjoServer banjo_server_{ZX_PROTOCOL_USB, this, &usb_protocol_ops_};
};

class TestUsbMassStorageDevice : public ums::UsbMassStorageDevice {
 public:
  explicit TestUsbMassStorageDevice() : UsbMassStorageDevice() {
    timer_ = fbl::MakeRefCounted<FakeTimer>();
    timer_->set_timeout_handler([&](sync_completion_t* completion, zx_duration_t duration) {
      if (duration == 0) {
        has_zero_duration_ = true;
      }
      if (duration == ZX_TIME_INFINITE) {
        return sync_completion_wait(completion, duration);
      }
      return ZX_OK;
    });
    set_waiter(timer_);
  }

  void Stop(fdf::StopCompleter completer) override {
    ASSERT_FALSE(has_zero_duration_);
    std::thread unpause_thread;
    if (on_stop_) {
      unpause_thread = std::thread([this] {
        while (!is_dead()) {
          zx::nanosleep(zx::deadline_after(zx::msec(1)));
        }
        on_stop_();
      });
    }
    UsbMassStorageDevice::Stop(std::move(completer));
    if (unpause_thread.joinable()) {
      unpause_thread.join();
    }
  }

  void set_on_stop(fit::function<void()> on_stop) { on_stop_ = std::move(on_stop); }
  FakeTimer& timer() { return *timer_; }

 private:
  fbl::RefPtr<FakeTimer> timer_;
  fit::function<void()> on_stop_;
  bool has_zero_duration_ = false;
};

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    usb_banjo_server_.SetContext(&context_);
    device_server_.Initialize(component::kDefaultInstance, std::nullopt,
                              usb_banjo_server_.GetBanjoConfig());
    return zx::make_result(
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs));
  }

  Context& context() { return context_; }

 private:
  Context context_;
  UsbBanjoServer usb_banjo_server_;
  compat::DeviceServer device_server_;
};

class TestConfig final {
 public:
  using DriverType = TestUsbMassStorageDevice;
  using EnvironmentType = Environment;
};

class UmsTest : public ::testing::Test {
 public:
  void SetUp() override {
    driver_stopped_ = false;
    driver_ = nullptr;
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      env.context().pending_write = 0;
      env.context().csw.dCSWSignature = htole32(CSW_SIGNATURE);
      env.context().csw.bmCSWStatus = CSW_SUCCESS;
      env.context().descs = kDescriptors;
      env.context().desc_length = sizeof(kDescriptors);
      env.context().on_request_queue = nullptr;
      env.context().on_cbw = nullptr;
      env.context().tur_csw_status = std::nullopt;
    });
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  void TearDown() override {
    if (!driver_stopped_) {
      zx::result<> result = driver_test().StopDriver();
      ASSERT_OK(result);
    }
  }

 protected:
  bool driver_stopped_ = false;
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  TestUsbMassStorageDevice* driver_ = nullptr;

  void StartDriver(ErrorInjection inject_failure = NoFault) {
    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      // Device parameters for physical (parent) device
      env.context().failure_mode = inject_failure;
    });

    zx::result<> result = driver_test().StartDriver();
    ASSERT_OK(result);

    size_t num_devs = 0;
    driver_test().RunInDriverContext([&](TestUsbMassStorageDevice& driver) {
      driver_ = &driver;
      num_devs = driver.block_devs().size();
    });
    ASSERT_GE(num_devs, size_t{2});
  }

  std::vector<std::string> GetDeviceNames() {
    std::vector<std::string> names;
    driver_test().RunInDriverContext([&](TestUsbMassStorageDevice& driver) {
      for (const auto& block_dev : driver.block_devs()) {
        if (block_dev) {
          names.push_back(block_dev->DeviceName().c_str());
        }
      }
    });
    return names;
  }
};

// UMS read test
// This test validates the read functionality on multiple LUNS of a USB mass storage device.
TEST_F(UmsTest, TestRead) {
  StartDriver();
  fzl::VmoMapper mapper;
  zx::vmo vmo;
  ASSERT_OK(mapper.CreateAndMap(70 * 1024 * 1024, ZX_VM_PERM_READ, nullptr, &vmo));

  // Perform read transactions
  uint32_t lun = 0;
  for (const std::string& dev_name : GetDeviceNames()) {
    auto client_end =
        driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_name.c_str());
    ASSERT_OK(client_end);
    auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

    storage::Vmoid owned_vmoid;
    ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

    const uint64_t length = (lun + 1) * 65534;
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = owned_vmoid.get(),
        .length = static_cast<uint32_t>(length),
        .vmo_offset = 0,
        .dev_offset = lun,
    };

    ASSERT_OK(client->FifoTransaction(&request, 1));

    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      EXPECT_EQ(lun, env.context().transfer_lun);
      const scsi::Opcode xfer_type = lun == 1 ? scsi::Opcode::READ_16 : scsi::Opcode::READ_10;
      EXPECT_EQ(xfer_type, env.context().transfer_type);
      EXPECT_EQ(0, memcmp(mapper.start(), env.context().last_transfer->data.data(),
                          env.context().last_transfer->data.size()));
    });

    ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
    ++lun;
  }
}

// This test validates the write functionality on multiple LUNS of a USB mass storage device.
TEST_F(UmsTest, TestWrite) {
  StartDriver();
  fzl::VmoMapper mapper;
  zx::vmo vmo;
  ASSERT_OK(
      mapper.CreateAndMap(70 * 1024 * 1024, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  // Add "entropy" for write operation
  for (size_t i = 0; i < mapper.size() / sizeof(size_t); i++) {
    reinterpret_cast<size_t*>(mapper.start())[i] = i;
  }
  // Perform write transactions
  uint32_t lun = 0;
  for (const std::string& dev_name : GetDeviceNames()) {
    auto client_end =
        driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_name.c_str());
    ASSERT_OK(client_end);
    auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

    storage::Vmoid owned_vmoid;
    ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

    const uint64_t length = (lun + 1) * 65534;
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_WRITE},
        .vmoid = owned_vmoid.get(),
        .length = static_cast<uint32_t>(length),
        .vmo_offset = 0,
        .dev_offset = lun,
    };

    ASSERT_OK(client->FifoTransaction(&request, 1));

    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      EXPECT_EQ(lun, env.context().transfer_lun);
      const scsi::Opcode xfer_type = lun == 1 ? scsi::Opcode::WRITE_16 : scsi::Opcode::WRITE_10;
      EXPECT_EQ(xfer_type, env.context().transfer_type);
      EXPECT_EQ(
          0, memcmp(mapper.start(), env.context().last_transfer->data.data(), length * kBlockSize));
    });

    ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
    ++lun;
  }
}

// This test validates the flush functionality on multiple LUNS of a USB mass storage device.
TEST_F(UmsTest, TestFlush) {
  StartDriver();

  // Perform flush transactions
  uint32_t lun = 0;
  for (const std::string& dev_name : GetDeviceNames()) {
    auto client_end =
        driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_name.c_str());
    ASSERT_OK(client_end);
    auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_FLUSH},
        .vmoid = BLOCK_VMOID_INVALID,
        .length = 0,
        .vmo_offset = 0,
        .dev_offset = 0,
    };

    ASSERT_OK(client->FifoTransaction(&request, 1));

    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      EXPECT_EQ(lun, env.context().transfer_lun);
      EXPECT_EQ(scsi::Opcode::SYNCHRONIZE_CACHE_10, env.context().transfer_type);
    });
    ++lun;
  }
}

// This test validates the trim functionality on multiple LUNS of a USB mass storage device.
TEST_F(UmsTest, TestTrim) {
  StartDriver();

  // Perform trim transactions
  uint32_t lun = 0;
  for (const std::string& dev_name : GetDeviceNames()) {
    auto client_end =
        driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_name.c_str());
    ASSERT_OK(client_end);
    auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_TRIM},
        .vmoid = BLOCK_VMOID_INVALID,
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };

    ASSERT_OK(client->FifoTransaction(&request, 1));

    driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
      EXPECT_EQ(lun, env.context().transfer_lun);
      EXPECT_EQ(scsi::Opcode::UNMAP, env.context().transfer_type);
    });
    ++lun;
  }
}

TEST_F(UmsTest, CbwStallDoesNotFreezeDriver) { StartDriver(RejectCacheCbw); }

TEST_F(UmsTest, DataStageStallDoesNotFreezeDriver) { StartDriver(RejectCacheDataStage); }

// Verifies that pending transactions are drained and completed during driver shutdown,
// preventing a Use-After-Free or assertion failure when the block devices are destroyed.
TEST_F(UmsTest, PendingTransactionsDrainedOnDriverShutdown) {
  StartDriver();

  std::vector<std::string> dev_names = GetDeviceNames();
  ASSERT_FALSE(dev_names.empty());

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_names[0].c_str());
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  libsync::Completion request_started;
  std::atomic<bool> allow_resume = false;
  std::atomic<bool> request_paused = false;

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.context().on_request_queue = [&](usb_request_t* req) {
      if (!request_paused.exchange(true)) {
        request_started.Signal();
        while (!allow_resume.load()) {
          zx::nanosleep(zx::deadline_after(zx::msec(1)));
        }
      }
    };
  });

  driver_test().RunInDriverContext([&](TestUsbMassStorageDevice& driver) {
    driver.set_on_stop([&] { allow_resume.store(true); });
  });

  BlockFifoRequest requests[2] = {
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
          .vmoid = BLOCK_VMOID_INVALID,
          .length = 0,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
          .vmoid = BLOCK_VMOID_INVALID,
          .length = 0,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
  };

  std::atomic<zx_status_t> txn_status = ZX_OK;
  std::thread client_thread([&] { txn_status = client->FifoTransaction(requests, 2); });

  // Wait for the first request to enter the USB queue and pause.
  request_started.Wait();

  // Verify that the second request is queued in queued_txns_.
  EXPECT_GE(driver_->queued_txns_count(), 1u);

  // Stop the driver. This marks the driver as dead, unpauses the in-flight request via on_stop,
  // drains the remaining queued request(s) before destroying the block devices, and shuts down.
  zx::result<> stop_result = driver_test().StopDriver();
  EXPECT_OK(stop_result);
  driver_stopped_ = true;

  client_thread.join();
  EXPECT_STATUS(txn_status.load(), ZX_ERR_IO_NOT_PRESENT);
}

// Verifies that when a LUN becomes unready and is torn down in CheckLunsReady,
// any pending transactions for that LUN in queued_txns_ are drained and completed
// before the BlockDevice is destroyed, preventing a Use-After-Free.
TEST_F(UmsTest, PendingTransactionsDrainedOnLunShutdown) {
  StartDriver();

  std::vector<std::string> dev_names = GetDeviceNames();
  ASSERT_GE(dev_names.size(), 2u);

  auto client1_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_names[1].c_str());
  ASSERT_OK(client1_end);
  auto client1 = block_client::RemoteBlockDevice::Create(std::move(client1_end.value())).value();

  // Prevent WorkerLoop from spinning and periodically invoking CheckLunsReady.
  driver_test().RunInDriverContext([&](TestUsbMassStorageDevice& driver) {
    driver.timer().set_timeout_handler([](sync_completion_t* completion, zx_duration_t duration) {
      return sync_completion_wait(completion, ZX_TIME_INFINITE);
    });
  });

  libsync::Completion tur_started;
  std::atomic<bool> allow_tur_resume = false;

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.context().on_cbw = [&](const ums_cbw_t& cbw) {
      if (cbw.bCBWLUN == 1 &&
          static_cast<scsi::Opcode>(cbw.CBWCB[0]) == scsi::Opcode::TEST_UNIT_READY) {
        env.context().tur_csw_status = CSW_FAILED;
        tur_started.Signal();
        while (!allow_tur_resume.load()) {
          zx::nanosleep(zx::deadline_after(zx::msec(1)));
        }
      }
    };
  });

  // Trigger CheckLunsReady on a background thread. It will run TestUnitReady for LUN 1,
  // trigger on_cbw, and pause while holding luns_lock_.
  std::thread check_thread([&] {
    driver_test().RunInDriverContext(
        [&](TestUsbMassStorageDevice& driver) { EXPECT_OK(driver.CheckLunsReady()); });
  });

  tur_started.Wait();

  // While CheckLunsReady is paused holding luns_lock_, queue a transaction on LUN 1.
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_FLUSH},
      .vmoid = BLOCK_VMOID_INVALID,
      .length = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  std::atomic<zx_status_t> txn_status = ZX_OK;
  std::thread client_thread([&] { txn_status = client1->FifoTransaction(&request, 1); });

  // Wait until the request is queued in queued_txns_.
  while (driver_->queued_txns_count() == 0) {
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
  EXPECT_GE(driver_->queued_txns_count(), 1u);

  // Resume TestUnitReady. CheckLunsReady will see LUN 1 is not ready, drain the queued
  // transaction for LUN 1 (completing it with ZX_ERR_IO_NOT_PRESENT), and tear down LUN 1.
  allow_tur_resume.store(true);

  check_thread.join();
  client_thread.join();

  EXPECT_STATUS(txn_status.load(), ZX_ERR_IO_NOT_PRESENT);
  EXPECT_EQ(driver_->block_devs()[1], nullptr);
}

// Verifies that when a LUN becomes unready and begins teardown in CheckLunsReady,
// any new transactions arriving after initial draining are immediately failed
// and not queued, preventing deadlocks during ShutdownAsync.
TEST_F(UmsTest, NewRequestsFailDuringLunShutdown) {
  StartDriver();

  std::vector<std::string> dev_names = GetDeviceNames();
  ASSERT_GE(dev_names.size(), 2u);

  auto client1_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(dev_names[1].c_str());
  ASSERT_OK(client1_end);
  auto client1 = block_client::RemoteBlockDevice::Create(std::move(client1_end.value())).value();

  // Prevent WorkerLoop from spinning and periodically invoking CheckLunsReady.
  driver_test().RunInDriverContext([&](TestUsbMassStorageDevice& driver) {
    driver.timer().set_timeout_handler([](sync_completion_t* completion, zx_duration_t duration) {
      return sync_completion_wait(completion, ZX_TIME_INFINITE);
    });
  });

  libsync::Completion pre_shutdown_started;
  std::atomic<bool> allow_shutdown_resume = false;

  driver_test().RunInEnvironmentTypeContext([&](Environment& env) {
    env.context().on_cbw = [&](const ums_cbw_t& cbw) {
      if (cbw.bCBWLUN == 1 &&
          static_cast<scsi::Opcode>(cbw.CBWCB[0]) == scsi::Opcode::TEST_UNIT_READY) {
        env.context().tur_csw_status = CSW_FAILED;
      }
    };
  });

  driver_test().RunInDriverContext([&](TestUsbMassStorageDevice& driver) {
    driver.set_on_pre_shutdown([&](uint8_t lun) {
      if (lun == 1) {
        pre_shutdown_started.Signal();
        while (!allow_shutdown_resume.load()) {
          zx::nanosleep(zx::deadline_after(zx::msec(1)));
        }
      }
    });
  });

  // Trigger CheckLunsReady on a background thread. It will see LUN 1 is not ready,
  // mark LUN 1 to fail new requests, drain transactions, and pause at on_pre_shutdown.
  std::thread check_thread([&] {
    driver_test().RunInDriverContext(
        [&](TestUsbMassStorageDevice& driver) { EXPECT_OK(driver.CheckLunsReady()); });
  });

  pre_shutdown_started.Wait();

  // Issue a transaction on LUN 1 while it is in the middle of teardown (post-drain,
  // pre-ShutdownAsync).
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_FLUSH},
      .vmoid = BLOCK_VMOID_INVALID,
      .length = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  std::atomic<zx_status_t> txn_status = ZX_OK;
  std::thread client_thread([&] { txn_status = client1->FifoTransaction(&request, 1); });

  // The request should complete immediately with ZX_ERR_IO_NOT_PRESENT without being queued.
  client_thread.join();
  EXPECT_STATUS(txn_status.load(), ZX_ERR_IO_NOT_PRESENT);
  EXPECT_EQ(driver_->queued_txns_count(), 0u);

  // Resume CheckLunsReady to complete ShutdownAsync and device destruction.
  allow_shutdown_resume.store(true);

  check_thread.join();

  EXPECT_EQ(driver_->block_devs()[1], nullptr);
}

FUCHSIA_DRIVER_EXPORT2(TestUsbMassStorageDevice);

}  // namespace
