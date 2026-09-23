// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/driver/component/cpp/node_offers.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/scsi/block-device.h>
#include <netinet/in.h>
#include <zircon/process.h>

#include <fbl/alloc_checker.h>

#include "src/devices/block/lib/common/include/common.h"

namespace scsi {

ScsiRequest::ScsiRequest(ScsiRequest&& other) {
  request_id_ = other.request_id_;
  data_vmo_ = std::move(other.data_vmo_);
  vmo_offset_ = other.vmo_offset_;
  device_offset_ = other.device_offset_;
  transfer_length_ = other.transfer_length_;
  block_size_ = other.block_size_;
  cdb_ = other.cdb_;
  cdb_length_ = other.cdb_length_;
  immediate_data_ = other.immediate_data_;
  immediate_data_length_ = other.immediate_data_length_;
  is_write_ = other.is_write_;
  completed_ = other.completed_;
  parent_ = other.parent_;

  other.parent_ = nullptr;
}

ScsiRequest& ScsiRequest::operator=(ScsiRequest&& other) {
  if (this != &other) {
    if (parent_) {
      ZX_ASSERT_MSG(completed_, "Active ScsiRequest overwritten before completion");
    }
    request_id_ = other.request_id_;
    data_vmo_ = std::move(other.data_vmo_);
    vmo_offset_ = other.vmo_offset_;
    device_offset_ = other.device_offset_;
    transfer_length_ = other.transfer_length_;
    block_size_ = other.block_size_;
    cdb_ = other.cdb_;
    cdb_length_ = other.cdb_length_;
    immediate_data_ = other.immediate_data_;
    immediate_data_length_ = other.immediate_data_length_;
    is_write_ = other.is_write_;
    completed_ = other.completed_;
    parent_ = other.parent_;

    other.parent_ = nullptr;
  }
  return *this;
}

// `Complete` can be called from any thread (such as an interrupt handler, a controller driver
// worker/dispatcher thread, or synchronously within `ExecuteCommandsAsync`).
// `BlockServer::SendReply` is internally synchronized and thread-safe. During device shutdown,
// `ShutdownAsync` invokes `BlockServer::DestroyAsync`, which waits for all sessions to terminate
// before executing its completion callback where `block_server_.reset()` is called.
void ScsiRequest::Complete(zx_status_t status) {
  ZX_ASSERT(parent_);
  ZX_ASSERT(!completed_);
  completed_ = true;
  if (parent_->block_server().has_value()) {
    parent_->block_server()->SendReply(request_id_, zx::make_result(status));
  }
}

BlockDevice::~BlockDevice() {
  ZX_ASSERT(!block_server_);
  RemoveDevice();
}

void BlockDevice::ShutdownAsync(fit::callback<void()> callback) {
  if (block_server_) {
    block_server_->DestroyAsync([this, callback = std::move(callback)]() mutable {
      block_server_.reset();
      if (callback) {
        callback();
      }
    });
  } else if (callback) {
    callback();
  }
}

zx::result<std::unique_ptr<BlockDevice>> BlockDevice::Bind(Controller* controller, uint8_t target,
                                                           uint16_t lun,
                                                           uint32_t max_transfer_bytes,
                                                           DeviceOptions device_options) {
  fbl::AllocChecker ac;
  auto block_device =
      fbl::make_unique_checked<BlockDevice>(&ac, controller, target, lun, device_options);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  auto status = block_device->AddDevice(max_transfer_bytes);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(block_device));
}

zx_status_t BlockDevice::AddDevice(uint32_t max_transfer_bytes) {
  zx::result inquiry_data = controller_->Inquiry(target_, lun_);
  if (inquiry_data.is_error()) {
    return inquiry_data.status_value();
  }
  // Check that its a direct access block device first.
  if (inquiry_data.value().peripheral_device_type != 0) {
    logger().log(fdf::ERROR, "Device is not direct access block device!, device type: 0x{:x}",
                 inquiry_data.value().peripheral_device_type);
    return ZX_ERR_IO;
  }

  // Print T10 Vendor ID/Product ID
  auto vendor_id =
      std::string(inquiry_data.value().t10_vendor_id, sizeof(inquiry_data.value().t10_vendor_id));
  auto product_id =
      std::string(inquiry_data.value().product_id, sizeof(inquiry_data.value().product_id));
  // Some vendors don't pad the strings with spaces (0x20). Null-terminate strings to avoid printing
  // illegal characters.
  vendor_id = std::string(vendor_id.c_str());
  product_id = std::string(product_id.c_str());
  logger().log(fdf::INFO, "Target {} LUN {}: Vendor ID = {}, Product ID = {}", target_, lun_,
               vendor_id, product_id);

  removable_ = inquiry_data.value().removable_media();

  // Verify that the Lun is ready. This command expects a unit attention error.
  if (zx_status_t status = controller_->TestUnitReady(target_, lun_); status != ZX_OK) {
    // TestUnitReady returns ZX_ERR_UNAVAILABLE status if a unit attention error occurred.
    if (status != ZX_ERR_UNAVAILABLE) {
      logger().log(fdf::ERROR, "Failed SCSI TEST UNIT READY command: {}",
                   zx_status_get_string(status));
      return status;
    }
    logger().log(fdf::DEBUG, "Expected Unit Attention error: {}", zx_status_get_string(status));

    // Send request sense commands to clear the Unit Attention Condition(UAC) of LUs. UAC is a
    // condition which needs to be serviced before the logical unit can process commands.
    // This command will get sense data, but ignore it for now because our goal is to clear the
    // UAC.
    // The UFS specification requires data transfer sizes to be a multiple of 4 bytes.
    // Although FixedFormatSenseDataHeader is 18 bytes, we use a 20-byte buffer.
    uint8_t request_sense_buffer[sizeof(scsi::FixedFormatSenseDataHeader) + 2];
    static_assert(sizeof(request_sense_buffer) % 4 == 0);
    zx_status_t clear_uac_status = controller_->RequestSense(
        target_, lun_, {request_sense_buffer, sizeof(request_sense_buffer)});
    if (clear_uac_status != ZX_OK) {
      logger().log(fdf::ERROR, "Failed SCSI REQUEST SENSE command: {}",
                   zx_status_get_string(clear_uac_status));
      return clear_uac_status;
    }

    // Verify that the Lun is ready. This command expects a success.
    clear_uac_status = controller_->TestUnitReady(target_, lun_);
    if (clear_uac_status != ZX_OK) {
      logger().log(fdf::ERROR, "Failed SCSI TEST UNIT READY command: {}",
                   zx_status_get_string(clear_uac_status));
      return clear_uac_status;
    }
  }

  zx::result<std::tuple<bool, bool>> parameter =
      controller_->ModeSenseDpoFuaAndWriteProtectedEnabled(target_, lun_,
                                                           device_options_.use_mode_sense_6);
  if (parameter.is_error()) {
    logger().log(fdf::WARN,
                 "Failed to get DPO FUA and write protected parameter for target {}, lun {}: {}.",
                 target_, lun_, zx_status_get_string(parameter.status_value()));
    return parameter.error_value();
  }
  std::tie(dpo_fua_available_, write_protected_) = parameter.value();

  zx::result write_cache_enabled =
      controller_->ModeSenseWriteCacheEnabled(target_, lun_, device_options_.use_mode_sense_6);
  if (write_cache_enabled.is_error()) {
    logger().log(fdf::WARN, "Failed to get write cache status for target {}, lun {}: {}.", target_,
                 lun_, zx_status_get_string(write_cache_enabled.status_value()));
    // Assume write cache is enabled so that flush operations are not ignored.
    write_cache_enabled_ = true;
  } else {
    write_cache_enabled_ = write_cache_enabled.value();
  }

  zx_status_t status = controller_->ReadCapacity(target_, lun_, &block_count_, &block_size_bytes_);
  if (status != ZX_OK) {
    return status;
  }
  logger().log(fdf::INFO, "{} blocks of {} bytes", block_count_, block_size_bytes_);

  zx::result<VPDBlockLimits> block_limits = controller_->InquiryBlockLimits(target_, lun_);
  if (block_limits.is_ok()) {
    // A maximum_transfer_length field set to 0 indicates that the device server does not
    // report a limit on the transfer length.
    if (betoh32(block_limits->maximum_transfer_blocks) == 0) {
      max_transfer_bytes_ = max_transfer_bytes;
    } else {
      max_transfer_bytes_ = std::min(
          max_transfer_bytes, betoh32(block_limits->maximum_transfer_blocks) * block_size_bytes_);
    }
  } else {
    max_transfer_bytes_ = max_transfer_bytes;
  }

  if (max_transfer_bytes_ == fuchsia_storage_block::wire::kMaxTransferUnbounded) {
    max_transfer_blocks_ = UINT32_MAX;
  } else {
    if (max_transfer_bytes_ % block_size_bytes_ != 0) {
      logger().log(fdf::ERROR,
                   "Max transfer size ({} bytes) is not a multiple of the block size ({} bytes).",
                   max_transfer_bytes_, block_size_bytes_);
      return ZX_ERR_BAD_STATE;
    }
    max_transfer_blocks_ = max_transfer_bytes_ / block_size_bytes_;
  }

  // If we only need to use the read(10)/write(10) commands, then limit max_transfer_blocks_ and
  // max_transfer_bytes_.
  if (block_count_ <= UINT32_MAX && !device_options_.use_read_write_12 &&
      max_transfer_blocks_ > UINT16_MAX) {
    max_transfer_blocks_ = UINT16_MAX;
    max_transfer_bytes_ = max_transfer_blocks_ * block_size_bytes_;
  }

  if (device_options_.check_unmap_support && block_limits.is_ok()) {
    zx::result unmap_command_supported = controller_->InquirySupportUnmapCommand(target_, lun_);
    if (unmap_command_supported.is_ok()) {
      unmap_command_supported_ =
          unmap_command_supported.value() && betoh32(block_limits->maximum_unmap_lba_count);
    }
  }

  {
    const std::string path_from_parent = std::string(controller_->driver_name()) + "/";
    compat::DeviceServer::BanjoConfig banjo_config;

    zx::result<> result = compat_server_.Initialize(
        controller_->driver_incoming(), controller_->driver_outgoing(),
        controller_->driver_node_name(), DeviceName(), compat::ForwardMetadata::None(),
        std::move(banjo_config), path_from_parent);
    if (result.is_error()) {
      return result.status_value();
    }
  }

  fbl::String partition_name = LunName();
  const block_server::PartitionInfo info = {
      .device_flags =
          (write_protected_
               ? static_cast<uint32_t>(fuchsia_storage_block::wire::DeviceFlag::kReadonly)
               : 0u) |
          (removable_ ? static_cast<uint32_t>(fuchsia_storage_block::wire::DeviceFlag::kRemovable)
                      : 0u) |
          (dpo_fua_available_
               ? static_cast<uint32_t>(fuchsia_storage_block::wire::DeviceFlag::kFuaSupport)
               : 0u),
      .start_block = 0,
      .block_count = block_count_,
      .block_size = block_size_bytes_,
      .type_guid = {},
      .instance_guid = {},
      .name = partition_name.c_str(),
      .flags = 0,
      .max_transfer_size = max_transfer_bytes_,
  };
  block_server_.emplace(info, this);

  auto handlers = fuchsia_hardware_block_volume::Service::InstanceHandler({
      .volume =
          [this](fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
            if (block_server_) {
              block_server_->Serve(std::move(server_end));
            }
          },
      .token =
          [this](fidl::ServerEnd<fuchsia_driver_token::NodeToken> server_end) {
            fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                             std::move(server_end), this);
          },
  });

  auto add_svc_result =
      controller_->driver_outgoing()->AddService<fuchsia_hardware_block_volume::Service>(
          std::move(handlers), DeviceName().c_str());
  if (add_svc_result.is_error()) {
    logger().log(fdf::ERROR, "Failed to add volume service: {}", add_svc_result.status_string());
    return add_svc_result.status_value();
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  node_controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;
  std::vector<fuchsia_driver_framework::wire::Offer> offers = compat_server_.CreateOffers2(arena);

  fidl::VectorView<fuchsia_driver_framework::wire::NodeProperty2> properties;
  offers.push_back(
      fdf::MakeOffer2<fuchsia_hardware_block_volume::Service>(arena, DeviceName().c_str()));

  const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                        .name(arena, DeviceName())
                        .offers2(arena, std::move(offers))
                        .properties2(properties)
                        .Build();

  fidl::WireResult<fuchsia_driver_framework::Node::AddChild> result =
      controller_->root_node()->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    logger().log(fdf::ERROR, "Failed to call AddChild: {}", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    fuchsia_driver_framework::NodeError node_error = result->error_value();
    logger().log(fdf::ERROR, "AddChild returned failure: {}", static_cast<uint32_t>(node_error));
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

void BlockDevice::OnRequests(std::span<block_server::Request> requests) {
  scsi::ScsiRequest translated[64];
  size_t count = 0;

  for (const auto& req : requests) {
    if (count >= std::size(translated)) {
      controller_->ExecuteCommandsAsync(target_, lun_, std::span{translated, count});
      count = 0;
    }

    scsi::ScsiRequest& scsi_req = translated[count++];
    scsi_req = {};
    scsi_req.parent_ = this;
    scsi_req.request_id_ = req.request_id;
    scsi_req.block_size_ = block_size_bytes_;

    switch (req.operation.tag) {
      case block_server::Operation::Tag::Read:
      case block_server::Operation::Tag::Write: {
        if (zx_status_t status = block_server::CheckIoRange(req, block_count_); status != ZX_OK) {
          scsi_req.Complete(status);
          count--;
          continue;
        }
        const bool is_write = req.operation.tag == block_server::Operation::Tag::Write;
        const bool is_fua = is_write && req.operation.write.options.flags.is_force_access();
        if (!dpo_fua_available_ && is_fua) {
          scsi_req.Complete(ZX_ERR_NOT_SUPPORTED);
          count--;
          continue;
        }

        scsi_req.data_vmo_ = req.vmo->borrow();
        scsi_req.vmo_offset_ =
            is_write ? req.operation.write.vmo_offset : req.operation.read.vmo_offset;
        scsi_req.is_write_ = is_write;

        uint64_t dev_off = is_write ? req.operation.write.device_block_offset
                                    : req.operation.read.device_block_offset;
        uint32_t len = is_write ? req.operation.write.block_count : req.operation.read.block_count;
        scsi_req.transfer_length_ = len;
        scsi_req.device_offset_ = dev_off;

        if (block_count_ > UINT32_MAX) {
          auto cdb = reinterpret_cast<Read16CDB*>(scsi_req.cdb_.data());
          scsi_req.cdb_length_ = 16;
          cdb->opcode = is_write ? Opcode::WRITE_16 : Opcode::READ_16;
          cdb->logical_block_address = htobe64(dev_off);
          cdb->transfer_length = htobe32(len);
          cdb->set_force_unit_access(is_fua);
        } else if (device_options_.use_read_write_12) {
          auto cdb = reinterpret_cast<Read12CDB*>(scsi_req.cdb_.data());
          scsi_req.cdb_length_ = 12;
          cdb->opcode = is_write ? Opcode::WRITE_12 : Opcode::READ_12;
          cdb->logical_block_address = htobe32(static_cast<uint32_t>(dev_off));
          cdb->transfer_length = htobe32(len);
          cdb->set_force_unit_access(is_fua);
        } else {
          auto cdb = reinterpret_cast<Read10CDB*>(scsi_req.cdb_.data());
          scsi_req.cdb_length_ = 10;
          cdb->opcode = is_write ? Opcode::WRITE_10 : Opcode::READ_10;
          cdb->logical_block_address = htobe32(static_cast<uint32_t>(dev_off));
          cdb->transfer_length = htobe16(static_cast<uint16_t>(len));
          cdb->set_force_unit_access(is_fua);
        }
        break;
      }
      case block_server::Operation::Tag::Flush: {
        if (!write_cache_enabled_) {
          scsi_req.Complete(ZX_OK);
          count--;
          continue;
        }
        scsi_req.is_write_ = false;
        scsi_req.data_vmo_ = {};
        scsi_req.transfer_length_ = 0;
        scsi_req.device_offset_ = 0;
        scsi_req.vmo_offset_ = 0;
        auto cdb = reinterpret_cast<SynchronizeCache10CDB*>(scsi_req.cdb_.data());
        scsi_req.cdb_length_ = sizeof(SynchronizeCache10CDB);
        cdb->opcode = Opcode::SYNCHRONIZE_CACHE_10;
        cdb->logical_block_address = 0;
        cdb->number_of_logical_blocks = 0;
        break;
      }
      case block_server::Operation::Tag::Trim: {
        if (!unmap_command_supported_) {
          scsi_req.Complete(ZX_ERR_NOT_SUPPORTED);
          count--;
          continue;
        }
        if (zx_status_t status = block_server::CheckIoRange(req, block_count_); status != ZX_OK) {
          scsi_req.Complete(status);
          count--;
          continue;
        }
        scsi_req.is_write_ = true;
        scsi_req.data_vmo_ = {};
        scsi_req.transfer_length_ = 0;
        scsi_req.device_offset_ = 0;
        scsi_req.vmo_offset_ = 0;

        uint8_t* pdata = scsi_req.immediate_data_.data();
        UnmapParameterListHeader* hdr = reinterpret_cast<UnmapParameterListHeader*>(pdata);
        hdr->data_length = htobe16(sizeof(UnmapParameterListHeader) - sizeof(hdr->data_length) +
                                   sizeof(UnmapBlockDescriptor));
        hdr->block_descriptor_data_length = htobe16(sizeof(UnmapBlockDescriptor));

        UnmapBlockDescriptor* desc =
            reinterpret_cast<UnmapBlockDescriptor*>(pdata + sizeof(UnmapParameterListHeader));
        desc->logical_block_address = htobe64(req.operation.trim.device_block_offset);
        desc->blocks = htobe32(req.operation.trim.block_count);

        scsi_req.immediate_data_length_ =
            sizeof(UnmapParameterListHeader) + sizeof(UnmapBlockDescriptor);

        auto cdb = reinterpret_cast<UnmapCDB*>(scsi_req.cdb_.data());
        scsi_req.cdb_length_ = sizeof(UnmapCDB);
        cdb->opcode = Opcode::UNMAP;
        cdb->parameter_list_length = htobe16(scsi_req.immediate_data_length_);
        break;
      }
      default:
        scsi_req.Complete(ZX_ERR_NOT_SUPPORTED);
        count--;
        continue;
    }
  }

  if (count > 0) {
    controller_->ExecuteCommandsAsync(target_, lun_, std::span{translated, count});
  }
}

void BlockDevice::Get(GetCompleter::Sync& completer) {
  zx::event token = controller_->node_token();
  if (token.is_valid()) {
    completer.Reply(zx::ok(std::move(token)));
  } else {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
  }
}

fdf::Logger& BlockDevice::logger() const { return controller_->driver_logger(); }

}  // namespace scsi
