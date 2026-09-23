// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_INTEL_BUFFER_H
#define MSD_INTEL_BUFFER_H

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma/platform/platform_event.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

#include <atomic>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <vector>

#include "types.h"

class AddressSpace;

class MsdIntelBuffer {
 public:
  static std::unique_ptr<MsdIntelBuffer> Import(zx::vmo handle, uint64_t client_id);
  static std::unique_ptr<MsdIntelBuffer> Create(uint64_t size, const char* name);

  magma::PlatformBuffer* platform_buffer() {
    DASSERT(platform_buf_);
    return platform_buf_.get();
  }

 private:
  MsdIntelBuffer(std::unique_ptr<magma::PlatformBuffer> platform_buf);

  std::unique_ptr<magma::PlatformBuffer> platform_buf_;
};

class MsdIntelAbiBuffer : public msd::Buffer {
 public:
  explicit MsdIntelAbiBuffer(std::shared_ptr<MsdIntelBuffer> ptr) : ptr_(std::move(ptr)) {}

  std::shared_ptr<MsdIntelBuffer> ptr() { return ptr_; }

 private:
  std::shared_ptr<MsdIntelBuffer> ptr_;
};

#endif  // MSD_INTEL_BUFFER_H
