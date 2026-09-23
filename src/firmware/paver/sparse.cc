// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/sparse.h"

#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/status.h>

#include <cstdarg>
#include <cstddef>
#include <vector>

#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/pave-logging.h"
#include "src/storage/lib/sparse/c/sparse.h"

namespace paver {
namespace {

struct SparseIoBuffer {
  zx::vmo vmo;
  size_t size;

  SparseIoBuffer() = default;
  SparseIoBuffer(zx::vmo vmo, size_t size) : vmo(std::move(vmo)), size(size) {}

  bool Read(uint64_t offset, uint8_t* dst, size_t size) const {
    if (zx_status_t status = vmo.read(dst, offset, size); status != ZX_OK) {
      ERROR("Failed to read from VMO: %s\n", zx_status_get_string(status));
      return false;
    }
    return true;
  }

  static bool ReadFrom(SparseIoBufferHandle handle, uint64_t offset, uint8_t* dst, size_t size) {
    auto me = static_cast<const SparseIoBuffer*>(handle);
    return me->Read(offset, dst, size);
  }

  bool Write(uint64_t offset, const uint8_t* src, size_t size) const {
    if (zx_status_t status = vmo.write(src, offset, size); status != ZX_OK) {
      ERROR("Failed to write to VMO: %s\n", zx_status_get_string(status));
      return false;
    }
    return true;
  }

  static bool FillTo(SparseIoBufferHandle handle, uint32_t payload) {
    auto me = static_cast<const SparseIoBuffer*>(handle);
    size_t size = me->Size();
    if (size % sizeof(payload) != 0) {
      return false;
    }

    std::vector<uint32_t> fill(size / sizeof(payload), payload);
    return me->Write(0, reinterpret_cast<const uint8_t*>(fill.data()),
                     fill.size() * sizeof(payload));
  }

  size_t Size() const {
    size_t size;
    return vmo.get_size(&size) == ZX_OK ? size : 0;
  }

  static size_t SizeOf(SparseIoBufferHandle handle) {
    auto me = static_cast<const SparseIoBuffer*>(handle);
    return me->Size();
  }

  static SparseIoBufferOps Interface() {
    return SparseIoBufferOps{.size = SizeOf, .read = ReadFrom, .fill = FillTo};
  }
};

struct SparseIoContext {
  PartitionClient& partition;
  BlockPartitionClient block;
  uint64_t block_size;
  fzl::OwnedVmoMapper transfer_vmo;
  storage::OwnedVmoid transfer_vmoid;

  static constexpr uint64_t kTransferVmoSize = 1024ul * 1024;

  static zx::result<SparseIoContext> Create(PartitionClient& client) {
    fzl::OwnedVmoMapper transfer_vmo;
    if (zx_status_t status = transfer_vmo.CreateAndMap(kTransferVmoSize, "transfer");
        status != ZX_OK) {
      ERROR("Failed to create transfer VMO: %s\n", zx_status_get_string(status));
      return zx::error(status);
    }
    if (!client.SupportsBlockPartition()) {
      ERROR("tlp partition is not a block device\n");
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    BlockPartitionClient* block = reinterpret_cast<BlockPartitionClient*>(&client);

    zx::result block_size = client.GetBlockSize();
    if (block_size.is_error()) {
      ERROR("Failed to get block size: %s\n", block_size.status_string());
      return block_size.take_error();
    }
    zx::result vmoid = block->RegisterVmoid(transfer_vmo.vmo());
    if (vmoid.is_error()) {
      return vmoid.take_error();
    }
    return zx::ok(SparseIoContext{client, std::move(*block), *block_size, std::move(transfer_vmo),
                                  std::move(*vmoid)});
  }

  bool Write(uint64_t device_offset, const zx::vmo& src, uint64_t src_offset, size_t size) {
    if (size % block_size > 0) {
      ERROR("Unaligned writes aren't supported; size is %zu but block_size is %lu\n", size,
            block_size);
      return false;
    }
    if (device_offset % block_size == 0 && src_offset % block_size == 0) {
      // Happy days, the write is already aligned.
      zx::result vmoid = block.RegisterVmoid(src);
      if (vmoid.is_error()) {
        ERROR("Failed to register VMO: %s\n", vmoid.status_string());
        return false;
      }
      if (zx::result result =
              block.Write(vmoid->get(), size, device_offset / block_size, src_offset / block_size);
          result.is_error()) {
        if (result.status_value() == ZX_ERR_OUT_OF_RANGE) {
          ERROR("Write failed with ZX_ERR_OUT_OF_RANGE: image likely exceeds partition size.\n");
        }
        ERROR("Failed to write %zu @ %lu from %lu: %s\n", size, device_offset / block_size,
              src_offset / block_size, result.status_string());
        return false;
      }
      return true;
    }
    // Otherwise, shuffle data through the transfer VMO.
    size_t written = 0;
    while (written < size) {
      size_t to_write = std::min(size - written, transfer_vmo.size());
      if (zx_status_t status = src.read(transfer_vmo.start(), src_offset + written, to_write);
          status != ZX_OK) {
        ERROR("Failed to read from source VMO: %s\n", zx_status_get_string(status));
        return false;
      }
      if (zx::result result = block.Write(transfer_vmoid.get(), to_write,
                                          (device_offset + written) / block_size, 0);
          result.is_error()) {
        if (result.status_value() == ZX_ERR_OUT_OF_RANGE) {
          ERROR(
              "Write failed with ZX_ERR_OUT_OF_RANGE. The image likely exceeds the partition size.\n");
        }
        ERROR("Failed to write %zu @ %lu: %s\n", to_write, (device_offset + written) / block_size,
              result.status_string());
        return false;
      }
      written += to_write;
    }
    return true;
  }

  static bool WriteTo(void* ctx, uint64_t device_offset, SparseIoBufferHandle src,
                      uint64_t src_offset, size_t size) {
    auto me = static_cast<SparseIoContext*>(ctx);
    auto buffer = static_cast<SparseIoBuffer*>(src);
    return me->Write(device_offset, buffer->vmo, src_offset, size);
  }
};

int Log(const char* msg, ...) {
  va_list args;
  va_start(args, msg);
  fprintf(stderr, "paver: ");
  int res = vfprintf(stderr, msg, args);
  va_end(args);
  return res;
}

}  // namespace

zx::result<> WriteSparse(PartitionClient& partition, const PartitionSpec& spec, zx::vmo payload_vmo,
                         size_t payload_size) {
  zx::result block_size = partition.GetBlockSize();
  if (block_size.is_error()) {
    ERROR("Couldn't get partition \"%s\" block size\n", spec.ToString().c_str());
    return block_size.take_error();
  }
  const size_t fill_size_bytes = 1024 * block_size.value();

  zx::vmo scratch_vmo;
  if (zx_status_t status = zx::vmo::create(fill_size_bytes, 0, &scratch_vmo); status != ZX_OK) {
    ERROR("Failed to create scratch VMO: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }
  auto fill_buffer = std::make_unique<SparseIoBuffer>(std::move(scratch_vmo), fill_size_bytes);
  zx::result context = SparseIoContext::Create(partition);
  if (context.is_error()) {
    ERROR("Failed to create I/O context: %s\n", context.status_string());
    return context.take_error();
  }
  SparseIoInterface io = {
      .ctx = &context.value(),
      .fill_handle = fill_buffer.get(),
      .handle_ops = SparseIoBuffer::Interface(),
      .write = SparseIoContext::WriteTo,
  };

  auto payload = std::make_unique<SparseIoBuffer>(std::move(payload_vmo), payload_size);
  if (!sparse_unpack_image(&io, Log, payload.get())) {
    ERROR("Failed to pave sparse image\n");
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

}  // namespace paver
