// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_LAYER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_LAYER_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>

#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>
#include <fbl/slab_allocator.h>

#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/drivers/coordinator/waiting-image-list.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"

namespace display_coordinator {

class FenceCollection;
class Layer;
class LayerTest;
class Client;
class Controller;

struct LayerNode : public fbl::DoublyLinkedListable<LayerNode*> {
  Layer* layer;
};

// Manages Client-created Layer configurations.
//
// Instances are not thread-safe. Concurrent access must be synchronized
// externally.
class Layer : public IdMappable<std::unique_ptr<Layer>, display::LayerId> {
 public:
  // `controller` must be non-null.
  explicit Layer(display::LayerId id);

  Layer(const Layer&) = delete;
  Layer(Layer&&) = delete;
  Layer& operator=(const Layer&) = delete;
  Layer& operator=(Layer&&) = delete;

  ~Layer();

  fbl::RefPtr<Image> committed_image() const { return committed_image_; }

  bool is_skipped() const { return is_skipped_; }

  // TODO(https://fxbug.dev/42118906) Although this is nominally a POD, the state management and
  // lifecycle are complicated by interactions with Client's threading model.
  friend Client;
  friend LayerTest;

  bool in_use() const {
    return committed_display_config_list_node_.InContainer() ||
           draft_display_config_list_node_.InContainer();
  }

  const display::ImageMetadata& draft_image_metadata() const {
    return draft_layer_config_.image_metadata();
  }

  // If the layer properties were changed in the draft configuration, this
  // retires all images as they are invalidated with layer properties change.
  bool ResolveDraftLayerProperties();

  // This sets up the fence and config stamp for pending images on this layer.
  //
  // - If the layer image has a fence to wait before presentation, this prepares
  //   the new fence and start async waiting for the fence.
  // - The layer's latest pending (waiting) image will be associated with the
  //   client configuration |stamp|, as it reflects the latest configuration
  //   state; this will overwrite all the previous stamp states for this image.
  //   The stamp will be used later when display core integrates stamps of all
  //   layers to determine the current frame state.
  //
  // Returns false if there were any errors.
  bool ResolveDraftImage(FenceCollection* fence,
                         display::ConfigStamp stamp = display::kInvalidConfigStamp);

  // Set the committed layer configuration to the draft layer configuration.
  void CommitChanges();

  // Set the draft layer configuration to the committed layer configuration.
  //
  // This discards any changes in the draft layer configuration.
  void DiscardChanges();

  // Removes references to all Images associated with this Layer.
  // Returns true if the committed config has been affected.
  bool CleanUpAllImages();

  // Removes references to the provided Image. `image` must be valid.
  // Returns true if the committed config has been affected.
  bool CleanUpImage(const Image& image);

  // If a new image is available, retire applied_image() and other pending images. Returns false if
  // no images were ready.
  bool ActivateLatestReadyImage();

  // Get the stamp of configuration that is associated (at ResolveDraftImage)
  // with the image that is currently being displayed on the device.
  // If no image is being displayed on this layer, returns nullopt.
  std::optional<display::ConfigStamp> GetCurrentClientConfigStamp() const;

  // Adds the layer's draft config to a display configuration's layer list.
  //
  // Returns true if the method succeeds. The layer's draft config is be at
  // the end of the display configuration's list of layer configs.
  //
  // Returns false if the layer's draft config is already in a display
  // configuration's layer list. No changes are made.
  bool AppendToConfigLayerList(fbl::DoublyLinkedList<LayerNode*>& config_layer_list);

  void SetPrimaryConfig(display::ImageMetadata image_metadata);
  void SetPrimaryPosition(display::CoordinateTransformation image_source_transformation,
                          display::Rectangle image_source, display::Rectangle display_destination);
  void SetPrimaryAlpha(display::AlphaMode alpha_mode, float alpha_coefficient);
  void SetColorConfig(display::Color color, display::Rectangle display_destination);
  void SetImage(fbl::RefPtr<Image> image_id, display::EventId wait_event_id);

  // Called on all waiting images when any fence is signaled.
  //
  // Returns true if an image is ready for use by the display engine.
  bool MarkFenceReady(Fence& fence);

  // Returns true if the layer has any waiting images. An image transitions from "draft" to
  // "waiting" (in the context of a specific layer) when that layer appears in a committed config.
  bool HasWaitingImages() const;

  const display::DriverLayer& committed_driver_layer_config() const {
    return committed_layer_config_;
  }

 private:
  // Retires the `draft_image_`.
  void RetireDraftImage();

  // Retires the `image` from the `waiting_images_` list.
  // Does nothing if `image` is not in the list.
  void RetireWaitingImage(const Image& image);

  // Retires the image most recently sent to the display engine driver.
  //
  // Returns true if this changes the applied display configuration.
  bool RetireAppliedImage();

  display::DriverLayer draft_layer_config_;
  display::DriverLayer committed_layer_config_;

  // True if `draft_layer_` is different from `committed_layer_`.
  bool draft_layer_config_differs_from_committed_;

  // The event passed to SetLayerImage which hasn't been applied yet.
  display::EventId draft_image_wait_event_id_ = display::kInvalidEventId;

  // The image given to SetLayerImage which hasn't been applied yet.
  fbl::RefPtr<Image> draft_image_;

  // Manages images which are waiting to be displayed. Each one has an optional wait fence which
  // may not have been signaled yet.
  //
  // Must be accessed on `controller_`'s driver dispatcher loop. Call-sites must guarantee this via
  // `ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher())`.
  WaitingImageList waiting_images_;

  fbl::RefPtr<Image> committed_image_;

  // Counters used for keeping track of when the layer's images need to be dropped.
  uint64_t draft_image_config_gen_ = 0;
  uint64_t applied_image_config_gen_ = 0;

  // If valid, belongs to the `DisplayConfig::draft_layers_` list.
  LayerNode draft_display_config_list_node_;

  // If valid, belongs to the `DisplayConfig::committed_layers_` list.
  LayerNode committed_display_config_list_node_;

  // Identifies the display that this layer was last applied to.
  //
  // Invalid if this layer never belonged to an applied display configuration.
  display::DisplayId applied_to_display_id_ = display::kInvalidDisplayId;

  bool is_skipped_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_LAYER_H_
