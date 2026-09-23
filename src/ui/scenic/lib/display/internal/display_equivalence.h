// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_DISPLAY_EQUIVALENCE_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_DISPLAY_EQUIVALENCE_H_

#include "src/ui/scenic/lib/display/internal/layer_equivalence.h"
#include "src/ui/scenic/lib/types/display_mode.h"

namespace display::internal {

// Represents the subset of the Display's configuration relevant for
// `fuchsia.hardware.display.Coordinator/CheckConfig()`.  This struct defines an
// **equivalence class**: any two display configurations that produce an identical
// `DisplayEquivalence` object are considered equivalent for the purpose of `CheckConfig()`
// validation.
//
// This aggregates `LayerEquivalence`s for active layers and display-wide
// properties like mode and color conversion settings.  This is used to key the
// cache for `CheckConfig()` results in `CoordinatorProxy`.  The order of layers
// in the `layers` vector matters, as it corresponds to the order provided to
// the display hardware.
struct DisplayEquivalence {
  std::vector<LayerEquivalence> layers;
  types::DisplayMode display_mode;
  // TODO(https://fxbug.dev/446042966): small changes to these might cause us to thrash the
  // `CheckConfig()` cache in `CoordinatorProxy`.  See bug for possible mitigations.
  std::array<float, 3> color_conversion_preoffsets = {};
  std::array<float, 9> color_conversion_coefficients = {};
  std::array<float, 3> color_conversion_postoffsets = {};

  constexpr bool operator==(const DisplayEquivalence& other) const {
    return layers == other.layers && display_mode == other.display_mode &&
           color_conversion_preoffsets == other.color_conversion_preoffsets &&
           color_conversion_coefficients == other.color_conversion_coefficients &&
           color_conversion_postoffsets == other.color_conversion_postoffsets;
  };
};

std::ostream& operator<<(std::ostream& str, const DisplayEquivalence& e);

}  // namespace display::internal

namespace std {

template <>
struct hash<display::internal::DisplayEquivalence> {
  std::size_t operator()(const display::internal::DisplayEquivalence& spec) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x5804602f4cac9f58;
    for (auto& layer : spec.layers) {
      types::hash_combine(seed, layer);
    }
    types::hash_combine(seed, spec.display_mode);

    // std::array doesn't have std::hash specialization.
    // We use "Golden Ratio" hashing, as popularized by Knuth.
    std::hash<float> hasher;
    constexpr std::size_t kGoldenRatio = 0x9e3779b9;
    for (const auto& val : spec.color_conversion_preoffsets) {
      seed ^= hasher(val) + kGoldenRatio + (seed << 6) + (seed >> 2);
    }
    for (const auto& val : spec.color_conversion_coefficients) {
      seed ^= hasher(val) + kGoldenRatio + (seed << 6) + (seed >> 2);
    }
    for (const auto& val : spec.color_conversion_postoffsets) {
      seed ^= hasher(val) + kGoldenRatio + (seed << 6) + (seed >> 2);
    }

    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_DISPLAY_EQUIVALENCE_H_
