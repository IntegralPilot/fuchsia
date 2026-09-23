// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_VIRTUAL_LAYER_H_
#define SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_VIRTUAL_LAYER_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem/cpp/wire.h>
#include <lib/zx/channel.h>
#include <zircon/types.h>

#include "src/graphics/display/bin/display-test/display.h"
#include "src/graphics/display/bin/display-test/image.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"

namespace display_test {

static constexpr display::LayerId kInitialLayerId(1);

struct custom_layer_t {
  display::LayerId id;
  bool active;

  bool done;

  fuchsia_math::wire::RectU src;
  fuchsia_math::wire::RectU dest;

  image_import_t import_info[2];
};

// A layer whose output can appear on multiple displays.
class VirtualLayer {
 public:
  using Coordinator = fuchsia_hardware_display::Coordinator;

  explicit VirtualLayer(Display* display);
  explicit VirtualLayer(const fbl::Vector<Display>& displays, bool tiled = true);

  virtual ~VirtualLayer() {}

  // Finish initializing the layer. All Set* methods should be called before this.
  virtual bool Init(const fidl::WireSyncClient<Coordinator>& dc) = 0;

  // Steps the local layout state to frame_num.
  virtual void StepLayout(int32_t frame_num) = 0;

  // Returns true iff the latest image on the layer should be rendered on the display, given that
  // the latest vsync stamp received from the display is `latest_vsync_stamp`.
  virtual bool ReadyToRender(display::ConfigStamp latest_vsync_stamp) = 0;

  // Sets the current layout to the display coordinator.
  virtual void SendLayout(const fidl::WireSyncClient<Coordinator>& dc) = 0;

  // Renders the current frame (and signals the fence if necessary).
  virtual void Render(int32_t frame_num) = 0;

  virtual void* GetCurrentImageBuf() = 0;
  virtual size_t GetCurrentImageSize() = 0;

  // Gets the display coordinator layer ID for usage on the given display.
  display::LayerId id(display::DisplayId display_id) const {
    for (unsigned i = 0; i < displays_.size(); i++) {
      if (displays_[i]->id() == display_id && layers_[i].active) {
        return layers_[i].id;
      }
    }
    return display::kInvalidLayerId;
  }

  // Gets the ID of the image on the given display.
  virtual display::ImageId image_id(display::DisplayId display_id) const = 0;

  void set_frame_done(display::DisplayId display_id) {
    for (unsigned i = 0; i < displays_.size(); i++) {
      if (displays_[i]->id() == display_id) {
        layers_[i].done = true;
      }
    }
  }

  virtual bool is_done() const {
    bool done = true;
    for (unsigned i = 0; i < displays_.size(); i++) {
      done &= !layers_[i].active || layers_[i].done;
    }
    return done;
  }

  void clear_done() {
    for (unsigned i = 0; i < displays_.size(); i++) {
      layers_[i].done = false;
    }
  }

 protected:
  custom_layer_t* CreateLayer(const fidl::WireSyncClient<Coordinator>& dc);
  void SetLayerImages(const fidl::WireSyncClient<Coordinator>& dc, bool alt_image);

  fbl::Vector<const Display*> displays_;
  fbl::Vector<custom_layer_t> layers_;

  uint32_t width_ = 0;
  uint32_t height_ = 0;

  display::LayerId next_layer_id_{kInitialLayerId};
};

class PrimaryLayer : public VirtualLayer {
 public:
  explicit PrimaryLayer(Display* display);
  explicit PrimaryLayer(const fbl::Vector<Display>& displays, bool mirrors = false);
  explicit PrimaryLayer(const fbl::Vector<Display>& displays, Image::Pattern pattern,
                        uint32_t fgcolor, uint32_t bgcolor, bool mirrors = false);

  // Set* methods to configure the layer.
  void SetImageDimens(uint32_t width, uint32_t height) {
    image_width_ = width;
    image_height_ = height;

    image_source_.width = width;
    image_source_.height = height;
    display_destination_.width = width;
    display_destination_.height = height;
  }
  void SetImageSource(uint32_t width, uint32_t height) {
    image_source_.width = width;
    image_source_.height = height;
  }
  void SetDisplayDestination(uint32_t width, uint32_t height) {
    display_destination_.width = width;
    display_destination_.height = height;
  }
  void SetLayerFlipping(bool flip) { layer_flipping_ = flip; }
  void SetPanSrc(bool pan) { pan_src_ = pan; }
  void SetPanDest(bool pan) { pan_dest_ = pan; }
  void SetLayerToggle(bool toggle) { layer_toggle_ = toggle; }
  void SetRotates(bool rotates) { rotates_ = rotates; }
  void SetAlpha(bool enable, float val) {
    alpha_enable_ = enable;
    alpha_val_ = val;
  }
  void SetScaling(bool enable) { scaling_ = enable; }
  void SetImageFormat(fuchsia_images2::wire::PixelFormat image_format) {
    image_format_ = image_format;
  }
  void SetFormatModifier(fuchsia_images2::PixelFormatModifier modifier) { modifier_ = modifier; }

  bool Init(const fidl::WireSyncClient<Coordinator>& dc) override;
  void StepLayout(int32_t frame_num) override;
  bool ReadyToRender(display::ConfigStamp latest_vsync_stamp) override;
  void SendLayout(const fidl::WireSyncClient<Coordinator>& channel) override;
  void Render(int32_t frame_num) override;

  void* GetCurrentImageBuf() override;
  size_t GetCurrentImageSize() override;

  display::ImageId image_id(display::DisplayId display_id) const override {
    for (unsigned i = 0; i < displays_.size(); i++) {
      if (displays_[i]->id() == display_id && layers_[i].active) {
        return layers_[i].import_info[alt_image_].id;
      }
    }
    return display::kInvalidImageId;
  }

 private:
  void SetLayerPositions(const fidl::WireSyncClient<Coordinator>& dc);
  void InitImageDimens();

  uint32_t image_width_ = 0;
  uint32_t image_height_ = 0;
  fuchsia_images2::wire::PixelFormat image_format_ = fuchsia_images2::wire::PixelFormat::kInvalid;
  bool override_colors_ = false;

  Image::Pattern image_pattern_ = Image::Pattern::kCheckerboard;
  uint32_t fgcolor_;
  uint32_t bgcolor_;

  fuchsia_math::wire::RectU image_source_ = {};
  fuchsia_math::wire::RectU display_destination_ = {};
  fuchsia_hardware_display_types::wire::CoordinateTransformation rotation_ =
      fuchsia_hardware_display_types::wire::CoordinateTransformation::kIdentity;
  bool layer_flipping_ = false;
  bool pan_src_ = false;
  bool pan_dest_ = false;
  bool layer_toggle_ = false;
  bool rotates_ = false;
  bool alpha_enable_ = false;
  float alpha_val_ = 0.f;
  bool scaling_ = false;
  fuchsia_images2::wire::PixelFormatModifier modifier_ =
      fuchsia_images2::wire::PixelFormatModifier::kLinear;
  bool mirrors_ = false;

  bool alt_image_ = false;
  Image* images_[2];
  fuchsia_hardware_display::wire::ConfigStamp image_config_stamps_[2] = {
      {.value = fuchsia_hardware_display::wire::kInvalidConfigStampValue},
      {.value = fuchsia_hardware_display::wire::kInvalidConfigStampValue}};
};

class ColorLayer : public VirtualLayer {
 public:
  explicit ColorLayer(Display* display);
  explicit ColorLayer(const fbl::Vector<Display>& displays);

  bool Init(const fidl::WireSyncClient<Coordinator>& dc) override;

  void SendLayout(const fidl::WireSyncClient<Coordinator>& dc) override {}
  void StepLayout(int32_t frame_num) override {}
  bool ReadyToRender(display::ConfigStamp latest_vsync_stamp) override { return true; }
  void Render(int32_t frame_num) override {}
  void* GetCurrentImageBuf() override { return nullptr; }
  size_t GetCurrentImageSize() override { return 0; }
  display::ImageId image_id(display::DisplayId display_id) const override {
    return display::kInvalidImageId;
  }
  virtual bool is_done() const override { return true; }
};

}  // namespace display_test

#endif  // SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_VIRTUAL_LAYER_H_
