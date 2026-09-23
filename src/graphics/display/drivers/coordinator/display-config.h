// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_DISPLAY_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_DISPLAY_CONFIG_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <memory>

#include <fbl/intrusive_double_list.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/coordinator/driver-display-config.h"
#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/drivers/coordinator/layer.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display_coordinator {

class Client;

// Almost-POD used by Client to manage display configuration. Public state is used by Controller.
class DisplayConfig : public IdMappable<std::unique_ptr<DisplayConfig>, display::DisplayId> {
 public:
  // `engine_max_layer_count` must be positive, and must not exceed
  // `display::EngineInfo::kMaxAllowedMaxLayerCount`.
  explicit DisplayConfig(display::DisplayId display_id,
                         fbl::Vector<display::PixelFormat> pixel_formats,
                         int engine_max_layer_count);

  DisplayConfig(const DisplayConfig&) = delete;
  DisplayConfig& operator=(const DisplayConfig&) = delete;
  DisplayConfig(DisplayConfig&&) = delete;
  DisplayConfig& operator=(DisplayConfig&&) = delete;

  ~DisplayConfig();

  void InitializeInspect(inspect::Node* parent);

  bool commit_layer_change() {
    bool ret = pending_commit_layer_change_;
    pending_commit_layer_change_ = false;
    pending_commit_layer_change_property_.Set(false);
    return ret;
  }

  // Discards all the draft changes (except for draft layers lists)
  // of a Display's `config`.
  //
  // The display draft layers' draft configs must be discarded before
  // `DiscardNonLayerDraftConfig()` is called.
  void DiscardNonLayerDraftConfig();

  // Maximum number of layers that can be composited on this display.
  //
  // This is a hard upper bound placed by the display engine hardware. The
  // maximum may not be reachable due to other factors, such as memory bandwidth
  // limitations.
  //
  // Guaranteed to be positive and at most
  // `display::EngineInfo::kMaxAllowedMaxLayerCount`.
  int engine_max_layer_count() const { return engine_max_layer_count_; }

  const DriverDisplayConfig& committed_config() const { return committed_; }
  const fbl::DoublyLinkedList<LayerNode*>& get_committed_layers() const {
    return committed_layers_;
  }

 private:
  // The last configuration sent to the display engine.
  DriverDisplayConfig committed_;

  // The display configuration modified by client calls.
  DriverDisplayConfig draft_;

  // If true, the draft configuration's layer list may differ from the current
  // configuration's list.
  bool draft_has_layer_list_change_ = false;

  bool pending_commit_layer_change_ = false;
  fbl::DoublyLinkedList<LayerNode*> draft_layers_;
  fbl::DoublyLinkedList<LayerNode*> committed_layers_;

  const fbl::Vector<display::PixelFormat> pixel_formats_;
  const int engine_max_layer_count_;

  bool has_draft_nonlayer_config_change_ = false;

  friend Client;

  inspect::Node node_;
  // Reflects `draft_has_layer_list_change_`.
  inspect::BoolProperty draft_has_layer_list_change_property_;
  // Reflects `pending_commit_layer_change_`.
  inspect::BoolProperty pending_commit_layer_change_property_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_DISPLAY_CONFIG_H_
