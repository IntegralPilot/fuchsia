// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_TYPES_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_TYPES_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <zircon/types.h>

#include <array>
#include <optional>
#include <variant>

#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/scheduling/id.h"
#include "src/ui/scenic/lib/types/blend_mode.h"
#include "src/ui/scenic/lib/types/rectangle.h"
#include "src/ui/scenic/lib/types/rectangle_f.h"
#include "src/ui/scenic/lib/types/rotate_flip.h"
#include "src/ui/scenic/lib/types/view_ref.h"

namespace flatland {

class LayerHandle {
 public:
  using InstanceId = scheduling::SessionId;

  LayerHandle() = default;
  LayerHandle(InstanceId instance_id, uint64_t layer_id)
      : instance_id_(instance_id), layer_id_(layer_id) {}

  // Allow copy and move ctors.
  LayerHandle(const LayerHandle& other) = default;
  LayerHandle& operator=(const LayerHandle& other) = default;
  LayerHandle(LayerHandle&& other) = default;
  LayerHandle& operator=(LayerHandle&& other) = default;

  // Default "Spaceship operator" generates all six comparison operators (==, !=, <, <=, >, >=)
  // by comparing each field in the order declared.
  auto operator<=>(const LayerHandle&) const = default;

  InstanceId GetInstanceId() const { return instance_id_; }
  uint64_t GetLayerId() const { return layer_id_; }

 private:
  friend struct std::hash<flatland::LayerHandle>;
  friend std::ostream& operator<<(std::ostream& out, const flatland::LayerHandle& h);

  InstanceId instance_id_ = 0;
  uint64_t layer_id_ = 0;
};

std::ostream& operator<<(std::ostream& out, const LayerHandle& h);

// The sample region to use for an image when texturing a rectangle.
using ImageSampleRegion = types::RectangleF;

// The clip region for a transform to bound its children.
using TransformClipRegion = types::Rectangle;

// Alpha blending mode.
using BlendMode = types::BlendMode;

using ViewRef = types::ViewRef;

// A mapping from a source region to a destination region, produced for each
// resolved layer: sample the src sub-rectangle of the content (unnormalized
// texture coordinates), apply transform (rotation and flip), and place the
// result at dest (global screen space, after all transforms and clipping).
// Solid color fills have no source; src is left default.
struct SrcToDest {
  SrcToDest(types::RectangleF src, types::RectangleF dest, types::RotateFlip transform)
      : src(src), dest(dest), transform(transform) {}

  explicit SrcToDest(types::RectangleF dest)
      : src(), dest(dest), transform(types::RotateFlip::kIdentity()) {}

  explicit SrcToDest(types::RectangleF::ConstructorArgs dest)
      : src(), dest(dest), transform(types::RotateFlip::kIdentity()) {}

  SrcToDest() = default;

  types::RectangleF src;
  types::RectangleF dest;
  types::RotateFlip transform = types::RotateFlip::kIdentity();

  // Two `SrcToDest` are identical if all of the following are true:
  // - transforms identical
  // - dests within epsilon-distance
  // - srcs within epsilon-distance
  bool operator==(const SrcToDest& other) const;
};

std::ostream& operator<<(std::ostream& str, const flatland::SrcToDest& s2d);

// A flexible representation of a flatland hit region.
class HitRegion {
 public:
  // Finite hit region with default interaction.
  explicit HitRegion(const types::RectangleF& region,
                     fuchsia_ui_composition::HitTestInteraction interaction =
                         fuchsia_ui_composition::HitTestInteraction::kDefault);
  explicit HitRegion(types::RectangleF::ConstructorArgs region,
                     fuchsia_ui_composition::HitTestInteraction interaction =
                         fuchsia_ui_composition::HitTestInteraction::kDefault);

  // Infinite hit region with default interaction.
  static HitRegion Infinite(fuchsia_ui_composition::HitTestInteraction interaction =
                                fuchsia_ui_composition::HitTestInteraction::kDefault);

  // Return true if region has finite extent.
  bool is_finite() const;

  // Finite region accessor. Caller ensures is_finite() is true.
  const types::RectangleF& region() const;

  // Hit test interaction accessor.
  fuchsia_ui_composition::HitTestInteraction interaction() const;

 private:
  // Helper for Infinite().
  explicit HitRegion(fuchsia_ui_composition::HitTestInteraction interaction);

  // Presence indicates a finite hit region.
  // Absence indicates an infinite hit region.
  std::optional<types::RectangleF> region_;

  fuchsia_ui_composition::HitTestInteraction interaction_ =
      fuchsia_ui_composition::HitTestInteraction::kDefault;
};

// Layer instance resolved from the global Flatland scene graph.
struct ResolvedLayer {
  // Reference to a sysmem image bound to a layer.
  struct ImageContent {
    allocation::GlobalImageId image_id = allocation::kInvalidImageId;
    uint32_t width = 0;   // full image width
    uint32_t height = 0;  // full image height
    bool operator==(const ImageContent&) const = default;
  };

  // A solid-color fill.  `color` is premultiplied, unlike the straight RGBA
  // that sessions store in UberStructLayer::SolidColorModeProperties::color;
  // this is done by the global flattening walk, and the blend mode is adjusted
  // accordingly (a stored kStraightAlpha blend mode resolves to kPremultipliedAlpha).
  // Layer and inherited opacity are NOT folded in here; they arrive via
  // `multiply_color`, as for image content.
  struct SolidColorContent {
    std::array<float, 4> color = {1.f, 1.f, 1.f, 1.f};
    bool operator==(const SolidColorContent&) const = default;
  };

  // The layer's resolved geometry: which source region it samples, the screen-space
  // destination it maps to, and the rotate/flip between them.
  SrcToDest geometry;

  // Encodes the effective opacity (layer opacity combined with inherited transform opacity),
  // using only the alpha channel when `blend_mode == kStraightAlpha`, and all 4 channels for
  // premultiplied blend modes.  Content opacity does not reside here; it is encoded in the
  // pixels for `ImageContent`, or in the `color` field of `SolidColorContent`.
  std::array<float, 4> multiply_color = {1.f, 1.f, 1.f, 1.f};
  types::BlendMode blend_mode = types::BlendMode::kReplace();
  std::variant<ImageContent, SolidColorContent> content;

  // Sentinel value representing an unset or invalid topology index (primarily for unit tests).
  static constexpr int32_t kInvalidTopologyIndex = -1;

  // Index of the Transform node in the global topology vector that produced this layer.
  // Used for debug dumps and (eventually, maybe) cross-frame layer identity tracking.
  int32_t topology_index = kInvalidTopologyIndex;

  bool operator==(const ResolvedLayer&) const = default;
};

std::ostream& operator<<(std::ostream& str, const flatland::ResolvedLayer& rl);

}  // namespace flatland

namespace std {

template <>
struct hash<flatland::LayerHandle> {
  size_t operator()(const flatland::LayerHandle& h) const noexcept {
    return hash<flatland::LayerHandle::InstanceId>{}(h.instance_id_) ^
           hash<uint64_t>{}(h.layer_id_);
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_FLATLAND_TYPES_H_
