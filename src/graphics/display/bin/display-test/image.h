// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_IMAGE_H_
#define SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_IMAGE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <zircon/types.h>

#include "src/graphics/display/lib/api-types/cpp/buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"

// Indices into event and event_ids
#define WAIT_EVENT 0

namespace display_test {

using image_import_t = struct image_import {
  display::ImageId id;
  zx::event events[1];
  display::EventId event_ids[1];
};

class Image {
 public:
  enum class Pattern {
    kCheckerboard,
    kBorder,
  };

  static Image* Create(const fidl::WireSyncClient<fuchsia_hardware_display::Coordinator>& dc,
                       uint32_t width, uint32_t height, fuchsia_images2::wire::PixelFormat format,
                       Pattern pattern, uint32_t fg_color, uint32_t bg_color,
                       fuchsia_images2::wire::PixelFormatModifier modifier);

  void Render(int32_t prev_step, int32_t step_num);

  void* buffer() { return buf_; }
  uint32_t width() { return width_; }
  uint32_t height() { return height_; }
  uint32_t stride() { return stride_; }
  fuchsia_images2::wire::PixelFormat format() { return format_; }
  fuchsia_images2::wire::PixelFormatModifier modifier() const { return modifier_; }

  fuchsia_hardware_display_types::wire::ImageMetadata GetMetadata() const;
  bool Import(const fidl::WireSyncClient<fuchsia_hardware_display::Coordinator>& dc,
              display::ImageId image_id, image_import_t* info_out) const;

 private:
  Image(uint32_t width, uint32_t height, int32_t stride, fuchsia_images2::wire::PixelFormat format,
        display::BufferCollectionId buffer_collection_id, void* buf, Pattern pattern,
        uint32_t fg_color, uint32_t bg_color, fuchsia_images2::wire::PixelFormatModifier modifier);

  void RenderNv12(int32_t prev_step, int32_t step_num);

  // pixel_generator takes a width and a height and generates a uint32_t pixel color for it.
  template <typename T>
  void RenderLinear(T pixel_generator, uint32_t start_y, uint32_t end_y);
  template <typename T>
  void RenderTiled(T pixel_generator, uint32_t start_y, uint32_t end_y);

  uint32_t width_;
  uint32_t height_;
  uint32_t stride_;
  fuchsia_images2::wire::PixelFormat format_;

  display::BufferCollectionId buffer_collection_id_;
  void* buf_;

  const Pattern pattern_;
  uint32_t fg_color_;
  uint32_t bg_color_;
  fuchsia_images2::PixelFormatModifier modifier_;
};

}  // namespace display_test

#endif  // SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_IMAGE_H_
