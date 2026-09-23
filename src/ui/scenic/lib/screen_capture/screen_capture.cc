// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screen_capture/screen_capture.h"

#include <fidl/fuchsia.ui.composition/cpp/type_conversions.h>
#include <lib/async/default.h>
#include <lib/fit/result.h>
#include <lib/fpromise/sequencer.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <utility>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"

using flatland::SrcToDest;
using fuchsia_ui_composition::wire::FrameInfo;
using fuchsia_ui_composition::wire::Orientation;
using fuchsia_ui_composition::wire::Rotation;
using fuchsia_ui_composition::wire::ScreenCaptureConfig;
using fuchsia_ui_composition::wire::ScreenCaptureError;
using std::vector;

namespace screen_capture {

ScreenCapture::ScreenCapture(const vector<std::shared_ptr<allocation::BufferCollectionImporter>>&
                                 buffer_collection_importers,
                             std::shared_ptr<flatland::Renderer> renderer,
                             GetRenderables get_renderables)
    : buffer_collection_importers_(buffer_collection_importers),
      renderer_(std::move(renderer)),
      get_renderables_(std::move(get_renderables)),
      executor_(async_get_default_dispatcher()) {}

ScreenCapture::~ScreenCapture() { ClearImages(); }

void ScreenCapture::Configure(ConfigureRequestView request, ConfigureCompleter::Sync& completer) {
  Configure(std::move(*request), [completer = completer.ToAsync()](auto result) mutable {
    if (result.is_error()) {
      completer.ReplyError(result.error_value());
    } else {
      completer.ReplySuccess();
    }
  });
}

void ScreenCapture::Configure(
    fuchsia_ui_composition::wire::ScreenCaptureConfig args,
    fit::function<void(fit::result<fuchsia_ui_composition::wire::ScreenCaptureError>)> callback) {
  // Check for missing args.
  if (!args.has_import_token() || !args.has_size() || !args.size().width || !args.size().height ||
      !args.has_buffer_count()) {
    FX_LOGS(WARNING) << "ScreenCapture::Configure: Missing arguments.";
    callback(fit::error(ScreenCaptureError::kMissingArgs));
    return;
  }

  // Check for invalid args.
  if (args.buffer_count() < 1) {
    FX_LOGS(WARNING) << "ScreenCapture::Configure: There must be at least one buffer.";
    callback(fit::error(ScreenCaptureError::kInvalidArgs));
    return;
  }

  fuchsia_ui_composition::wire::BufferCollectionImportToken import_token =
      std::move(args.import_token());
  const zx_koid_t global_collection_id = fsl::GetRelatedKoid(import_token.value.get());

  // Event pair ID must be valid.
  if (global_collection_id == ZX_KOID_INVALID) {
    FX_LOGS(WARNING) << "ScreenCapture::Configure: Event pair ID must be valid.";
    callback(fit::error(ScreenCaptureError::kInvalidArgs));
    return;
  }

  // Release any existing buffers and reset image_ids_ and available_buffers_
  ClearImages(ConfigureState::kConfiguring);

  // Create the associated metadata. Note that clients are responsible for ensuring reasonable
  // parameters.
  allocation::ImageMetadata metadata;
  metadata.collection_id = global_collection_id;
  metadata.width = args.size().width;
  metadata.height = args.size().height;

  stream_rotation_ =
      args.has_rotation() ? args.rotation() : fuchsia_ui_composition::wire::Rotation::kCw0Degrees;

  fpromise::sequencer seq;
  std::vector<fpromise::promise<>> promises;
  promises.reserve(args.buffer_count());
  // For each buffer in the collection, add the image to our importers.
  for (uint32_t i = 0; i < args.buffer_count(); i++) {
    metadata.identifier = allocation::GenerateUniqueImageId();
    metadata.vmo_index = i;
    std::vector<fpromise::promise<>> inner_promises;
    inner_promises.reserve(buffer_collection_importers_.size());
    for (auto& importer : buffer_collection_importers_) {
      auto promise =
          importer->ImportBufferImage(metadata, allocation::BufferCollectionUsage::kRenderTarget);
      inner_promises.push_back(std::move(promise));
    }
    auto join_promise =
        fpromise::join_promise_vector(std::move(inner_promises))
            .and_then([this, i,
                       metadata](std::vector<fpromise::result<>>& results) -> fpromise::result<> {
              for (auto& result : results) {
                if (!result.is_ok()) {
                  // If this importer fails, we need to release the image from all of the importers
                  // that successfully imported it and release all of the past buffer images as
                  // well. Luckily we can do this right here instead of waiting for a fence since we
                  // know these images are not being used by anything yet.
                  for (uint32_t j = 0; j < results.size(); j++) {
                    if (results[j].is_ok()) {
                      buffer_collection_importers_[j]->ReleaseBufferImage(metadata.identifier);
                    }
                  }
                  return fpromise::error();
                }
              }
              image_ids_[i] = metadata;
              available_buffers_.push_back(i);
              return fpromise::ok();
            });
    // We use a sequencer to ensure that each buffer is processed sequentially. This is required so
    // that if there is a failure importing an image, we don't end up in an inconsistent state.
    promises.push_back(join_promise.wrap_with(seq));
  }
  auto join_promise =
      fpromise::join_promise_vector(std::move(promises))
          .and_then([this, callback = std::move(callback),
                     keepalive_import_token =
                         std::move(import_token)](std::vector<fpromise::result<>>& results) {
            bool ok = std::ranges::all_of(results, [](auto& result) { return result.is_ok(); });
            if (!ok) {
              ClearImages();
              FX_LOGS(WARNING) << "ScreenCapture::Configure: Failed to import BufferImage.";
              callback(fit::error(ScreenCaptureError::kBadOperation));
              return;
            }
            configure_state_ = ConfigureState::kConfigured;
            callback(fit::ok());
          });
  executor_.schedule_task(std::move(join_promise));
}

void ScreenCapture::Configure(
    fuchsia_ui_composition::ScreenCaptureConfig args,
    fit::function<void(fit::result<fuchsia_ui_composition::ScreenCaptureError>)> callback) {
  fidl::Arena arena;
  Configure(fidl::ToWire(arena, std::move(args)), [callback = std::move(callback)](auto result) {
    if (result.is_error()) {
      callback(fit::error(
          static_cast<fuchsia_ui_composition::ScreenCaptureError>(result.error_value())));
    } else {
      callback(fit::ok());
    }
  });
}

void ScreenCapture::GetNextFrame(GetNextFrameRequestView request,
                                 GetNextFrameCompleter::Sync& completer) {
  GetNextFrame(std::move(*request), [completer = completer.ToAsync()](auto result) mutable {
    if (result.is_error()) {
      completer.ReplyError(result.error_value());
    } else {
      completer.ReplySuccess(result.value());
    }
  });
}

void ScreenCapture::GetNextFrame(
    fuchsia_ui_composition::wire::GetNextFrameArgs args,
    fit::function<void(fit::result<fuchsia_ui_composition::wire::ScreenCaptureError,
                                   fuchsia_ui_composition::wire::FrameInfo>)>
        callback) {
  // Check that we have been configured.
  if (configure_state_ != ConfigureState::kConfigured) {
    FX_LOGS(ERROR) << "ScreenCapture::GetNextFrame: Not configured.";
    callback(fit::error(ScreenCaptureError::kBadOperation));
    return;
  }
  // Check that we have an available buffer that we can render.
  if (available_buffers_.empty()) {
    FX_LOGS(WARNING) << "ScreenCapture::GetNextFrame: No buffers available.";
    callback(fit::error(ScreenCaptureError::kBufferFull));
    return;
  }

  if (!args.has_event()) {
    FX_LOGS(WARNING) << "ScreenCapture::GetNextFrame: Missing arguments.";
    callback(fit::error(ScreenCaptureError::kMissingArgs));
    return;
  }

  // Get renderables from the engine.
  // TODO(https://fxbug.dev/42179243): Ensure this does not happen more than once in the same vsync.
  auto renderables = get_renderables_();

  uint32_t buffer_id = available_buffers_.front();
  const auto& metadata = image_ids_[buffer_id];

  auto image_width = metadata.width;
  auto image_height = metadata.height;

  const auto rotated_layers =
      RotateRenderables(renderables, stream_rotation_, image_width, image_height);

  // Render content into user-provided buffer, which will signal the user-provided event.
  std::span release_fences(&args.event(), 1);

  renderer_->Render(metadata, rotated_layers, {.release_fences = release_fences});

  fidl::Arena arena;
  auto frame_info =
      fuchsia_ui_composition::wire::FrameInfo::Builder(arena).buffer_id(buffer_id).Build();

  available_buffers_.pop_front();
  callback(fit::ok(frame_info));
}

void ScreenCapture::GetNextFrame(
    fuchsia_ui_composition::GetNextFrameArgs args,
    fit::function<void(
        fit::result<fuchsia_ui_composition::ScreenCaptureError, fuchsia_ui_composition::FrameInfo>)>
        callback) {
  fidl::Arena arena;
  GetNextFrame(fidl::ToWire(arena, std::move(args)), [callback = std::move(callback)](auto result) {
    if (result.is_error()) {
      callback(fit::error(
          static_cast<fuchsia_ui_composition::ScreenCaptureError>(result.error_value())));
    } else {
      callback(fit::ok(fidl::ToNatural(result.value())));
    }
  });
}

void ScreenCapture::ReleaseFrame(ReleaseFrameRequestView request,
                                 ReleaseFrameCompleter::Sync& completer) {
  ReleaseFrame(request->buffer_id, [completer = completer.ToAsync()](auto result) mutable {
    if (result.is_error()) {
      completer.ReplyError(result.error_value());
    } else {
      completer.ReplySuccess();
    }
  });
}

void ScreenCapture::ReleaseFrame(
    uint32_t buffer_id,
    fit::function<void(fit::result<fuchsia_ui_composition::ScreenCaptureError>)> callback) {
  // Check that the buffer index is in range.
  if (image_ids_.find(buffer_id) == image_ids_.end()) {
    FX_LOGS(WARNING) << "ScreenCapture::ReleaseFrame: Buffer ID does not exist.";
    callback(fit::error(ScreenCaptureError::kInvalidArgs));
    return;
  }

  // Check that the buffer index is not already available.
  if (std::find(available_buffers_.begin(), available_buffers_.end(), buffer_id) !=
      available_buffers_.end()) {
    FX_LOGS(WARNING) << "ScreenCapture::ReleaseFrame: Buffer ID already available.";
    callback(fit::error(ScreenCaptureError::kInvalidArgs));
    return;
  }

  available_buffers_.push_back(buffer_id);
  callback(fit::ok());
}

void ScreenCapture::ClearImages(ConfigureState state) {
  for (auto& image_id : image_ids_) {
    auto identifier = image_id.second.identifier;
    for (auto& buffer_collection_importer : buffer_collection_importers_) {
      buffer_collection_importer->ReleaseBufferImage(identifier);
    }
  }
  image_ids_.clear();
  available_buffers_.clear();
  configure_state_ = state;
}

std::vector<flatland::ResolvedLayer> ScreenCapture::RotateRenderables(
    const std::vector<flatland::ResolvedLayer>& layers,
    fuchsia_ui_composition::wire::Rotation rotation, uint32_t image_width, uint32_t image_height) {
  if (rotation == fuchsia_ui_composition::wire::Rotation::kCw0Degrees)
    return layers;

  std::vector<flatland::ResolvedLayer> final_layers;
  final_layers.reserve(layers.size());

  for (auto layer : layers) {
    const auto& geometry = layer.geometry;

    // (x,y) is the origin pre-rotation. (0,0) is the top-left of the image.
    auto x = geometry.dest.x();
    auto y = geometry.dest.y();

    // (w, h) is the width and height of the rectangle pre-rotation.
    auto w = geometry.dest.width();
    auto h = geometry.dest.height();

    // Account for translation of the rectangle in the bounds of the canvas.
    float new_x = 0;
    float new_y = 0;
    // Account for the new extent.
    float new_w = 0;
    float new_h = 0;
    // Account for the new orientation.
    types::RotateFlip::Enum ccw_rotation = types::RotateFlip::Enum::kIdentity;

    switch (rotation) {
      case fuchsia_ui_composition::wire::Rotation::kCw90Degrees:
        new_x = static_cast<float>(image_width) - y - h;
        new_y = x;
        new_w = h;
        new_h = w;
        // The renderer requires counter-clockwise rotation instead of clockwise as used by screen
        // capture. 90 clockwise is equivalent to 270 counter-clockwise.
        ccw_rotation = types::RotateFlip::Enum::kRotateCcw270;
        break;
      case fuchsia_ui_composition::wire::Rotation::kCw180Degrees:
        new_x = static_cast<float>(image_width) - x - w;
        new_y = static_cast<float>(image_height) - y - h;
        new_w = w;
        new_h = h;
        ccw_rotation = types::RotateFlip::Enum::kRotateCcw180;
        break;
      case fuchsia_ui_composition::wire::Rotation::kCw270Degrees:
        new_x = y;
        new_y = static_cast<float>(image_height) - x - w;
        new_w = h;
        new_h = w;
        // The renderer requires counter-clockwise rotation instead of clockwise as used by screen
        // capture. 270 clockwise is equivalent to 90 counter-clockwise.
        ccw_rotation = types::RotateFlip::Enum::kRotateCcw90;
        break;
      default:
        FX_DCHECK(false);
        break;
    }

    const auto new_transform = geometry.transform.RotatedBy(ccw_rotation);
    layer.geometry = flatland::SrcToDest(
        geometry.src, types::RectangleF({.x = new_x, .y = new_y, .width = new_w, .height = new_h}),
        new_transform);
    final_layers.push_back(layer);
  }

  return final_layers;
}

}  // namespace screen_capture
