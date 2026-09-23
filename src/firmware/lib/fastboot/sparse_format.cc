// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/firmware/lib/fastboot/sparse_format.h"

#include <algorithm>

#include "src/storage/lib/sparse/c/sparse.h"
#include "third_party/android/platform/system/core/libsparse/sparse_format.h"
// The above header includes the
// third_party/android/platform/system/core/libsparse/sparse_defs.h which defines a `error()`
// macro. It confuses the compiler when we try to create a zx::result with zx::error(). Thus,
// we need to undefine the macro.
#ifdef error
#undef error
#endif

namespace fastboot {

namespace {

bool VmoWrite(void* dst_handle, uint64_t dst_offset, SparseIoBufferHandle src_handle,
              uint64_t src_offset, size_t size) {
  const auto& src_buffer = *reinterpret_cast<fzl::OwnedVmoMapper*>(src_handle);
  if ((size + src_offset) > src_buffer.size()) {
    return false;
  }
  const uint8_t* src = reinterpret_cast<const uint8_t*>(src_buffer.start());
  const auto& dst_buffer = *reinterpret_cast<zx::vmo*>(dst_handle);
  return dst_buffer.write(src + src_offset, dst_offset, size) == ZX_OK;
}

SparseIoBufferOps MappedVmoOps() {
  struct OwnedVmoMapperOps {
    static size_t Size(SparseIoBufferHandle handle) {
      const auto& buffer = *reinterpret_cast<fzl::OwnedVmoMapper*>(handle);
      return buffer.size();
    }

    static bool Read(SparseIoBufferHandle handle, uint64_t offset, uint8_t* dst, size_t size) {
      const auto& buffer = *reinterpret_cast<fzl::OwnedVmoMapper*>(handle);
      const uint8_t* src = reinterpret_cast<const uint8_t*>(buffer.start());
      std::memcpy(dst, src + offset, size);
      return true;
    }

    static bool Fill(SparseIoBufferHandle handle, uint32_t payload) {
      const auto& fill_buffer = *reinterpret_cast<fzl::OwnedVmoMapper*>(handle);
      if (fill_buffer.size() % sizeof payload != 0) {
        return false;
      }
      const size_t fill_amount = fill_buffer.size() / sizeof payload;
      uint32_t* dst = reinterpret_cast<uint32_t*>(fill_buffer.start());
      std::fill(dst, dst + fill_amount, payload);
      return true;
    }
  };

  return {
      .size = &OwnedVmoMapperOps::Size,
      .read = &OwnedVmoMapperOps::Read,
      .fill = &OwnedVmoMapperOps::Fill,
  };
}

}  // namespace

std::optional<uint64_t> GetUnsparsedSize(const void* buffer, uint64_t size) {
  if (size < sizeof(sparse_header_t)) {
    return std::nullopt;
  }
  const sparse_header_t* header = reinterpret_cast<const sparse_header_t*>(buffer);
  if (header->magic != SPARSE_HEADER_MAGIC) {
    return std::nullopt;
  }
  return uint64_t{header->blk_sz} * uint64_t{header->total_blks};
}

std::optional<uint64_t> GetUnsparsedSize(const fzl::OwnedVmoMapper& buffer) {
  return GetUnsparsedSize(buffer.start(), buffer.size());
}

bool IsSparseFormat(const fzl::OwnedVmoMapper& buffer) {
  return GetUnsparsedSize(buffer).has_value();
}

zx::result<> Unsparse(fzl::OwnedVmoMapper& src, zx::vmo& dst, fzl::OwnedVmoMapper& fill_buffer,
                      UnsparseErrorLogger logger) {
  if (!logger) {
    logger = &sparse_nop_logger;
  }
  SparseIoInterface io = {
      .ctx = &dst,
      .fill_handle = &fill_buffer,
      .handle_ops = MappedVmoOps(),
      .write = &VmoWrite,
  };

  if (!sparse_unpack_image(&io, logger, &src)) {
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }
  return zx::ok();
}

}  // namespace fastboot
