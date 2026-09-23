// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_MODE_AND_ID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_MODE_AND_ID_H_

#include <zircon/assert.h>

#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace display {

// Bundles a display mode and the ID that represents it.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class ModeAndId {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr ModeAndId(const ModeAndId::ConstructorArgs& args);

  constexpr ModeAndId(const ModeAndId&) noexcept = default;
  constexpr ModeAndId(ModeAndId&&) noexcept = default;
  constexpr ModeAndId& operator=(const ModeAndId&) noexcept = default;
  constexpr ModeAndId& operator=(ModeAndId&&) noexcept = default;
  ~ModeAndId() = default;

  friend constexpr bool operator==(const ModeAndId& lhs, const ModeAndId& rhs);
  friend constexpr bool operator!=(const ModeAndId& lhs, const ModeAndId& rhs);

  constexpr const Mode& mode() const { return mode_; }
  constexpr ModeId id() const { return id_; }

 private:
  struct ConstructorArgs {
    ModeId id;
    Mode mode;
  };

  ModeId id_;
  Mode mode_;
};

constexpr ModeAndId::ModeAndId(const ModeAndId::ConstructorArgs& args)
    : id_(args.id), mode_(args.mode) {
  ZX_DEBUG_ASSERT(args.id != kInvalidModeId);
}

constexpr bool operator==(const ModeAndId& lhs, const ModeAndId& rhs) {
  return lhs.id_ == rhs.id_ && lhs.mode_ == rhs.mode_;
}

constexpr bool operator!=(const ModeAndId& lhs, const ModeAndId& rhs) { return !(lhs == rhs); }

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_MODE_AND_ID_H_
