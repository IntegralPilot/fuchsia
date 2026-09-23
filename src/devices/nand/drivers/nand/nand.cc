// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/nand/drivers/nand/nand.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include <algorithm>
#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/nand/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>

// TODO: Investigate elimination of unmap.
// This code does vx_vmar_map/unmap and copies data in/out of the
// mapped virtual address. Unmapping is expensive, but required (a closing
// of the vmo does not unmap, so not unmapping will quickly lead to memory
// exhaustion. Check to see if we can do something different - is vmo_read/write
// cheaper than mapping and unmapping (which will cause TLB flushes) ?

namespace nand {

zx_status_t NandDriver::ReadPage(uint8_t* data, uint8_t* oob, uint32_t nand_page,
                                 uint32_t* corrected_bits, size_t retries) {
  // Always grab the oob, whether the caller wants it or not.
  uint8_t* oob_result = oob ? oob : oob_buffer_.get();
  if (data && dangerous_reads_cache_->GetPage(nand_page, data, oob_result)) {
    *corrected_bits = 0;
    return ZX_OK;
  }

  zx_status_t status = ZX_ERR_INTERNAL;
  size_t retry = 0;
  bool ecc_failure = false;
  for (; status != ZX_OK && retry < retries; retry++) {
    status = raw_nand_.ReadPageHwecc(nand_page, data, nand_info_.page_size, nullptr, oob_result,
                                     nand_info_.oob_size, nullptr, corrected_bits);
    if (status == ZX_OK) {
      // Only record the returned corrected bits number on success, otherwise it is undefined.
      read_ecc_bit_flips_.Insert(*corrected_bits);
    } else {
      read_internal_failure_.Add(1);
      fdf::warn("Retrying Read@{}", nand_page);
      if (status == ZX_ERR_IO_DATA_INTEGRITY) {
        ecc_failure = true;
        read_ecc_bit_flips_.Insert(nand_info_.ecc_bits + 1);
      }
    }
  }

  if (status != ZX_OK) {
    read_failure_.Add(1);
    read_attempts_.Insert(ULONG_MAX);
    fdf::warn("Read error {}, exhausted all retries", zx_status_get_string(status));
  } else {
    read_attempts_.Insert(retry);
    if (retry > 1) {
      fdf::info("Successfully read@{} on retry {}", nand_page, retry - 1);
    }
  }
  // If we get a failed ECC from the nand device, report up the stack that
  // things are going badly, in case the repeated read goes inexplicably better.
  if (ecc_failure) {
    *corrected_bits = nand_info_.ecc_bits;
  }
  if (status == ZX_OK && data && *corrected_bits > nand_info_.ecc_bits / 2) {
    // Cache this page, since we should re-read it for a block transfer soon.
    dangerous_reads_cache_->Insert(nand_page, data, oob_result);
  }
  return status;
}

zx_status_t NandDriver::EraseOp(nand_operation_t* nand_op) {
  uint32_t nand_page;

  for (uint32_t i = 0; i < nand_op->erase.num_blocks; i++) {
    nand_page = (nand_op->erase.first_block + i) * nand_info_.pages_per_block;
    // Purge cache for deleted content.
    dangerous_reads_cache_->PurgeRange(nand_page, nand_info_.pages_per_block);
    zx_status_t status = raw_nand_.EraseBlock(nand_page);
    if (status != ZX_OK) {
      fdf::error("Erase of block {} failed", nand_op->erase.first_block + i);
      return status;
    }
  }
  return ZX_OK;
}

zx_status_t NandDriver::MapVmos(const nand_operation_t& nand_op, fzl::VmoMapper* data,
                                uint8_t** vaddr_data, fzl::VmoMapper* oob, uint8_t** vaddr_oob) {
  const size_t page_size = static_cast<size_t>(zx_system_get_page_size());
  zx_status_t status;
  if (nand_op.rw.data_vmo != ZX_HANDLE_INVALID) {
    const auto vmo = zx::unowned_vmo(nand_op.rw.data_vmo);
    const size_t offset_bytes = nand_op.rw.offset_data_vmo * nand_info_.page_size;
    const size_t aligned_offset_bytes = fbl::round_down(offset_bytes, page_size);
    const size_t page_offset_bytes = offset_bytes - aligned_offset_bytes;
    status = data->Map(*vmo, aligned_offset_bytes,
                       nand_op.rw.length * nand_info_.page_size + page_offset_bytes,
                       ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_MAP_RANGE);
    if (status != ZX_OK) {
      fdf::error("Cannot map data vmo: {}", zx_status_get_string(status));
      return status;
    }
    *vaddr_data = reinterpret_cast<uint8_t*>(data->start()) + page_offset_bytes;
  }

  // Map oob.
  if (nand_op.rw.oob_vmo != ZX_HANDLE_INVALID) {
    const auto vmo = zx::unowned_vmo(nand_op.rw.oob_vmo);
    const size_t offset_bytes = nand_op.rw.offset_oob_vmo * nand_info_.page_size;
    const size_t aligned_offset_bytes = fbl::round_down(offset_bytes, page_size);
    const size_t page_offset_bytes = offset_bytes - aligned_offset_bytes;
    status = oob->Map(*vmo, aligned_offset_bytes,
                      nand_op.rw.length * nand_info_.oob_size + page_offset_bytes,
                      ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_MAP_RANGE);
    if (status != ZX_OK) {
      fdf::error("Cannot map oob vmo: {}", zx_status_get_string(status));
      return status;
    }
    *vaddr_oob = reinterpret_cast<uint8_t*>(oob->start()) + page_offset_bytes;
  }
  return ZX_OK;
}

zx_status_t NandDriver::ReadOp(nand_operation_t* nand_op) {
  fzl::VmoMapper data;
  fzl::VmoMapper oob;
  uint8_t* vaddr_data = nullptr;
  uint8_t* vaddr_oob = nullptr;

  zx_status_t status = MapVmos(*nand_op, &data, &vaddr_data, &oob, &vaddr_oob);
  if (status != ZX_OK) {
    return status;
  }

  uint32_t max_corrected_bits = 0;
  for (uint32_t i = 0; i < nand_op->rw.length; i++) {
    uint32_t ecc_correct = 0;
    status = ReadPage(vaddr_data, vaddr_oob, nand_op->rw.offset_nand + i, &ecc_correct,
                      kNandReadRetries);
    if (status != ZX_OK) {
      fdf::error("Read data error {} at page offset {}", status, nand_op->rw.offset_nand + i);
      break;
    }
    max_corrected_bits = std::max(max_corrected_bits, ecc_correct);

    if (vaddr_data) {
      vaddr_data += nand_info_.page_size;
    }
    if (vaddr_oob) {
      vaddr_oob += nand_info_.oob_size;
    }
  }
  nand_op->rw.corrected_bit_flips = max_corrected_bits;

  return status;
}

zx_status_t NandDriver::WriteOp(nand_operation_t* nand_op) {
  fzl::VmoMapper data;
  fzl::VmoMapper oob;
  uint8_t* vaddr_data = nullptr;
  uint8_t* vaddr_oob = nullptr;

  zx_status_t status = MapVmos(*nand_op, &data, &vaddr_data, &oob, &vaddr_oob);
  if (status != ZX_OK) {
    return status;
  }

  for (uint32_t i = 0; i < nand_op->rw.length; i++) {
    status = raw_nand_.WritePageHwecc(vaddr_data, nand_info_.page_size, vaddr_oob,
                                      nand_info_.oob_size, nand_op->rw.offset_nand + i);
    if (status != ZX_OK) {
      fdf::error("Write data error {} at page offset {}", zx_status_get_string(status),
                 nand_op->rw.offset_nand + i);
      break;
    }

    if (vaddr_data) {
      vaddr_data += nand_info_.page_size;
    }
    if (vaddr_oob) {
      vaddr_oob += nand_info_.oob_size;
    }
  }

  return status;
}

void NandDriver::PerformTransaction(Transaction txn) {
  zx_status_t status = ZX_OK;

  switch (txn.operation()->command) {
    case NAND_OP_READ:
      status = ReadOp(txn.operation());
      break;
    case NAND_OP_WRITE:
      status = WriteOp(txn.operation());
      break;
    case NAND_OP_ERASE:
      status = EraseOp(txn.operation());
      break;
    default:
      status = ZX_ERR_NOT_SUPPORTED;
      break;
  }
  txn.Complete(status);
}

void NandDriver::NandQuery(nand_info_t* info_out, size_t* nand_op_size_out) {
  memcpy(info_out, &nand_info_, sizeof(*info_out));
  *nand_op_size_out = Transaction::OperationSize(sizeof(nand_operation_t));
}

void NandDriver::NandQueue(nand_operation_t* op, nand_queue_callback completion_cb, void* cookie) {
  if (completion_cb == nullptr) {
    fdf::debug("nand op {:p} completion_cb unset!", static_cast<void*>(op));
    fdf::debug("cannot queue command!");
    return;
  }

  Transaction txn(op, completion_cb, cookie, sizeof(nand_operation_t));

  switch (op->command) {
    case NAND_OP_READ:
    case NAND_OP_WRITE: {
      if (op->rw.offset_nand >= num_nand_pages_ || !op->rw.length ||
          (num_nand_pages_ - op->rw.offset_nand) < op->rw.length) {
        txn.Complete(ZX_ERR_OUT_OF_RANGE);
        return;
      }
      if (op->rw.data_vmo == ZX_HANDLE_INVALID && op->rw.oob_vmo == ZX_HANDLE_INVALID) {
        txn.Complete(ZX_ERR_BAD_HANDLE);
        return;
      }
      break;
    }
    case NAND_OP_ERASE:
      if (!op->erase.num_blocks || op->erase.first_block >= nand_info_.num_blocks ||
          (op->erase.num_blocks > (nand_info_.num_blocks - op->erase.first_block))) {
        txn.Complete(ZX_ERR_OUT_OF_RANGE);
        return;
      }
      break;

    default:
      txn.Complete(ZX_ERR_NOT_SUPPORTED);
      return;
  }

  // TODO: UPDATE STATS HERE.
  async::PostTask(transaction_performer_dispatcher_.async_dispatcher(),
                  [this, txn = std::move(txn)]() mutable { PerformTransaction(std::move(txn)); });
}

zx_status_t NandDriver::NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                                   size_t* num_bad_blocks) {
  *num_bad_blocks = 0;
  return ZX_ERR_NOT_SUPPORTED;
}

void NandDriver::Stop(fdf::StopCompleter completer) {
  prepare_stop_completer_.emplace(std::move(completer));
  transaction_performer_dispatcher_.ShutdownAsync();
}

zx::result<> NandDriver::Start(fdf::DriverContext context) {
  component_inspector_ = context.CreateInspector(this);
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result raw_nand = compat::ConnectBanjo<ddk::RawNandProtocolClient>(incoming);
  if (raw_nand.is_error()) {
    fdf::error("Failed to connect to raw-nand banjo protocol: {}", raw_nand);
    return raw_nand.take_error();
  }
  raw_nand_ = std::move(raw_nand.value());

  zx_status_t status = raw_nand_.GetNandInfo(&nand_info_);
  if (status != ZX_OK) {
    fdf::error("Failed to get nand info: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  dangerous_reads_cache_ =
      std::make_unique<ReadCache>(8, nand_info_.page_size, nand_info_.oob_size);
  oob_buffer_ = std::make_unique<uint8_t[]>(nand_info_.oob_size);

  num_nand_pages_ = nand_info_.num_blocks * nand_info_.pages_per_block;

  root_ = component_inspector_->root().CreateChild("nand");
  // 32 buckets: 0-31. Current devices only use up to BCH30.
  // Will populate read failures as ecc bits + 1.
  read_ecc_bit_flips_ = root_.CreateLinearUintHistogram("read_ecc_bit_flips", 0, 1, 32);
  // Buckets 0, 1, 2, 4...128. Failures will be maxint and dump in the overflow bucket.
  read_attempts_ = root_.CreateExponentialUintHistogram("read_attempts", 0, 1, 2, 9);
  read_internal_failure_ = root_.CreateUint("read_internal_failure", 0);
  read_failure_ = root_.CreateUint("read_failure", 0);

  // Set a scheduling role for the transaction-performing dispatcher.
  // This is required in order to service the blobfs-pager-thread, which is on a deadline profile.
  // This will no longer be needed once we have the ability to propagate deadlines. Until then, we
  // need to set deadline profiles for all threads that the blobfs-pager-thread interacts with in
  // order to service page requests.
  static constexpr std::string_view kRoleName = "fuchsia.devices.nand.drivers.nand.device";
  zx::result transaction_dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "transaction-performer",
      [this](fdf_dispatcher_t*) {
        if (prepare_stop_completer_.has_value()) {
          fdf::StopCompleter completer = std::move(prepare_stop_completer_).value();
          completer(zx::ok());
        }
      },
      kRoleName);
  if (transaction_dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher: {}", transaction_dispatcher);
    return transaction_dispatcher.take_error();
  }
  transaction_performer_dispatcher_ = std::move(transaction_dispatcher.value());

  compat::DeviceServer::BanjoConfig banjo_config{.default_proto_id = ZX_PROTOCOL_NAND};
  banjo_config.callbacks[ZX_PROTOCOL_NAND] = nand_server_.callback();

  zx::result result = compat_server_.Initialize(
      incoming, outgoing(), context.node_name(), kChildNodeName,
      compat::ForwardMetadata::Some({DEVICE_METADATA_PRIVATE}), std::move(banjo_config));
  if (result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result);
    return result.take_error();
  }

  if (zx::result result =
          partition_map_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming);
      result.is_error()) {
    fdf::error("Failed to forward partition map: {}", result);
    return result.take_error();
  }

  const std::array<fuchsia_driver_framework::NodeProperty2, 2> properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                         static_cast<uint32_t>(bind_fuchsia_nand::BIND_PROTOCOL_DEVICE)),
      fdf::MakeProperty2(bind_fuchsia::NAND_CLASS, NAND_CLASS_PARTMAP),
  };

  std::vector<fuchsia_driver_framework::Offer> offers = compat_server_.CreateOffers2();
  std::optional partition_map_offer = partition_map_metadata_server_.CreateOffer();
  if (partition_map_offer.has_value()) {
    offers.emplace_back(std::move(partition_map_offer.value()));
  }

  zx::result child = AddChild(kChildNodeName, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to create child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

}  // namespace nand

FUCHSIA_DRIVER_EXPORT2(nand::NandDriver);
