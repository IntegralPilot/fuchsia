// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_MMIO_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_MMIO_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio-pinned-buffer.h>
#include <lib/magma/platform/platform_mmio.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>

namespace magma {

class ZirconPlatformMmio : public PlatformMmio {
 public:
  explicit ZirconPlatformMmio(fdf::MmioBuffer mmio);

  ~ZirconPlatformMmio();
  bool Pin(const zx::bti& bti);
  uint64_t physical_address() override;

 private:
  fdf::MmioBuffer mmio_;
  std::optional<fdf::MmioPinnedBuffer> pinned_mmio_;
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_MMIO_H_
