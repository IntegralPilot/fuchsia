// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_VSI_BUFFER_H
#define MSD_VSI_BUFFER_H

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

class MsdVsiBuffer {
 public:
  static std::unique_ptr<MsdVsiBuffer> Import(zx::vmo handle, uint64_t client_id);
  static std::unique_ptr<MsdVsiBuffer> Create(uint64_t size, const char* name);

  magma::PlatformBuffer* platform_buffer() {
    DASSERT(platform_buf_);
    return platform_buf_.get();
  }

  explicit MsdVsiBuffer(std::unique_ptr<magma::PlatformBuffer> platform_buf)
      : platform_buf_(std::move(platform_buf)) {}

 private:
  std::unique_ptr<magma::PlatformBuffer> platform_buf_;
};

class MsdVsiAbiBuffer : public msd::Buffer {
 public:
  explicit MsdVsiAbiBuffer(std::shared_ptr<MsdVsiBuffer> ptr)
      : ptr_(std::move(ptr)), magic_(kMagic) {}

  static MsdVsiAbiBuffer* cast(msd::Buffer* buf) {
    DASSERT(buf);
    auto buffer = static_cast<MsdVsiAbiBuffer*>(buf);
    DASSERT(buffer->magic_ == kMagic);
    return buffer;
  }
  std::shared_ptr<MsdVsiBuffer> ptr() { return ptr_; }

 private:
  std::shared_ptr<MsdVsiBuffer> ptr_;
  static const uint32_t kMagic = 0x62756666;  // "buff"
  const uint32_t magic_;
};

#endif  // MSD_VSI_BUFFER_H
