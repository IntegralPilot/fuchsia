// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_BUFFER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_BUFFER_H_

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma_service/msd.h>

#include <functional>
#include <memory>

namespace msd {
class MagmaSystemBuffer {
 public:
  static std::unique_ptr<MagmaSystemBuffer> Create(
      msd::Driver* driver, std::unique_ptr<magma::PlatformBuffer> platform_buffer);
  ~MagmaSystemBuffer() {}

  uint64_t size() { return platform_buf_->size(); }
  uint64_t id() { return platform_buf_->id(); }

  // note: this does not relinquish ownership of the PlatformBuffer
  magma::PlatformBuffer* platform_buffer() { return platform_buf_.get(); }

  msd::Buffer* msd_buf() { return msd_buf_.get(); }

 private:
  MagmaSystemBuffer(std::unique_ptr<magma::PlatformBuffer> platform_buf,
                    std::unique_ptr<msd::Buffer> msd_buf);
  std::unique_ptr<magma::PlatformBuffer> platform_buf_;
  std::unique_ptr<msd::Buffer> msd_buf_;
};

}  // namespace msd
#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_BUFFER_H_
