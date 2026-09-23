// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/display-config.h"

#include <lib/inspect/cpp/vmo/types.h>
#include <zircon/assert.h>

#include <atomic>
#include <utility>

#include <fbl/intrusive_double_list.h>
#include <fbl/string_printf.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"

namespace display_coordinator {

DisplayConfig::DisplayConfig(display::DisplayId display_id,
                             fbl::Vector<display::PixelFormat> pixel_formats,
                             int engine_max_layer_count)
    : IdMappable(display_id),
      pixel_formats_(std::move(pixel_formats)),
      engine_max_layer_count_(engine_max_layer_count) {
  ZX_DEBUG_ASSERT(display_id != display::kInvalidDisplayId);
  ZX_DEBUG_ASSERT(engine_max_layer_count > 0);
  ZX_DEBUG_ASSERT(engine_max_layer_count <= display::EngineInfo::kMaxAllowedMaxLayerCount);
}

DisplayConfig::~DisplayConfig() = default;

void DisplayConfig::InitializeInspect(inspect::Node* parent) {
  static std::atomic_uint64_t inspect_count;
  node_ = parent->CreateChild(fbl::StringPrintf("display-config-%ld", inspect_count++).c_str());
  draft_has_layer_list_change_property_ =
      node_.CreateBool("draft_has_layer_list_change", draft_has_layer_list_change_);
  pending_commit_layer_change_property_ =
      node_.CreateBool("pending_commit_layer_change", pending_commit_layer_change_);
}

void DisplayConfig::DiscardNonLayerDraftConfig() {
  draft_has_layer_list_change_ = false;
  draft_has_layer_list_change_property_.Set(false);

  draft_ = committed_;
  has_draft_nonlayer_config_change_ = false;
}

}  // namespace display_coordinator
