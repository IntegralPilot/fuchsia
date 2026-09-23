// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/engine/engine_types.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

namespace flatland {

DisplaySrcDstFrames DisplaySrcDstFrames::New(SrcToDest geometry) {
  types::Rectangle image_source({
      .x = static_cast<int32_t>(geometry.src.x()),
      .y = static_cast<int32_t>(geometry.src.y()),
      .width = static_cast<int32_t>(geometry.src.width()),
      .height = static_cast<int32_t>(geometry.src.height()),
  });

  types::Rectangle display_destination({
      .x = static_cast<int32_t>(geometry.dest.x()),
      .y = static_cast<int32_t>(geometry.dest.y()),
      .width = static_cast<int32_t>(geometry.dest.width()),
      .height = static_cast<int32_t>(geometry.dest.height()),
  });
  return {.src = image_source, .dst = display_destination};
}

}  // namespace flatland
