// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/renderer/null_renderer.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <optional>

namespace flatland {

fpromise::promise<> NullRenderer::ImportBufferCollection(
    allocation::GlobalBufferCollectionId collection_id,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, BufferCollectionUsage usage,
    std::optional<fuchsia_math::SizeU> size) {
  FX_DCHECK(collection_id != allocation::kInvalidId);
  FX_DCHECK(token.is_valid());

  std::scoped_lock lock(lock_);
  auto& map = GetBufferCollectionInfosFor(usage);
  if (map.find(collection_id) != map.end()) {
    FX_LOGS(ERROR) << "Duplicate GlobalBufferCollectionID: " << collection_id;
    return fpromise::make_error_promise();
  }
  std::optional<fuchsia_sysmem2::ImageFormatConstraints> image_constraints;
  if (size.has_value()) {
    fuchsia_sysmem2::ImageFormatConstraints constraints;
    constraints.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
    constraints.color_spaces(std::vector{fuchsia_images2::ColorSpace::kSrgb});
    constraints.required_min_size(fuchsia_math::SizeU(size->width(), size->height()));
    constraints.required_max_size(fuchsia_math::SizeU(size->width(), size->height()));
    image_constraints = std::move(constraints);
  }
  fuchsia_sysmem2::BufferUsage sysmem_usage;
  sysmem_usage.none(fuchsia_sysmem2::kNoneUsage);
  auto result =
      BufferCollectionInfo::New(sysmem_allocator, std::move(token), std::move(image_constraints),
                                std::move(sysmem_usage), usage);
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Unable to register collection.";
    return fpromise::make_error_promise();
  }

  // Multiple threads may be attempting to read/write from |map| so we
  // lock this function here.
  // TODO(https://fxbug.dev/42120738): Convert this to a lock-free structure.
  map[collection_id] = std::move(result.value());
  return fpromise::make_ok_promise();
}

void NullRenderer::ReleaseBufferCollection(allocation::GlobalBufferCollectionId collection_id,
                                           BufferCollectionUsage usage) {
  // Multiple threads may be attempting to read/write from the various maps,
  // lock this function here.
  // TODO(https://fxbug.dev/42120738): Convert this to a lock-free structure.
  std::scoped_lock lock(lock_);
  auto& map = GetBufferCollectionInfosFor(usage);

  auto collection_itr = map.find(collection_id);

  // If the collection is not in the map, then there's nothing to do.
  if (collection_itr == map.end()) {
    return;
  }

  // Erase the sysmem collection from the map.
  map.erase(collection_id);
}

fpromise::promise<> NullRenderer::ImportBufferImage(const allocation::ImageMetadata& metadata,
                                                    BufferCollectionUsage usage) {
  // The metadata can't have an invalid collection id.
  if (metadata.collection_id == allocation::kInvalidId) {
    FX_LOGS(WARNING) << "Image has invalid collection id.";
    return fpromise::make_error_promise();
  }

  // The metadata can't have an invalid identifier.
  if (metadata.identifier == allocation::kInvalidImageId) {
    FX_LOGS(WARNING) << "Image has invalid identifier.";
    return fpromise::make_error_promise();
  }

  std::scoped_lock lock(lock_);
  auto& map = GetBufferCollectionInfosFor(usage);

  const auto& collection_itr = map.find(metadata.collection_id);
  if (collection_itr == map.end()) {
    FX_LOGS(ERROR) << "Collection with id " << metadata.collection_id << " does not exist.";
    return fpromise::make_error_promise();
  }

  auto& collection = collection_itr->second;
  if (!collection.BuffersAreAllocated()) {
    FX_LOGS(ERROR) << "Buffers for collection " << metadata.collection_id
                   << " have not been allocated.";
    return fpromise::make_error_promise();
  }

  const auto& sysmem_info = collection.GetSysmemInfo();
  const auto vmo_count = sysmem_info.buffers().value().size();
  const auto& image_constraints = sysmem_info.settings().value().image_format_constraints().value();

  if (metadata.vmo_index >= vmo_count) {
    FX_LOGS(ERROR) << "ImportBufferImage failed, vmo_index " << metadata.vmo_index
                   << " must be less than vmo_count " << vmo_count;
    return fpromise::make_error_promise();
  }

  if (metadata.width < image_constraints.min_size().value().width() ||
      metadata.width > image_constraints.max_size().value().width()) {
    FX_LOGS(ERROR) << "ImportBufferImage failed, width " << metadata.width
                   << " is not within valid range [" << image_constraints.min_size().value().width()
                   << "," << image_constraints.max_size().value().width() << "]";
    return fpromise::make_error_promise();
  }

  if (metadata.height < image_constraints.min_size().value().height() ||
      metadata.height > image_constraints.max_size().value().height()) {
    FX_LOGS(ERROR) << "ImportBufferImage failed, height " << metadata.height
                   << " is not within valid range ["
                   << image_constraints.min_size().value().height() << ","
                   << image_constraints.max_size().value().height() << "]";
    return fpromise::make_error_promise();
  }

  if (usage == BufferCollectionUsage::kClientImage) {
    image_map_[metadata.identifier] = image_constraints;
  }
  return fpromise::make_ok_promise();
}

void NullRenderer::ReleaseBufferImage(allocation::GlobalImageId image_id) {
  FX_DCHECK(image_id != display::kInvalidImageId);
  std::scoped_lock lock(lock_);
  image_map_.erase(image_id);
}

void NullRenderer::SetColorConversionValues(const fidl::Array<float, 9>& coefficients,
                                            const fidl::Array<float, 3>& preoffsets,
                                            const fidl::Array<float, 3>& postoffsets) {}

// Check that the buffer collections for each of the images passed in have been validated.
// DCHECK if they have not.
void NullRenderer::Render(const allocation::ImageMetadata& render_target,
                          std::span<const ResolvedLayer> layers, const RenderArgs& render_args) {
  std::scoped_lock lock(lock_);
  for (const auto& layer : layers) {
    if (std::holds_alternative<ResolvedLayer::SolidColorContent>(layer.content)) {
      // Solid color layer.
      continue;
    }

    const auto& image = std::get<ResolvedLayer::ImageContent>(layer.content);
    const auto& image_map_itr_ = image_map_.find(image.image_id);
    FX_DCHECK(image_map_itr_ != image_map_.end());
    const auto& image_constraints = image_map_itr_->second;

    // Make sure the image conforms to the constraints of the collection.
    FX_DCHECK(image.width <= image_constraints.max_size().value().width());
    FX_DCHECK(image.height <= image_constraints.max_size().value().height());
  }

  // Fire all of the release fences.
  for (auto& fence : render_args.release_fences) {
    fence.signal(0, ZX_EVENT_SIGNALED);
  }
}

fuchsia_images2::PixelFormat NullRenderer::ChoosePreferredRenderTargetFormat(
    const std::vector<fuchsia_images2::PixelFormat>& available_formats) const {
  for (const auto& format : available_formats) {
    if (format == fuchsia_images2::PixelFormat::kB8G8R8A8) {
      return format;
    }
  }
  FX_DCHECK(false) << "Preferred format is not available.";
  return fuchsia_images2::PixelFormat::kInvalid;
}

bool NullRenderer::SupportsRenderInProtected() const { return false; }

bool NullRenderer::RequiresRenderInProtected(std::span<const ResolvedLayer> layers) const {
  return false;
}

std::unordered_map<allocation::GlobalBufferCollectionId, BufferCollectionInfo>&
NullRenderer::GetBufferCollectionInfosFor(BufferCollectionUsage usage) {
  switch (usage) {
    case BufferCollectionUsage::kRenderTarget:
      return render_target_map_;
    case BufferCollectionUsage::kReadback:
      return readback_map_;
    case BufferCollectionUsage::kClientImage:
      return client_image_map_;
    default:
      FX_NOTREACHED();
      return render_target_map_;
  }
}

}  // namespace flatland
