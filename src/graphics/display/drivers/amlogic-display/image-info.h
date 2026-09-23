// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_IMAGE_INFO_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_IMAGE_INFO_H_

#include <fidl/fuchsia.hardware.amlogiccanvas/cpp/wire.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/bti.h>
#include <zircon/types.h>

#include <optional>

#include <fbl/intrusive_double_list.h>

namespace amlogic_display {

struct ImageInfo : public fbl::DoublyLinkedListable<std::unique_ptr<ImageInfo>> {
  std::optional<fidl::UnownedClientEnd<fuchsia_hardware_amlogiccanvas::Device>> canvas;
  uint8_t canvas_idx;
  uint32_t image_height;
  uint32_t image_width;

  PixelFormatAndModifier pixel_format;
  bool is_afbc;
  zx::pmt pmt;
  zx_paddr_t paddr;

  ~ImageInfo();
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_IMAGE_INFO_H_
