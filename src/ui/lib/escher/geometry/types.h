// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_GEOMETRY_TYPES_H_
#define SRC_UI_LIB_ESCHER_GEOMETRY_TYPES_H_

#include <lib/syslog/cpp/macros.h>

#include <array>

#include "src/ui/lib/escher/util/debug_print.h"

#include <glm/glm.hpp>
#include <glm/gtc/epsilon.hpp>
#include <glm/gtc/matrix_transform.hpp>

namespace escher {

using glm::mat2;
using glm::mat3;
using glm::mat4;
using glm::vec2;
using glm::vec3;
using glm::vec4;

ESCHER_DEBUG_PRINTABLE(vec2);
ESCHER_DEBUG_PRINTABLE(vec3);
ESCHER_DEBUG_PRINTABLE(vec4);
ESCHER_DEBUG_PRINTABLE(mat2);
ESCHER_DEBUG_PRINTABLE(mat3);
ESCHER_DEBUG_PRINTABLE(mat4);

// A 2d, axis-aligned rectangle parameterized by an
// origin point and an extent representing the width
// and height. The extent must be >= 0. The uv coords
// are given in clockwise order, starting from the origin.
struct Rectangle2D {
  Rectangle2D(const vec2& in_origin, const vec2& in_extent,
              const std::array<vec2, 4>& in_uvs = {vec2(0, 0), vec2(1, 0), vec2(1, 1), vec2(0, 1)})
      : origin(in_origin), extent(in_extent), clockwise_uvs(in_uvs) {
    FX_CHECK(glm::all(glm::greaterThanEqual(extent, vec2(0.f))));
  }
  Rectangle2D() = default;

  glm::vec2 origin = vec2(0, 0);
  glm::vec2 extent = vec2(1, 1);
  std::array<vec2, 4> clockwise_uvs = {vec2(0, 0), vec2(1, 0), vec2(1, 1), vec2(0, 1)};

  bool operator==(const Rectangle2D& other) const {
    // TODO(https://fxbug.dev/42151723): This epsilon should be unified with the one below, along
    // with everywhere else we are using an epsilon value in Escher code.
    constexpr float kRectangleEpislon = 0.001f;
    return glm::all(glm::epsilonEqual(origin, other.origin, kRectangleEpislon)) &&
           glm::all(glm::epsilonEqual(extent, other.extent, kRectangleEpislon)) &&
           glm::all(
               glm::epsilonEqual(clockwise_uvs[0], other.clockwise_uvs[0], kRectangleEpislon)) &&
           glm::all(
               glm::epsilonEqual(clockwise_uvs[1], other.clockwise_uvs[1], kRectangleEpislon)) &&
           glm::all(
               glm::epsilonEqual(clockwise_uvs[2], other.clockwise_uvs[2], kRectangleEpislon)) &&
           glm::all(glm::epsilonEqual(clockwise_uvs[3], other.clockwise_uvs[3], kRectangleEpislon));
  }
};

ESCHER_DEBUG_PRINTABLE(Rectangle2D);

// A ray with an origin and a direction of travel.
struct ray4 {
  // The ray's origin point in space.
  // Must be homogeneous (last component must be non-zero).
  glm::vec4 origin;

  // The ray's direction vector in space.
  // This is not necessarily a unit vector. The last component must be zero.
  glm::vec4 direction;

  // Gets the coordinate point along the ray for a given parameterized distance.
  glm::vec4 At(const float t) const { return origin + t * direction; }
};

// Used to compare whether two values are nearly equal.
constexpr float kEpsilon = 0.000001f;

inline ray4 operator*(const glm::mat4& matrix, const ray4& ray) {
  FX_DCHECK(ray.direction.w == 0) << "Ray direction should not be subject to translation.";
  return ray4{matrix * ray.origin, matrix * ray.direction};
}

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_GEOMETRY_TYPES_H_
