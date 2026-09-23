// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_intel_buffer.h"

MsdIntelBuffer::MsdIntelBuffer(std::unique_ptr<magma::PlatformBuffer> platform_buf)
    : platform_buf_(std::move(platform_buf)) {}

std::unique_ptr<MsdIntelBuffer> MsdIntelBuffer::Import(zx::vmo handle, uint64_t client_id) {
  auto platform_buf = magma::PlatformBuffer::Import(std::move(handle));
  if (!platform_buf)
    return DRETP(nullptr, "PlaformBuffer::Import failed");

  platform_buf->set_local_id(client_id);

  return std::unique_ptr<MsdIntelBuffer>(new MsdIntelBuffer(std::move(platform_buf)));
}

std::unique_ptr<MsdIntelBuffer> MsdIntelBuffer::Create(uint64_t size, const char* name) {
  auto platform_buf = magma::PlatformBuffer::Create(size, name);
  if (!platform_buf)
    return DRETP(nullptr, "PlatformBuffer::Create failed");

  return std::unique_ptr<MsdIntelBuffer>(new MsdIntelBuffer(std::move(platform_buf)));
}
