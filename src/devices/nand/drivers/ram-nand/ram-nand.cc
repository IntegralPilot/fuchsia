// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ram-nand.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/node/cpp/add_child.h>
#include <lib/zbi-format/partition.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <atomic>
#include <utility>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/nand/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <fbl/auto_lock.h>
#include <safemath/safe_math.h>

namespace {

static_assert(ZBI_PARTITION_NAME_LEN == fuchsia_hardware_nand::wire::kNameLen, "bad fidl name");
static_assert(ZBI_PARTITION_GUID_LEN == fuchsia_hardware_nand::wire::kGuidLen, "bad fidl guid");

uint32_t GetPartitionCount(const fuchsia_hardware_nand::wire::RamNandInfo& info) {
  return std::min(info.partition_map.partition_count, fuchsia_hardware_nand::wire::kMaxPartitions);
}

fuchsia_hardware_nand::Config ExtractNandConfig(
    const fuchsia_hardware_nand::wire::RamNandInfo& info) {
  fuchsia_hardware_nand::BadBlockConfig bad_block_config({
      .type = fuchsia_hardware_nand::BadBlockConfigType::kAmlogicUboot,
  });
  std::vector<fuchsia_hardware_nand::PartitionConfig> extra_partition_configs;

  for (size_t i = 0; i < GetPartitionCount(info); i++) {
    const auto& partition = info.partition_map.partitions[i];
    if (partition.hidden && partition.bbt) {
      bad_block_config.table_start_block() = partition.first_block;
      bad_block_config.table_end_block() = partition.last_block;
    } else if (!partition.hidden && partition.copy_count > 1) {
      auto& extra = extra_partition_configs.emplace_back(fuchsia_hardware_nand::PartitionConfig{{
          .copy_count = partition.copy_count,
          .copy_byte_offset = partition.copy_byte_offset,
      }});
      std::ranges::copy(partition.unique_guid, extra.type_guid().begin());
    }
  }

  return fuchsia_hardware_nand::Config(
      {.bad_block_config = bad_block_config,
       .extra_partition_configs = std::move(extra_partition_configs)});
}

fuchsia_boot_metadata::PartitionMap ExtractPartitionMap(
    const fuchsia_hardware_nand::wire::RamNandInfo& info) {
  fuchsia_boot_metadata::PartitionMap map(
      {.block_count = info.nand_info.num_blocks,
       .block_size = info.nand_info.page_size * info.nand_info.pages_per_block,
       .guid{{0}}});
  std::ranges::copy(info.partition_map.device_guid, map.guid().value().begin());

  const std::span src_partitions(info.partition_map.partitions.begin(), GetPartitionCount(info));
  auto partitions =
      src_partitions |
      std::views::filter([](const fuchsia_hardware_nand::wire::Partition& partition) {
        return !partition.hidden;
      }) |
      std::views::transform([](const fuchsia_hardware_nand::wire::Partition& src) {
        const auto name_end = std::ranges::find(src.name, '\0');
        fuchsia_boot_metadata::Partition dst({.type_guid{{0}},
                                              .unique_guid{{0}},
                                              .first_block = src.first_block,
                                              .last_block = src.last_block,
                                              .name = std::string(src.name.begin(), name_end)});
        std::ranges::copy(src.type_guid, dst.type_guid().begin());
        std::ranges::copy(src.unique_guid, dst.unique_guid().begin());
        return dst;
      });
  map.partitions().emplace(partitions.begin(), partitions.end());
  return map;
}

}  // namespace

namespace ram_nand {

NandDevice::~NandDevice() {
  if (operation_performer_.has_value()) {
    {
      fbl::AutoLock lock(&lock_);
      dead_ = true;
    }
    sync_completion_signal(&operations_pending_);
    std::thread operation_performer = std::move(operation_performer_).value();
    operation_performer.join();
  }
  ZX_ASSERT(pending_operations_.empty());
  if (mapped_addr_) {
    zx_vmar_unmap(zx_vmar_root_self(), mapped_addr_, params_.GetSize());
  }
}

zx::result<std::string> NandDevice::Init(
    fuchsia_hardware_nand::wire::RamNandInfo& info,
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
    const std::shared_ptr<fdf::Namespace>& incoming,
    const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
    const std::optional<std::string>& node_name) {
  ZX_DEBUG_ASSERT(!operation_performer_.has_value());

  // Validate dimensions are non-zero to prevent division-by-zero or empty allocations.
  if (params_.page_size == 0 || params_.pages_per_block == 0 || params_.num_blocks == 0) {
    fdf::error("Invalid NAND parameters: page_size={}, pages_per_block={}, num_blocks={}",
               params_.page_size, params_.pages_per_block, params_.num_blocks);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Ensure total page count does not overflow 32-bit integer limits.
  uint32_t max_pages;
  if (!safemath::CheckMul(params_.pages_per_block, params_.num_blocks).AssignIfValid(&max_pages)) {
    fdf::error("NAND pages_per_block * num_blocks overflows uint32_t");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Ensure sum of page size and OOB size does not overflow 32-bit integer limits.
  uint32_t page_and_oob;
  if (!safemath::CheckAdd(params_.page_size, params_.oob_size).AssignIfValid(&page_and_oob)) {
    fdf::error("NAND page_size + oob_size overflows uint32_t");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Ensure total RAM-backed device size does not overflow 64-bit integer limits.
  uint64_t total_size;
  if (!safemath::CheckMul(page_and_oob, max_pages).AssignIfValid(&total_size)) {
    fdf::error("NAND total size overflows uint64_t");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx_status_t status;
  const bool use_vmo = info.vmo.is_valid();
  if (use_vmo) {
    vmo_ = std::move(info.vmo);

    uint64_t size;
    status = vmo_.get_size(&size);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    if (size < params_.GetSize()) {
      fdf::error("VMO size too small: Expected at least {} bytes but actual is {} bytes",
                 params_.GetSize(), size);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  } else {
    status = zx::vmo::create(params_.GetSize(), 0, &vmo_);
    if (status != ZX_OK) {
      fdf::error("Failed to create vmo: {}", zx_status_get_string(status));
      return zx::error(status);
    }
  }

  status = zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo_.get(), 0,
                       params_.GetSize(), &mapped_addr_);
  if (status != ZX_OK) {
    fdf::error("Failed to map vmar: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  if (!use_vmo) {
    memset(reinterpret_cast<char*>(mapped_addr_), 0xff, params_.GetSize());
  }

  operation_performer_.emplace(fit::bind_member<&NandDevice::PerformOperations>(this));

  if (info.wear_vmo.is_valid()) {
    if (zx_status_t status =
            wear_info_.Map(info.wear_vmo, 0, info.nand_info.num_blocks * sizeof(uint32_t));
        status != ZX_OK) {
      fdf::error("Failed to map wear info: {}", zx_status_get_string(status));
      return zx::error(status);
    }
  }

  fail_after_ = static_cast<uint64_t>(info.fail_after) * params_.page_size;
  if (fail_after_ > 0) {
    fdf::info("fail-after: {}", fail_after_);
  }

  compat::DeviceServer::BanjoConfig banjo_config{.default_proto_id = ZX_PROTOCOL_NAND};
  banjo_config.callbacks[ZX_PROTOCOL_NAND] = banjo_server_.callback();
  zx::result init_result =
      compat_server_.Initialize(incoming, outgoing, node_name, device_name_,
                                compat::ForwardMetadata::None(), std::move(banjo_config));
  if (init_result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", init_result);
    return init_result.take_error();
  }
  if (info.export_partition_map) {
    const fuchsia_boot_metadata::PartitionMap partition_map = ExtractPartitionMap(info);
    zx::result result = partition_map_metadata_server_.Serve(*outgoing, dispatcher_, partition_map);
    if (result.is_error()) {
      fdf::error("Failed to serve partition map: {}", result);
      return result.take_error();
    }
  }
  if (info.export_nand_config) {
    const fuchsia_hardware_nand::Config nand_config = ExtractNandConfig(info);
    const fit::result persisted = fidl::Persist(nand_config);
    if (persisted.is_error()) {
      fdf::error("Failed to persist nand config: {}", persisted.error_value().FormatDescription());
      return zx::error(persisted.error_value().status());
    }
    compat_server_.inner().AddMetadata(DEVICE_METADATA_PRIVATE, persisted.value().data(),
                                       persisted.value().size());
  }

  zx::result connector = devfs_connector_.Bind(dispatcher_);
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs({
      .connector = std::move(connector.value()),
      .class_name = "nand",
      .connector_supports = fuchsia_device_fs::ConnectionType::kController,
  });

  std::vector<fuchsia_driver_framework::Offer> offers = compat_server_.CreateOffers2();
  std::optional partition_map_offer = partition_map_metadata_server_.CreateOffer();
  if (partition_map_offer.has_value()) {
    offers.emplace_back(std::move(partition_map_offer.value()));
  }

  const std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                         static_cast<uint32_t>(bind_fuchsia_nand::BIND_PROTOCOL_DEVICE)),
      fdf::MakeProperty2(bind_fuchsia::NAND_CLASS, params_.nand_class),
  };

  zx::result child = fdf::AddChild(parent, *fdf::Logger::GlobalInstance(), device_name_, devfs,
                                   properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to create child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  child_node_wait_.set_object(child_.channel().get());
  child_node_wait_.set_trigger(ZX_CHANNEL_PEER_CLOSED);
  if (zx_status_t status = child_node_wait_.Begin(dispatcher_); status != ZX_OK) {
    fdf::error("Failed to start wait on child node: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(device_name_);
}

void NandDevice::Unlink(UnlinkCompleter::Sync& completer) {
  unlink_completer_ = completer.ToAsync();
  if (child_) {
    auto result = fidl::WireCall(child_)->Remove();
    if (!result.ok()) {
      fdf::error("Failed to call Remove on child node: {}", result.status_string());
      unlink_completer_->Reply(ZX_OK);
      unlink_completer_.reset();
      if (on_unlink_) {
        std::move(on_unlink_)();
      }
    }
  } else {
    unlink_completer_->Reply(ZX_OK);
    unlink_completer_.reset();
    if (on_unlink_) {
      std::move(on_unlink_)();
    }
  }
}

void NandDevice::NandQuery(nand_info_t* info_out, size_t* nand_op_size_out) {
  *info_out = params_;
  *nand_op_size_out = sizeof(RamNandOp);
}

void NandDevice::NandQueue(nand_operation_t* operation, nand_queue_callback completion_cb,
                           void* cookie) {
  uint32_t max_pages = params_.NumPages();
  switch (operation->command) {
    case NAND_OP_READ_BYTES:
    case NAND_OP_WRITE_BYTES:
      if (operation->rw_bytes.offset_nand >= params_.GetSize() || !operation->rw_bytes.length ||
          (params_.GetSize() - operation->rw_bytes.offset_nand) < operation->rw_bytes.length) {
        completion_cb(cookie, ZX_ERR_OUT_OF_RANGE, operation);
        return;
      }
      if (operation->rw_bytes.data_vmo == ZX_HANDLE_INVALID) {
        completion_cb(cookie, ZX_ERR_BAD_HANDLE, operation);
        return;
      }
      break;
    case NAND_OP_READ:
    case NAND_OP_WRITE: {
      if (operation->rw.offset_nand >= max_pages || !operation->rw.length ||
          (max_pages - operation->rw.offset_nand) < operation->rw.length) {
        completion_cb(cookie, ZX_ERR_OUT_OF_RANGE, operation);
        return;
      }
      if (operation->rw.data_vmo == ZX_HANDLE_INVALID &&
          operation->rw.oob_vmo == ZX_HANDLE_INVALID) {
        completion_cb(cookie, ZX_ERR_BAD_HANDLE, operation);
        return;
      }
      break;
    }
    case NAND_OP_ERASE:
      if (!operation->erase.num_blocks || operation->erase.first_block >= params_.num_blocks ||
          params_.num_blocks - operation->erase.first_block < operation->erase.num_blocks) {
        completion_cb(cookie, ZX_ERR_OUT_OF_RANGE, operation);
        return;
      }
      break;

    default:
      completion_cb(cookie, ZX_ERR_NOT_SUPPORTED, operation);
      return;
  }

  if (zx::result result = AddPendingOperation(operation, completion_cb, cookie);
      result.is_error()) {
    fdf::error("Failed to queue operation: {}", result);
    completion_cb(cookie, result.status_value(), operation);
  }
}

zx_status_t NandDevice::NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                                   size_t* num_bad_blocks) {
  *num_bad_blocks = 0;
  return ZX_OK;
}

zx::result<> NandDevice::AddPendingOperation(nand_operation_t* operation,
                                             nand_queue_callback completion_cb, void* cookie) {
  fbl::AutoLock lock(&lock_);
  if (dead_) {
    fdf::error("Device is dead");
    return zx::error(ZX_ERR_BAD_STATE);
  }
  pending_operations_.emplace_back(RamNandOp{
      .op = operation,
      .completion_cb = completion_cb,
      .cookie = cookie,
  });
  sync_completion_signal(&operations_pending_);
  return zx::ok();
}

std::pair<bool, std::vector<NandDevice::RamNandOp>> NandDevice::TakePendingOperations() {
  sync_completion_wait(&operations_pending_, ZX_TIME_INFINITE);
  fbl::AutoLock lock(&lock_);
  std::vector pending_operations = std::move(pending_operations_);
  sync_completion_reset(&operations_pending_);
  return std::make_pair(dead_, std::move(pending_operations));
}

void NandDevice::PerformOperations() {
  while (true) {
    auto [dead, operations] = TakePendingOperations();
    if (dead) {
      for (const RamNandOp& operation : operations) {
        operation.completion_cb(operation.cookie, ZX_ERR_BAD_STATE, operation.op);
      }
      return;
    }

    for (RamNandOp& operation : operations) {
      PerformOperation(operation);
    }
  }
}

void NandDevice::PerformOperation(RamNandOp& operation) {
  zx_status_t status = ZX_OK;

  switch (operation.op->command) {
    case NAND_OP_WRITE_BYTES:
      if (fail_after_ > 0) {
        if (write_count_ >= fail_after_) {
          status = ZX_ERR_IO;
          break;
        }
        if (operation.op->rw_bytes.length > fail_after_ - write_count_) {
          const uint64_t old_length = operation.op->rw_bytes.length;
          operation.op->rw_bytes.length = fail_after_ - write_count_;
          status = ReadWriteData(operation.op, true);
          if (status == ZX_OK) {
            write_count_ = fail_after_;
            status = ZX_ERR_IO;
          }
          operation.op->rw_bytes.length = old_length;
          break;
        }
      }
      __FALLTHROUGH;
    case NAND_OP_READ_BYTES:
      status = ReadWriteData(operation.op, true);

      if (status == ZX_OK && operation.op->command == NAND_OP_WRITE_BYTES) {
        write_count_ += operation.op->rw_bytes.length;
      }
      break;
    case NAND_OP_WRITE:
      if (fail_after_ > 0) {
        if (write_count_ >= fail_after_) {
          status = ZX_ERR_IO;
          break;
        }
        uint64_t op_bytes = static_cast<uint64_t>(operation.op->rw.length) * params_.page_size;
        if (op_bytes > fail_after_ - write_count_) {
          const uint32_t old_length = operation.op->rw.length;
          operation.op->rw.length =
              static_cast<uint32_t>((fail_after_ - write_count_) / params_.page_size);
          status = ReadWriteData(operation.op, false);

          if (status == ZX_OK) {
            status = ReadWriteOob(operation.op);
          }
          if (status == ZX_OK) {
            write_count_ = fail_after_;
            status = ZX_ERR_IO;
          }
          operation.op->rw.length = old_length;
          break;
        }
      }
      __FALLTHROUGH;
    case NAND_OP_READ:
      status = ReadWriteData(operation.op, false);
      if (status == ZX_OK) {
        status = ReadWriteOob(operation.op);
      }
      if (status == ZX_OK && operation.op->command == NAND_OP_WRITE) {
        write_count_ += static_cast<uint64_t>(operation.op->rw.length) * params_.page_size;
      }
      break;

    case NAND_OP_ERASE: {
      status = Erase(operation.op);
      break;
    }
    default:
      ZX_DEBUG_ASSERT(false);  // Unexpected.
  }

  operation.completion_cb(operation.cookie, status, operation.op);
}

zx_status_t NandDevice::ReadWriteData(nand_operation_t* operation, bool bytes) {
  if (operation->rw.data_vmo == ZX_HANDLE_INVALID) {
    return ZX_OK;
  }

  uint64_t nand_addr;
  uint64_t vmo_addr;
  uint64_t length;
  if (bytes) {
    nand_addr = operation->rw_bytes.offset_nand;
    vmo_addr = operation->rw_bytes.offset_data_vmo;
    length = operation->rw_bytes.length;
  } else {
    nand_addr = static_cast<uint64_t>(operation->rw.offset_nand) * params_.page_size;
    vmo_addr = static_cast<uint64_t>(operation->rw.offset_data_vmo) * params_.page_size;
    length = static_cast<uint64_t>(operation->rw.length) * params_.page_size;
  }
  void* addr = reinterpret_cast<char*>(mapped_addr_) + nand_addr;

  if (operation->command == NAND_OP_READ || operation->command == NAND_OP_READ_BYTES) {
    operation->rw.corrected_bit_flips = 0;
    return zx_vmo_write(operation->rw.data_vmo, addr, vmo_addr, length);
  }

  if (bytes) {
    ZX_DEBUG_ASSERT(operation->command == NAND_OP_WRITE_BYTES);
  } else {
    ZX_DEBUG_ASSERT(operation->command == NAND_OP_WRITE);
    // Likely something bad is going on if writing multiple blocks.
    ZX_DEBUG_ASSERT_MSG(operation->rw.length <= params_.pages_per_block, "Writing multiple blocks");
    ZX_DEBUG_ASSERT_MSG(
        operation->rw.offset_nand / params_.pages_per_block ==
            (operation->rw.offset_nand + operation->rw.length - 1) / params_.pages_per_block,
        "Writing multiple blocks");
  }

  return zx_vmo_read(operation->rw.data_vmo, addr, vmo_addr, length);
}

zx_status_t NandDevice::ReadWriteOob(nand_operation_t* operation) {
  if (operation->rw.oob_vmo == ZX_HANDLE_INVALID) {
    return ZX_OK;
  }

  uint64_t nand_addr =
      MainDataSize() + static_cast<uint64_t>(operation->rw.offset_nand) * params_.oob_size;
  uint64_t vmo_addr = static_cast<uint64_t>(operation->rw.offset_oob_vmo) * params_.page_size;
  uint64_t length = static_cast<uint64_t>(operation->rw.length) * params_.oob_size;
  void* addr = reinterpret_cast<char*>(mapped_addr_) + nand_addr;

  if (operation->command == NAND_OP_READ) {
    operation->rw.corrected_bit_flips = 0;
    return zx_vmo_write(operation->rw.oob_vmo, addr, vmo_addr, length);
  }

  ZX_DEBUG_ASSERT(operation->command == NAND_OP_WRITE);
  return zx_vmo_read(operation->rw.oob_vmo, addr, vmo_addr, length);
}

zx_status_t NandDevice::Erase(nand_operation_t* operation) {
  ZX_DEBUG_ASSERT(operation->command == NAND_OP_ERASE);

  uint64_t block_size = static_cast<uint64_t>(params_.page_size) * params_.pages_per_block;
  uint64_t nand_addr = static_cast<uint64_t>(operation->erase.first_block) * block_size;
  uint64_t length = static_cast<uint64_t>(operation->erase.num_blocks) * block_size;
  void* addr = reinterpret_cast<char*>(mapped_addr_) + nand_addr;

  memset(addr, 0xff, length);

  // Clear the OOB area:
  uint64_t oob_per_block = static_cast<uint64_t>(params_.oob_size) * params_.pages_per_block;
  length = static_cast<uint64_t>(operation->erase.num_blocks) * oob_per_block;
  nand_addr = MainDataSize() + static_cast<uint64_t>(operation->erase.first_block) * oob_per_block;
  addr = reinterpret_cast<char*>(mapped_addr_) + nand_addr;

  memset(addr, 0xff, length);

  static_assert(std::atomic_ref<uint32_t>::is_always_lock_free);
  uint32_t* ptr = reinterpret_cast<uint32_t*>(wear_info_.start());
  if (ptr) {
    for (uint32_t i = 0; i < operation->erase.num_blocks; ++i) {
      std::atomic_ref<uint32_t> counter(ptr[operation->erase.first_block + i]);
      counter.fetch_add(1, std::memory_order_relaxed);
    }
  }

  return ZX_OK;
}

void NandDevice::DevfsConnect(fidl::ServerEnd<fuchsia_hardware_nand::RamNand> server) {
  bindings_.AddBinding(dispatcher_, std::move(server), this, fidl::kIgnoreBindingClosure);
}

void NandDevice::OnChildNodeClosed(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                   zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    if (status != ZX_ERR_CANCELED) {
      fdf::error("Child node wait error: {}", zx_status_get_string(status));
    }
    if (unlink_completer_) {
      unlink_completer_->Reply(ZX_OK);
      unlink_completer_.reset();
    }
    return;
  }
  fdf::info("Child node closed, destroying ram-nand device {}", device_name_);
  if (unlink_completer_) {
    unlink_completer_->Reply(ZX_OK);
    unlink_completer_.reset();
  }
  if (on_unlink_) {
    std::move(on_unlink_)();
  }
}

}  // namespace ram_nand
