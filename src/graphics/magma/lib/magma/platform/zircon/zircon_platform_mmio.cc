// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_platform_mmio.h"

#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>

namespace magma {

ZirconPlatformMmio::ZirconPlatformMmio(fdf::MmioBuffer mmio)
    : PlatformMmio(mmio.get(), mmio.get_size()), mmio_(std::move(mmio)) {}

bool ZirconPlatformMmio::Pin(const zx::bti& bti) {
  zx_status_t status = mmio_.Pin(bti, &pinned_mmio_);
  if (status != ZX_OK) {
    return DRETF(false, "Failed to pin mmio: %d\n", status);
  }
  return true;
}

uint64_t ZirconPlatformMmio::physical_address() {
  MAGMA_DASSERT(pinned_mmio_);
  return pinned_mmio_->get_paddr();
}

ZirconPlatformMmio::~ZirconPlatformMmio() { DLOG("ZirconPlatformMmio dtor"); }

}  // namespace magma
