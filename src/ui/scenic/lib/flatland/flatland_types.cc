// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_types.h"

#include <lib/syslog/cpp/macros.h>

namespace flatland {

std::ostream& operator<<(std::ostream& str, const flatland::SrcToDest& s2d) {
  return str << "SrcToDest[src:" << s2d.src << " dest:" << s2d.dest
             << " transform:" << s2d.transform << "]";
}

bool SrcToDest::operator==(const SrcToDest& other) const {
  constexpr float kEpsilon = 0.001f;
  const auto approx_equal = [](const types::RectangleF& a, const types::RectangleF& b) {
    return std::abs(a.x() - b.x()) < kEpsilon && std::abs(a.y() - b.y()) < kEpsilon &&
           std::abs(a.width() - b.width()) < kEpsilon &&
           std::abs(a.height() - b.height()) < kEpsilon;
  };
  return approx_equal(dest, other.dest) && approx_equal(src, other.src) &&
         transform == other.transform;
}

HitRegion::HitRegion(const types::RectangleF& region,
                     fuchsia_ui_composition::HitTestInteraction interaction)
    : region_(std::make_optional(region)), interaction_(interaction) {}

HitRegion::HitRegion(types::RectangleF::ConstructorArgs region,
                     fuchsia_ui_composition::HitTestInteraction interaction)
    : HitRegion(types::RectangleF(region), interaction) {}

HitRegion HitRegion::Infinite(fuchsia_ui_composition::HitTestInteraction interaction) {
  return HitRegion(interaction);
}

bool HitRegion::is_finite() const { return region_.has_value(); }

const types::RectangleF& HitRegion::region() const {
  FX_DCHECK(region_.has_value()) << "region accessor needs a value";
  return region_.value();
}

fuchsia_ui_composition::HitTestInteraction HitRegion::interaction() const { return interaction_; }

HitRegion::HitRegion(fuchsia_ui_composition::HitTestInteraction interaction)
    : interaction_(interaction) {}

std::ostream& operator<<(std::ostream& out, const LayerHandle& h) {
  out << "(L:" << h.GetInstanceId() << ":" << h.GetLayerId() << ")";
  return out;
}

std::ostream& operator<<(std::ostream& str, const flatland::ResolvedLayer& rl) {
  static_assert(2 == std::variant_size_v<decltype(flatland::ResolvedLayer::content)>,
                "operator<< must be updated to support new content types");

  str << "ResolvedLayer[geometry:" << rl.geometry << " multiply_color:(" << rl.multiply_color[0]
      << "," << rl.multiply_color[1] << "," << rl.multiply_color[2] << "," << rl.multiply_color[3]
      << ")"
      << " blend_mode:" << rl.blend_mode;
  if (std::holds_alternative<flatland::ResolvedLayer::SolidColorContent>(rl.content)) {
    const auto& solid = std::get<flatland::ResolvedLayer::SolidColorContent>(rl.content);
    str << " content:SolidColor(" << solid.color[0] << "," << solid.color[1] << ","
        << solid.color[2] << "," << solid.color[3] << ")";
  } else {
    const auto& image = std::get<flatland::ResolvedLayer::ImageContent>(rl.content);
    str << " content:Image(id:" << image.image_id << " size:" << image.width << "x" << image.height
        << ")";
  }
  str << " topology_index:" << rl.topology_index << "]";
  return str;
}

}  // namespace flatland
