// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_ENGINE_TYPES_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_ENGINE_TYPES_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>

#include <span>

#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/types/rectangle.h"

#include <glm/glm.hpp>

namespace flatland {

// Struct to represent the display's flatland info. The TransformHandle must be the root
// transform of the root Flatland instance. A new DisplayInfo struct is added to the
// display_map_ when a client calls AddDisplay().
struct DisplayInfo {
  // The width and height of the display in pixels.
  glm::uvec2 dimensions;

  // The pixel formats available on this particular display.
  std::vector<fuchsia_images2::PixelFormat> formats;

  // The maximum number of hardware layers supported by this display.
  uint32_t max_layer_count = 0;
};

// Struct to combine the source and destination rectangles used to set a layer's
// position on the display. The src rectangle represents the (cropped) UV coordinates
// of the image and the dst rectangle represents the position in screen space where
// the layer will be placed.
struct DisplaySrcDstFrames {
  types::Rectangle src;
  types::Rectangle dst;

  // When setting an image on a layer in the display, you have to specify the "source"
  // and "destination", where the source represents the pixel offsets and dimensions to
  // use from the image and the destination represents where on the display the (cropped)
  // image will go in pixel coordinates. This exactly mirrors the setup we have in the
  // Rectangle2D struct and ImageMetadata struct, so we just need to convert that over to
  // the proper display controller readable format. The input rectangle contains both the
  // source and destination information.
  static DisplaySrcDstFrames New(SrcToDest geometry);
};

// The data that gets forwarded either to the display or the software renderer. The lengths
// of |layers| and |images| must be the same, and each layer/image pair for a given
// index represents a single renderable object.
struct RenderData {
  // TODO(https://fxbug.dev/42149711): should we remove this, and pass to RenderFrame() as a
  // std::map of RenderData keyed by display_id?  That would have the benefit of guaranteeing by
  // construction that each display_id could only appear once.
  display::DisplayId display_id;
  std::span<const ResolvedLayer> layers;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_ENGINE_TYPES_H_
