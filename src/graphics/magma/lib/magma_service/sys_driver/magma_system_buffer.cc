// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "magma_system_buffer.h"

#include <lib/magma/util/macros.h>

namespace msd {

MagmaSystemBuffer::MagmaSystemBuffer(std::unique_ptr<magma::PlatformBuffer> platform_buf,
                                     std::unique_ptr<msd::Buffer> msd_buf)
    : platform_buf_(std::move(platform_buf)), msd_buf_(std::move(msd_buf)) {}

std::unique_ptr<MagmaSystemBuffer> MagmaSystemBuffer::Create(
    msd::Driver* driver, std::unique_ptr<magma::PlatformBuffer> platform_buffer) {
  if (!platform_buffer)
    return MAGMA_DRETP(nullptr, "Failed to create PlatformBuffer");

  uint32_t duplicate_handle;
  if (!platform_buffer->duplicate_handle(&duplicate_handle))
    return MAGMA_DRETP(nullptr, "failed to get duplicate_handle");

  auto msd_buf = driver->MsdImportBuffer(zx::vmo(duplicate_handle), platform_buffer->id());
  if (!msd_buf)
    return MAGMA_DRETP(nullptr,
                       "Failed to import newly allocated buffer into the MSD Implementation");

  return std::unique_ptr<MagmaSystemBuffer>(
      new MagmaSystemBuffer(std::move(platform_buffer), std::move(msd_buf)));
}
}  // namespace msd
