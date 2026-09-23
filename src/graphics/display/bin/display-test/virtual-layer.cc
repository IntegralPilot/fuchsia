// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/bin/display-test/virtual-layer.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/image-format/image_format.h>
#include <lib/sysmem-version/sysmem-version.h>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <iterator>

#include <fbl/algorithm.h>

#include "fidl/fuchsia.hardware.display.types/cpp/wire_types.h"
#include "src/graphics/display/bin/display-test/utils.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"

namespace fhd = fuchsia_hardware_display;
namespace fhdt = fuchsia_hardware_display_types;

namespace display_test {

static constexpr uint32_t kSrcFrameBouncePeriod = 90;
static constexpr uint32_t kDestFrameBouncePeriod = 60;
static constexpr uint32_t kRotationPeriod = 24;
static constexpr uint32_t kScalePeriod = 45;

namespace {

constexpr display::ImageId GetPrimaryImageImportId(unsigned display_index, int image_index) {
  ZX_ASSERT(image_index >= 0);
  ZX_ASSERT(image_index <= 1);
  return display::ImageId(1 + display_index * 3 + image_index);
}

}  // namespace

static uint32_t get_fg_color() {
  static uint32_t layer_count = 0;
  static uint32_t colors[] = {
      0xffff0000,
      0xff00ff00,
      0xff0000ff,
  };
  return colors[layer_count++ % std::size(colors)];
}

// Checks if two rectangles intersect, and if so, returns their intersection.
static bool compute_intersection(const fuchsia_math::wire::RectU& a,
                                 const fuchsia_math::wire::RectU& b,
                                 fuchsia_math::wire::RectU* intersection) {
  uint32_t left = std::max(a.x, b.x);
  uint32_t right = std::min(a.x + a.width, b.x + b.width);
  uint32_t top = std::max(a.y, b.y);
  uint32_t bottom = std::min(a.y + a.height, b.y + b.height);

  if (left >= right || top >= bottom) {
    return false;
  }

  intersection->x = left;
  intersection->y = top;
  intersection->width = right - left;
  intersection->height = bottom - top;

  return true;
}

static uint32_t interpolate_scaling(uint32_t x, uint32_t frame_num) {
  return x / 2 + interpolate(x / 2, frame_num, kScalePeriod);
}

VirtualLayer::VirtualLayer(Display* display) {
  displays_.push_back(display);
  width_ = display->mode().active_area.width;
  height_ = display->mode().active_area.height;
}

VirtualLayer::VirtualLayer(const fbl::Vector<Display>& displays, bool tiled) {
  for (const Display& d : displays) {
    displays_.push_back(&d);
  }

  width_ = 0;
  height_ = 0;
  for (auto* d : displays_) {
    if (tiled) {
      width_ += d->mode().active_area.width;
    } else {
      width_ = std::max(width_, d->mode().active_area.width);
    }
    height_ = std::max(height_, d->mode().active_area.height);
  }
}

custom_layer_t* VirtualLayer::CreateLayer(const fidl::WireSyncClient<fhd::Coordinator>& dc) {
  layers_.push_back(custom_layer_t());
  layers_[layers_.size() - 1].active = false;
  layers_[layers_.size() - 1].id = next_layer_id_++;

  fidl::WireResult<fuchsia_hardware_display::Coordinator::CreateLayer> result =
      dc->CreateLayer(layers_[layers_.size() - 1].id.ToFidl());
  if (!result.ok() || result.value().is_error()) {
    printf("Creating layer failed: %s\n", result.status_string());
    return nullptr;
  }

  return &layers_[layers_.size() - 1];
}

PrimaryLayer::PrimaryLayer(Display* display) : VirtualLayer(display) {
  image_format_ = display->format();
}

PrimaryLayer::PrimaryLayer(const fbl::Vector<Display>& displays, bool mirrors)
    : VirtualLayer(displays, !mirrors), mirrors_(mirrors) {
  image_format_ = displays_[0]->format();
  SetImageDimens(width_, height_);
}

PrimaryLayer::PrimaryLayer(const fbl::Vector<Display>& displays, Image::Pattern pattern,
                           uint32_t fgcolor, uint32_t bgcolor, bool mirrors)
    : VirtualLayer(displays, !mirrors),
      image_pattern_(pattern),
      fgcolor_(fgcolor),
      bgcolor_(bgcolor),
      mirrors_(mirrors) {
  override_colors_ = true;
  image_format_ = displays_[0]->format();
  SetImageDimens(width_, height_);
}

bool PrimaryLayer::Init(const fidl::WireSyncClient<fhd::Coordinator>& dc) {
  if ((displays_.size() > 1 || rotates_) && scaling_) {
    printf("Unsupported config\n");
    return false;
  }
  uint32_t fg_color = override_colors_ ? fgcolor_ : get_fg_color();
  uint32_t bg_color = alpha_enable_ ? 0x3fffffff : 0xffffffff;
  if (override_colors_) {
    bg_color = bgcolor_;
  }

  images_[0] = Image::Create(dc, image_width_, image_height_, image_format_, image_pattern_,
                             fg_color, bg_color, modifier_);
  if (layer_flipping_) {
    images_[1] = Image::Create(dc, image_width_, image_height_, image_format_, image_pattern_,
                               fg_color, bg_color, modifier_);
  }
  if (!images_[0] || (layer_flipping_ && !images_[1])) {
    return false;
  }

  if (!layer_flipping_) {
    images_[0]->Render(-1, -1);
  }

  for (unsigned i = 0; i < displays_.size(); i++) {
    custom_layer_t* layer = CreateLayer(dc);
    if (layer == nullptr) {
      return false;
    }

    if (!images_[0]->Import(dc, GetPrimaryImageImportId(i, 0), &layer->import_info[0])) {
      return false;
    }
    if (layer_flipping_) {
      if (!images_[1]->Import(dc, GetPrimaryImageImportId(i, 1), &layer->import_info[1])) {
        return false;
      }
    } else {
      layer->import_info[alt_image_].events[WAIT_EVENT].signal(0, ZX_EVENT_SIGNALED);
    }

    fhdt::wire::ImageMetadata image_metadata = images_[0]->GetMetadata();
    const fhd::wire::LayerId fidl_layer_id = layer->id.ToFidl();
    auto set_config_result = dc->SetLayerPrimaryConfig(fidl_layer_id, image_metadata);
    if (!set_config_result.ok()) {
      printf("Setting layer config failed\n");
      return false;
    }

    auto set_alpha_result = dc->SetLayerPrimaryAlpha(
        fidl_layer_id,
        alpha_enable_ ? fhdt::wire::AlphaMode::kHwMultiply : fhdt::wire::AlphaMode::kDisable,
        alpha_val_);
    if (!set_alpha_result.ok()) {
      printf("Setting layer alpha config failed\n");
      return false;
    }
  }

  StepLayout(0);
  if (!layer_flipping_) {
    SetLayerImages(dc, false);
  }
  if (!(pan_src_ || pan_dest_)) {
    SetLayerPositions(dc);
  }

  return true;
}

void* PrimaryLayer::GetCurrentImageBuf() { return images_[alt_image_]->buffer(); }
size_t PrimaryLayer::GetCurrentImageSize() {
  PixelFormatAndModifier format_and_modifier(images_[alt_image_]->format(),
                                             images_[alt_image_]->modifier());
  return images_[alt_image_]->height() * images_[alt_image_]->stride() *
         ImageFormatStrideBytesPerWidthPixel(format_and_modifier);
}
void PrimaryLayer::StepLayout(int32_t frame_num) {
  if (layer_flipping_) {
    alt_image_ = frame_num % 2;
  }
  if (pan_src_) {
    image_source_.x =
        interpolate(image_width_ - image_source_.width, frame_num, kSrcFrameBouncePeriod);
  }
  if (pan_dest_) {
    display_destination_.x =
        interpolate(width_ - display_destination_.width, frame_num, kDestFrameBouncePeriod);
  }
  if (rotates_) {
    switch ((frame_num / kRotationPeriod) % 4) {
      case 0:
        rotation_ = fuchsia_hardware_display_types::wire::CoordinateTransformation::kIdentity;
        break;
      case 1:
        rotation_ = fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90;
        break;
      case 2:
        rotation_ = fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw180;
        break;
      case 3:
        rotation_ = fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw270;
        break;
    }

    if (frame_num % kRotationPeriod == 0 && frame_num != 0) {
      uint32_t tmp = display_destination_.width;
      display_destination_.width = display_destination_.height;
      display_destination_.height = tmp;
    }
  }

  fuchsia_math::wire::RectU display = {};
  for (unsigned i = 0; i < displays_.size(); i++) {
    display.width = displays_[i]->mode().active_area.width;
    display.height = displays_[i]->mode().active_area.height;

    if (mirrors_) {
      layers_[i].src.x = 0;
      layers_[i].src.y = 0;
      layers_[i].src.width = image_width_;
      layers_[i].src.height = image_height_;
      layers_[i].dest.x = 0;
      layers_[i].dest.y = 0;
      layers_[i].dest.width = display.width;
      layers_[i].dest.height = display.height;
      layers_[i].active = true;
      continue;
    }

    // Calculate the portion of the dest frame which shows up on this display
    if (compute_intersection(display, display_destination_, &layers_[i].dest)) {
      // Find the subset of the src region which shows up on this display
      if (rotation_ == fuchsia_hardware_display_types::wire::CoordinateTransformation::kIdentity ||
          rotation_ ==
              fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw180) {
        if (!scaling_) {
          layers_[i].src.x = image_source_.x + (layers_[i].dest.x - display_destination_.x);
          layers_[i].src.y = image_source_.y;
          layers_[i].src.width = layers_[i].dest.width;
          layers_[i].src.height = layers_[i].dest.height;
        } else {
          layers_[i].src.x =
              image_source_.x +
              interpolate_scaling(layers_[i].dest.x - display_destination_.x, frame_num);
          layers_[i].src.x = image_source_.x;
          layers_[i].src.width = interpolate_scaling(layers_[i].dest.width, frame_num);
          layers_[i].src.height = interpolate_scaling(layers_[i].dest.height, frame_num);
        }
      } else {
        layers_[i].src.x = image_source_.x;
        layers_[i].src.y = image_source_.y + (layers_[i].dest.y - display_destination_.y);
        layers_[i].src.height = layers_[i].dest.width;
        layers_[i].src.width = layers_[i].dest.height;
      }

      // Put the dest frame coordinates in the display's coord space
      layers_[i].dest.x -= display.x;
      layers_[i].active = true;
    } else {
      layers_[i].active = false;
    }

    display.x += display.width;
  }

  if (layer_toggle_) {
    for (auto& layer : layers_) {
      layer.active = !(frame_num % 2);
    }
  }
}

void PrimaryLayer::SendLayout(const fidl::WireSyncClient<fhd::Coordinator>& dc) {
  if (layer_flipping_) {
    SetLayerImages(dc, alt_image_);
  }
  if (scaling_ || pan_src_ || pan_dest_) {
    SetLayerPositions(dc);
  }
}

bool PrimaryLayer::ReadyToRender(display::ConfigStamp latest_vsync_stamp) {
  if (!layer_flipping_) {
    // We don't render when `layer_flipping_` is false, so it's OK to answer true.
    return true;
  }
  // If this is the first time the image has been used, it's ready to render into.  Otherwise,
  // it's only ready to render into if it has already been replaced by a subsequent config showing
  // the other image.
  return image_config_stamps_[alt_image_].value == 0 ||
         latest_vsync_stamp.value() > image_config_stamps_[alt_image_].value;
}

void PrimaryLayer::Render(int32_t frame_num) {
  if (!layer_flipping_) {
    return;
  }
  images_[alt_image_]->Render(frame_num < 2 ? 0 : frame_num - 2, frame_num);
  for (auto& layer : layers_) {
    layer.import_info[alt_image_].events[WAIT_EVENT].signal(0, ZX_EVENT_SIGNALED);
  }
}

void PrimaryLayer::SetLayerPositions(const fidl::WireSyncClient<fhd::Coordinator>& dc) {
  for (auto& layer : layers_) {
    const fhd::wire::LayerId fidl_layer_id = layer.id.ToFidl();
    ZX_ASSERT(dc->SetLayerPrimaryPosition(fidl_layer_id, rotation_, layer.src, layer.dest).ok());
  }
}

void VirtualLayer::SetLayerImages(const fidl::WireSyncClient<fhd::Coordinator>& dc,
                                  bool alt_image) {
  for (auto& layer : layers_) {
    const auto& image = layer.import_info[alt_image];
    const fhd::wire::LayerId fidl_layer_id = layer.id.ToFidl();
    const fhd::wire::ImageId fidl_image_id = image.id.ToFidl();
    const fhd::wire::EventId fidl_wait_event_id = image.event_ids[WAIT_EVENT].ToFidl();
    auto result = dc->SetLayerImage2(fidl_layer_id, fidl_image_id, fidl_wait_event_id);

    ZX_ASSERT(result.ok());
  }
}

ColorLayer::ColorLayer(Display* display) : VirtualLayer(display) {}

ColorLayer::ColorLayer(const fbl::Vector<Display>& displays) : VirtualLayer(displays) {}

bool ColorLayer::Init(const fidl::WireSyncClient<fhd::Coordinator>& dc) {
  for (unsigned i = 0; i < displays_.size(); i++) {
    custom_layer_t* layer = CreateLayer(dc);
    if (layer == nullptr) {
      return false;
    }

    layer->active = true;

    constexpr fuchsia_images2::wire::PixelFormat kColorLayerFormat =
        fuchsia_images2::wire::PixelFormat::kB8G8R8A8;
    uint32_t color = get_fg_color();

    fidl::Array<uint8_t, 8> bytes;
    std::memcpy(bytes.data(), &color, sizeof(color));
    const fhd::wire::LayerId fidl_layer_id = layer->id.ToFidl();
    const fuchsia_math::wire::RectU display_destination = {
        .x = 0,
        .y = 0,
        .width = displays_[i]->mode().active_area.width,
        .height = displays_[i]->mode().active_area.height,
    };
    auto result = dc->SetLayerColorConfig(fidl_layer_id,
                                          fuchsia_hardware_display_types::wire::Color{
                                              .format = kColorLayerFormat,
                                              .bytes = bytes,
                                          },
                                          display_destination);

    if (!result.ok()) {
      printf("Setting layer config failed\n");
      return false;
    }
  }

  return true;
}

}  // namespace display_test
