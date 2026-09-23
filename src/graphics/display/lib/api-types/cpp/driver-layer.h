// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_LAYER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_LAYER_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.engine/DriverLayer`].
//
// Instances are guaranteed to represent valid layer definitions.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class DriverLayer {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // True iff `layer` is convertible to a valid DriverLayer.
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_display_engine::wire::Layer& fidl_layer);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr DriverLayer(const DriverLayer::ConstructorArgs& args);

  // `fidl_layer` must be convertible to a valid DriverLayer.
  explicit constexpr DriverLayer(const fuchsia_hardware_display_engine::wire::Layer& fidl_layer);

  constexpr DriverLayer(const DriverLayer&) noexcept = default;
  constexpr DriverLayer(DriverLayer&&) noexcept = default;
  constexpr DriverLayer& operator=(const DriverLayer&) noexcept = default;
  constexpr DriverLayer& operator=(DriverLayer&&) noexcept = default;
  ~DriverLayer() = default;

  friend constexpr bool operator==(const DriverLayer& lhs, const DriverLayer& rhs);
  friend constexpr bool operator!=(const DriverLayer& lhs, const DriverLayer& rhs);

  constexpr fuchsia_hardware_display_engine::wire::Layer ToFidl() const;

  const Rectangle& display_destination() const { return display_destination_; }
  const Rectangle& image_source() const { return image_source_; }
  DriverImageId image_id() const { return image_id_; }
  const ImageMetadata& image_metadata() const { return image_metadata_; }
  const Color& fallback_color() const { return fallback_color_; }

  // Out-of-tree drivers must not use this method, because it will be reworked.
  AlphaMode alpha_mode() const { return alpha_mode_; }

  // Out-of-tree drivers must not use this method, because it will be reworked.
  float alpha_coefficient() const { return alpha_coefficient_; }

  CoordinateTransformation image_source_transformation() const {
    return image_source_transformation_;
  }

 private:
  struct ConstructorArgs {
    Rectangle display_destination;
    Rectangle image_source;
    DriverImageId image_id;
    ImageMetadata image_metadata;
    Color fallback_color;
    AlphaMode alpha_mode = AlphaMode::kDisable;
    float alpha_coefficient = 0;
    CoordinateTransformation image_source_transformation = CoordinateTransformation::kIdentity;
  };

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const DriverLayer::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(
      const fuchsia_hardware_display_engine::wire::Layer& fidl_layer);

  Rectangle display_destination_;
  Rectangle image_source_;
  DriverImageId image_id_;
  ImageMetadata image_metadata_;
  Color fallback_color_;
  AlphaMode alpha_mode_;
  float alpha_coefficient_;
  CoordinateTransformation image_source_transformation_;
};

// static
constexpr bool DriverLayer::IsValid(
    const fuchsia_hardware_display_engine::wire::Layer& fidl_layer) {
  // Constraints on `display_destination`.
  if (!Rectangle::IsValid(fidl_layer.display_destination)) {
    return false;
  }
  if (Rectangle::From(fidl_layer.display_destination).dimensions().IsEmpty()) {
    return false;
  }

  // Constraints on `image_source`.
  if (!Rectangle::IsValid(fidl_layer.image_source)) {
    return false;
  }
  const Rectangle image_source = Rectangle::From(fidl_layer.image_source);

  // Constraints on `image_id`.
  const DriverImageId image_id = DriverImageId(fidl_layer.image_id);
  if (image_source.dimensions().IsEmpty() && (image_id != kInvalidDriverImageId)) {
    return false;
  }

  // Constraints on `image_metadata`.
  if (!ImageMetadata::IsValid(fidl_layer.image_metadata)) {
    return false;
  }
  const ImageMetadata image_metadata(fidl_layer.image_metadata);
  if (image_source.dimensions().IsEmpty() != image_metadata.dimensions().IsEmpty()) {
    return false;
  }
  if (image_source.dimensions().IsEmpty() &&
      (image_metadata.tiling_type() != ImageTilingType::kLinear)) {
    return false;
  }
  // TODO(costan): `image_source` must be contained in `image_metadata`.

  // Constraints on `fallback_color`.
  if (!Color::IsValid(fidl_layer.fallback_color)) {
    return false;
  }

  // Constraints on `image_source_transformation`.
  if (!CoordinateTransformation::IsValid(fidl_layer.image_source_transformation)) {
    return false;
  }
  const CoordinateTransformation image_source_transformation(
      fidl_layer.image_source_transformation);
  if (image_source.dimensions().IsEmpty() &&
      (image_source_transformation != CoordinateTransformation::kIdentity)) {
    return false;
  }

  return true;
}

constexpr DriverLayer::DriverLayer(const DriverLayer::ConstructorArgs& args)
    : display_destination_(args.display_destination),
      image_source_(args.image_source),
      image_id_(args.image_id),
      image_metadata_(args.image_metadata),
      fallback_color_(args.fallback_color),
      alpha_mode_(args.alpha_mode),
      alpha_coefficient_(args.alpha_coefficient),
      image_source_transformation_(args.image_source_transformation) {
  DebugAssertIsValid(args);
}

constexpr DriverLayer::DriverLayer(const fuchsia_hardware_display_engine::wire::Layer& fidl_layer)
    : display_destination_(Rectangle::From(fidl_layer.display_destination)),
      image_source_(Rectangle::From(fidl_layer.image_source)),
      image_id_(DriverImageId(fidl_layer.image_id)),
      image_metadata_(fidl_layer.image_metadata),
      fallback_color_(Color::From(fidl_layer.fallback_color)),
      alpha_mode_(fidl_layer.alpha_mode),
      alpha_coefficient_(fidl_layer.alpha_layer_val),
      image_source_transformation_(fidl_layer.image_source_transformation) {
  DebugAssertIsValid(fidl_layer);
}

constexpr bool operator==(const DriverLayer& lhs, const DriverLayer& rhs) {
  return lhs.display_destination_ == rhs.display_destination_ &&
         lhs.image_source_ == rhs.image_source_ && lhs.image_id_ == rhs.image_id_ &&
         lhs.image_metadata_ == rhs.image_metadata_ && lhs.fallback_color_ == rhs.fallback_color_ &&
         lhs.alpha_mode_ == rhs.alpha_mode_ && lhs.alpha_coefficient_ == rhs.alpha_coefficient_ &&
         lhs.image_source_transformation_ == rhs.image_source_transformation_;
}

constexpr bool operator!=(const DriverLayer& lhs, const DriverLayer& rhs) { return !(lhs == rhs); }

constexpr fuchsia_hardware_display_engine::wire::Layer DriverLayer::ToFidl() const {
  return fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = display_destination_.ToFidl(),
      .image_source = image_source_.ToFidl(),
      .image_id = image_id_.ToFidl(),
      .image_metadata = image_metadata_.ToFidl(),
      .fallback_color = fallback_color_.ToFidl(),
      .alpha_mode = alpha_mode_.ToFidl(),
      .alpha_layer_val = alpha_coefficient_,
      .image_source_transformation = image_source_transformation_.ToFidl(),
  };
}

// static
constexpr void DriverLayer::DebugAssertIsValid(const DriverLayer::ConstructorArgs& args) {
  // Constraints on `display_destination`.
  ZX_DEBUG_ASSERT(!args.display_destination.dimensions().IsEmpty());

  // Constraints on `image_source`.
  ZX_DEBUG_ASSERT(!args.image_source.dimensions().IsEmpty() ||
                  args.image_id == kInvalidDriverImageId);

  // Constraints on `image_metadata`.
  ZX_DEBUG_ASSERT(args.image_source.dimensions().IsEmpty() ==
                  args.image_metadata.dimensions().IsEmpty());
  ZX_DEBUG_ASSERT(!args.image_source.dimensions().IsEmpty() ||
                  args.image_metadata.tiling_type() == ImageTilingType::kLinear);
  // TODO(costan): `image_source` must be contained in `image_metadata`.

  // Constraints on `image_source_transformation`.
  ZX_DEBUG_ASSERT(!args.image_source.dimensions().IsEmpty() ||
                  args.image_source_transformation == CoordinateTransformation::kIdentity);
}

// static
constexpr void DriverLayer::DebugAssertIsValid(
    const fuchsia_hardware_display_engine::wire::Layer& fidl_layer) {
  // Constraints on `display_destination`.
  ZX_DEBUG_ASSERT(Rectangle::IsValid(fidl_layer.display_destination));
  ZX_DEBUG_ASSERT(!Rectangle::From(fidl_layer.display_destination).dimensions().IsEmpty());

  // Constraints on `image_source`.
  ZX_DEBUG_ASSERT(Rectangle::IsValid(fidl_layer.image_source));
  const Rectangle image_source = Rectangle::From(fidl_layer.image_source);

  // Constraints on `image_id`.
  const DriverImageId image_id = DriverImageId(fidl_layer.image_id);
  ZX_DEBUG_ASSERT(!image_source.dimensions().IsEmpty() || image_id == kInvalidDriverImageId);

  // Constraints on `image_metadata`.
  ZX_DEBUG_ASSERT(ImageMetadata::IsValid(fidl_layer.image_metadata));
  const ImageMetadata image_metadata(fidl_layer.image_metadata);
  ZX_DEBUG_ASSERT(image_source.dimensions().IsEmpty() == image_metadata.dimensions().IsEmpty());
  ZX_DEBUG_ASSERT(!image_source.dimensions().IsEmpty() ||
                  image_metadata.tiling_type() == ImageTilingType::kLinear);
  // TODO(costan): `image_source` must be contained in `image_metadata`.

  // Constraints on `fallback_color`.
  ZX_DEBUG_ASSERT(Color::IsValid(fidl_layer.fallback_color));

  // Constraints on `image_source_transformation`.
  ZX_DEBUG_ASSERT(CoordinateTransformation::IsValid(fidl_layer.image_source_transformation));
  const CoordinateTransformation image_source_transformation(
      fidl_layer.image_source_transformation);
  ZX_DEBUG_ASSERT(!image_source.dimensions().IsEmpty() ||
                  image_source_transformation == CoordinateTransformation::kIdentity);
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_LAYER_H_
