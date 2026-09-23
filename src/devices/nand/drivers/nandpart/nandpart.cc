// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/devices/nand/drivers/nandpart/nandpart.h"

#include <assert.h>
#include <fidl/fuchsia.boot.metadata/cpp/natural_types.h>
#include <fidl/fuchsia.driver.framework/cpp/markers.h>
#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.nand/cpp/natural_types.h>
#include <fuchsia/hardware/nand/c/banjo.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <fuchsia/hardware/nandinfo/c/banjo.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/driver/node/cpp/add_child.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/operation/nand.h>
#include <lib/stdcompat/span.h>
#include <lib/zircon-assert/zircon/assert.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <string.h>
#include <zircon/hw/gpt.h>
#include <zircon/status.h>
#include <zircon/system/public/zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/nand/cpp/bind.h>
#include <fbl/algorithm.h>

#include "src/devices/nand/drivers/nandpart/bad-block.h"
#include "src/devices/nand/drivers/nandpart/nandpart-utils.h"

namespace nand {
namespace {

constexpr uint8_t fvm_guid[] = GUID_FVM_VALUE;
constexpr uint8_t test_guid[] = GUID_TEST_VALUE;

struct PrivateStorage {
  uint32_t offset;
};

using NandPartOp = nand::BorrowedOperation<PrivateStorage>;

// Shim for calling sub-partition's callback.
void CompletionCallback(void* cookie, zx_status_t status, nand_operation_t* nand_op) {
  NandPartOp op(nand_op, *static_cast<size_t*>(cookie));
  // Re-translate the offsets.
  switch (op.operation()->command) {
    case NAND_OP_READ_BYTES:
    case NAND_OP_WRITE_BYTES:
      op.operation()->rw_bytes.offset_nand -= op.private_storage()->offset;
      break;
    case NAND_OP_READ:
    case NAND_OP_WRITE:
      op.operation()->rw.offset_nand -= op.private_storage()->offset;
      break;
    case NAND_OP_ERASE:
      op.operation()->erase.first_block -= op.private_storage()->offset;
      break;
    default:
      ZX_ASSERT(false);
  }
  op.Complete(status);
}

bool InBounds(uint64_t offset, uint64_t length, uint64_t limit) {
  return offset + length >= offset && offset + length <= limit;
}

}  // namespace

zx::result<> Driver::Start(fdf::DriverContext context) {
  std::shared_ptr<fdf::Namespace> incoming = context.take_incoming();
  zx::result nand_result = compat::ConnectBanjo<ddk::NandProtocolClient>(incoming);
  if (nand_result.is_error()) {
    fdf::error("Failed to connect to nand banjo protocol: {}", nand_result);
    return nand_result.take_error();
  }
  ddk::NandProtocolClient nand = nand_result.value();

  // Query parent to get its nand_info_t and size for nand_operation_t.
  nand_info_t nand_info;
  size_t parent_op_size;
  nand.Query(&nand_info, &parent_op_size);
  // Make sure parent_op_size is aligned, so we can safely add our data at the end.
  parent_op_size = fbl::round_up(parent_op_size, 8u);

  // Query parent for nand configuration info.
  zx::result nand_config =
      compat::GetMetadata<fuchsia_hardware_nand::Config>(incoming, DEVICE_METADATA_PRIVATE);
  if (!nand_config.is_ok()) {
    fdf::error("Failed to get metadata: {}", nand_config.status_string());
    return nand_config.take_error();
  }
  // Create a bad block instance.
  BadBlock::Config config{
      .bad_block_config = nand_config->bad_block_config(),
  };
  nand.GetProto(&config.nand_proto);
  zx::result bad_block = BadBlock::Create(config);
  if (bad_block.is_error()) {
    fdf::error("Failed to create BadBlock object: {}", bad_block);
    return bad_block.take_error();
  }

  // Query parent for partition map.
  zx::result metadata = fdf_metadata::GetMetadata<fuchsia_boot_metadata::PartitionMap>(incoming);
  if (metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata.status_string());
    return metadata.take_error();
  }
  fuchsia_boot_metadata::PartitionMap& pmap = metadata.value();

  // Sanity check partition map and transform into expected form.
  zx_status_t status = SanitizePartitionMap(pmap, nand_info);
  if (status != ZX_OK) {
    fdf::error("Failed to sanitize partition map: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  // Create a device for each partition.
  for (const fuchsia_boot_metadata::Partition& part : pmap.partitions().value()) {
    nand_info.num_blocks = static_cast<uint32_t>(part.last_block() - part.first_block() + 1);
    memcpy(&nand_info.partition_guid, part.type_guid().data(), sizeof(nand_info.partition_guid));
    // We only use FTL for the FVM partition.
    if (memcmp(part.type_guid().data(), fvm_guid, sizeof(fvm_guid)) == 0) {
      nand_info.nand_class = NAND_CLASS_FTL;
    } else if (memcmp(part.type_guid().data(), test_guid, sizeof(test_guid)) == 0) {
      nand_info.nand_class = NAND_CLASS_TEST;
    } else {
      nand_info.nand_class = NAND_CLASS_BBS;
    }

    nand_protocol_t nand_proto;
    nand.GetProto(&nand_proto);
    auto& device = devices_.emplace_back(
        std::make_unique<NandPartDevice>(nand_proto, bad_block.value(), parent_op_size, nand_info,
                                         static_cast<uint32_t>(part.first_block()), part.name()));

    // Find optional partition_config information.
    uint32_t copy_count = 1;
    for (const fuchsia_hardware_nand::PartitionConfig& extra :
         nand_config.value().extra_partition_configs()) {
      if (memcmp(extra.type_guid().data(), part.type_guid().data(), part.type_guid().size()) == 0 &&
          extra.copy_count() > 0) {
        copy_count = extra.copy_count();
        break;
      }
    }
    zx::result result =
        device->Init(copy_count, node().borrow(), context.node_name(), incoming, outgoing());
    if (result.is_error()) {
      fdf::error("Failed to initialize nand-part device \"{}\": {}", part.name(), result);

      continue;
    }
  }

  return zx::ok();
}

zx::result<> NandPartDevice::Init(uint32_t copy_count,
                                  fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
                                  const std::optional<std::string>& node_name,
                                  const std::shared_ptr<fdf::Namespace>& incoming,
                                  const std::shared_ptr<fdf::OutgoingDirectory>& outgoing) {
  if (bad_block_ == nullptr) {
    fdf::error("Bad block not initialized");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  compat::DeviceServer::BanjoConfig banjo_config{.default_proto_id = ZX_PROTOCOL_NAND};
  banjo_config.callbacks[ZX_PROTOCOL_NAND] = nand_server_.callback();
  banjo_config.callbacks[ZX_PROTOCOL_BAD_BLOCK] = bad_block_server_.callback();

  zx::result result =
      compat_server_.Initialize(incoming, outgoing, node_name, name_,
                                compat::ForwardMetadata::None(), std::move(banjo_config));
  if (result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result);
    return result.take_error();
  }

  extra_partition_copy_count_ = copy_count;
  const std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_nand::BIND_PROTOCOL_DEVICE),
      fdf::MakeProperty2(bind_fuchsia::NAND_CLASS, nand_info_.nand_class),
  };
  std::vector<uint8_t> metadata(sizeof(extra_partition_copy_count_));
  memcpy(metadata.data(), &extra_partition_copy_count_, sizeof(extra_partition_copy_count_));

  zx_status_t status =
      compat_server_.inner().AddMetadata(DEVICE_METADATA_PRIVATE, metadata.data(), metadata.size());
  if (status != ZX_OK) {
    fdf::error("Failed to add metadata: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  const std::vector<fuchsia_driver_framework::Offer> offers = compat_server_.CreateOffers2();

  zx::result child =
      fdf::AddChild(parent, *fdf::Logger::GlobalInstance(), name_, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void NandPartDevice::NandQuery(nand_info_t* info_out, size_t* nand_op_size_out) {
  memcpy(info_out, &nand_info_, sizeof(*info_out));
  // Add size of extra context.
  *nand_op_size_out = NandPartOp::OperationSize(parent_op_size_);
}

void NandPartDevice::NandQueue(nand_operation_t* nand_op, nand_queue_callback completion_cb,
                               void* cookie) {
  NandPartOp op(nand_op, completion_cb, cookie, parent_op_size_);
  uint32_t command = op.operation()->command;

  // Make offset relative to full underlying device
  switch (command) {
    case NAND_OP_READ_BYTES:
    case NAND_OP_WRITE_BYTES: {
      if (!InBounds(op.operation()->rw_bytes.offset_nand, op.operation()->rw_bytes.length,
                    static_cast<uint64_t>(nand_info_.num_blocks) * nand_info_.pages_per_block *
                        nand_info_.page_size)) {
        op.Complete(ZX_ERR_OUT_OF_RANGE);
        return;
      }
      op.private_storage()->offset =
          erase_block_start_ * nand_info_.pages_per_block * nand_info_.page_size;
      op.operation()->rw_bytes.offset_nand += op.private_storage()->offset;
      break;
    }
    case NAND_OP_READ:
    case NAND_OP_WRITE: {
      if (!InBounds(op.operation()->rw.offset_nand, op.operation()->rw.length,
                    static_cast<uint64_t>(nand_info_.num_blocks) * nand_info_.pages_per_block)) {
        op.Complete(ZX_ERR_OUT_OF_RANGE);
        return;
      }
      op.private_storage()->offset = erase_block_start_ * nand_info_.pages_per_block;
      op.operation()->rw.offset_nand += op.private_storage()->offset;
      break;
    }
    case NAND_OP_ERASE: {
      if (!InBounds(op.operation()->erase.first_block, op.operation()->erase.num_blocks,
                    nand_info_.num_blocks)) {
        op.Complete(ZX_ERR_OUT_OF_RANGE);
        return;
      }
      op.private_storage()->offset = erase_block_start_;
      op.operation()->erase.first_block += erase_block_start_;
      break;
    }
    default:
      op.Complete(ZX_ERR_NOT_SUPPORTED);
      return;
  }

  // Call parent's queue
  nand_.Queue(op.take(), CompletionCallback, &parent_op_size_);
}

zx_status_t NandPartDevice::NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                                       size_t* num_bad_blocks) {
  // TODO implement this.
  *num_bad_blocks = 0;
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t NandPartDevice::BadBlockGetBadBlockList(uint32_t* bad_block_list,
                                                    size_t bad_block_list_len,
                                                    size_t* bad_block_count) {
  if (bad_blocks_.empty()) {
    zx::result bad_blocks = bad_block_->GetBadBlockList(
        erase_block_start_, erase_block_start_ + nand_info_.num_blocks - 1);
    if (bad_blocks.is_error()) {
      return bad_blocks.status_value();
    }
    bad_blocks_ = std::move(bad_blocks.value());
    for (uint32_t& bad_block : bad_blocks_) {
      bad_block -= erase_block_start_;
    }
  }

  *bad_block_count = bad_blocks_.size();
  fdf::debug("Nandpart \"{}\": Bad block count: {}", name_, *bad_block_count);

  if (bad_block_list_len == 0 || bad_blocks_.size() == 0) {
    return ZX_OK;
  }
  if (bad_block_list == NULL) {
    return ZX_ERR_INVALID_ARGS;
  }

  const size_t size = sizeof(uint32_t) * std::min(*bad_block_count, bad_block_list_len);
  memcpy(bad_block_list, bad_blocks_.data(), size);
  return ZX_OK;
}

zx_status_t NandPartDevice::BadBlockMarkBlockBad(uint32_t block) {
  if (block >= nand_info_.num_blocks) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // First, invalidate our cached copy.
  bad_blocks_.clear();

  // Second, "write-through" to actually persist.
  block += erase_block_start_;
  return bad_block_->MarkBlockBad(block);
}

}  // namespace nand

FUCHSIA_DRIVER_EXPORT2(nand::Driver);
