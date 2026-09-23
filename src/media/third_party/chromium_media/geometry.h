// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_GEOMETRY_H_
#define SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_GEOMETRY_H_

#include <stdint.h>
#include <zircon/assert.h>

#include <algorithm>
#include <array>
#include <optional>
#include <string>

#include <safemath/safe_math.h>

namespace gfx {

struct HdrMetadataCta861_3 {
  uint16_t max_content_light_level = 0;
  uint16_t max_frame_average_light_level = 0;
  bool operator==(const HdrMetadataCta861_3& rhs) const = default;
};

struct HdrMetadataSmpteSt2086 {
  std::array<std::array<uint16_t, 2>, 3> display_primaries = {};
  std::array<uint16_t, 2> white_point = {};
  uint32_t max_luminance = 0;
  uint32_t min_luminance = 0;
  bool operator==(const HdrMetadataSmpteSt2086& rhs) const = default;
};

struct HDRMetadata {
  std::optional<HdrMetadataCta861_3> cta_861_3;
  std::optional<HdrMetadataSmpteSt2086> smpte_st_2086;

  bool IsEmpty() const { return !cta_861_3 && !smpte_st_2086; }
  bool operator==(const HDRMetadata& rhs) const = default;
};
class Size {
 public:
  constexpr Size() : width_(0), height_(0) {}
  constexpr Size(int width, int height)
      : width_(std::max(0, width)), height_(std::max(0, height)) {}

  constexpr int width() const { return width_; }
  constexpr int height() const { return height_; }
  // GetArea() was intentionally removed to prevent signed integer overflow.
  // When merging incoming usages of GetArea() from upstream Chromium, switch
  // them to GetCheckedArea() or Area64().
  constexpr safemath::CheckedNumeric<int> GetCheckedArea() const {
    return safemath::CheckMul(width_, height_);
  }
  // Size constructors and mutators clamp width_ and height_ to >= 0 when set,
  // maintaining the class invariant that width_ and height_ are always in
  // [0, INT_MAX]. Thus casting to uint64_t cannot sign-extend, and the product
  // cannot overflow uint64_t.
  constexpr uint64_t Area64() const {
    ZX_DEBUG_ASSERT(width_ >= 0 && height_ >= 0);
    return static_cast<uint64_t>(width_) * static_cast<uint64_t>(height_);
  }

  void set_width(int width) { width_ = std::max(0, width); }
  void set_height(int height) { height_ = std::max(0, height); }

  void SetSize(int width, int height) {
    set_width(width);
    set_height(height);
  }

  void SetToMin(const Size& other) {
    width_ = std::min(width_, other.width_);
    height_ = std::min(height_, other.height_);
  }

  void SetToMax(const Size& other) {
    width_ = std::max(width_, other.width_);
    height_ = std::max(height_, other.height_);
  }

  bool IsEmpty() const { return width_ == 0 || height_ == 0; }
  std::string ToString() const {
    return std::to_string(width_) + "x" + std::to_string(height_);
  }

 private:
  int width_;
  int height_;
};

inline bool operator==(const Size& lhs, const Size& rhs) {
  return lhs.width() == rhs.width() && lhs.height() == rhs.height();
}

inline bool operator!=(const Size& lhs, const Size& rhs) {
  return !(lhs == rhs);
}

class Point {
 public:
  constexpr Point() : x_(0), y_(0) {}
  constexpr Point(int x, int y) : x_(x), y_(y) {}

  constexpr int x() const { return x_; }
  constexpr int y() const { return y_; }
  void set_x(int x) { x_ = x; }
  void set_y(int y) { y_ = y; }
  std::string ToString() const {
    return std::to_string(x_) + "," + std::to_string(y_);
  }

 private:
  int x_;
  int y_;
};

inline bool operator==(const Point& lhs, const Point& rhs) {
  return lhs.x() == rhs.x() && lhs.y() == rhs.y();
}

inline bool operator!=(const Point& lhs, const Point& rhs) {
  return !(lhs == rhs);
}

class Rect {
 public:
  constexpr Rect() = default;

  constexpr Rect(int width, int height) : size_(width, height) {}

  constexpr Rect(int x, int y, int width, int height)
      : origin_(x, y), size_(width, height) {}

  constexpr explicit Rect(const Size& size) : size_(size) {}

  constexpr int x() const { return origin_.x(); }
  void set_x(int x) { origin_.set_x(x); }

  constexpr int y() const { return origin_.y(); }
  void set_y(int y) { origin_.set_y(y); }

  constexpr int width() const { return size_.width(); }
  void set_width(int width) { size_.set_width(width); }

  constexpr int height() const { return size_.height(); }
  void set_height(int height) { size_.set_height(height); }

  // Intentionally diverges from upstream Chromium by returning
  // CheckedNumeric<int> without clamping width() or height() on overflow.
  constexpr safemath::CheckedNumeric<int> right() const {
    return safemath::CheckAdd(x(), width());
  }
  constexpr safemath::CheckedNumeric<int> bottom() const {
    return safemath::CheckAdd(y(), height());
  }

  constexpr bool IsValid() const {
    return right().IsValid() && bottom().IsValid();
  }

  constexpr const Point& origin() const { return origin_; }
  void set_origin(const Point& origin) { origin_ = origin; }

  constexpr const Size& size() const { return size_; }
  void set_size(const Size& size) {
    set_width(size.width());
    set_height(size.height());
  }

  // Intentionally diverges from upstream Chromium by asserting IsValid() in
  // both release and debug builds before comparing boundaries.
  constexpr bool Contains(int point_x, int point_y) const {
    ZX_ASSERT(IsValid());
    return (point_x >= x()) && (point_x < right().ValueOrDie()) &&
           (point_y >= y()) && (point_y < bottom().ValueOrDie());
  }

  constexpr bool Contains(const Rect& rect) const {
    ZX_ASSERT(IsValid());
    ZX_ASSERT(rect.IsValid());
    return (
        rect.x() >= x() && rect.right().ValueOrDie() <= right().ValueOrDie() &&
        rect.y() >= y() && rect.bottom().ValueOrDie() <= bottom().ValueOrDie());
  }

  std::string ToString() const {
    const char* format = "x: %d y: %d width: %d height: %d";
    int chars = snprintf(nullptr, 0, format, x(), y(), width(), height());
    auto char_array = std::make_unique<char[]>(chars + 1);
    int chars2 = snprintf(char_array.get(), chars + 1, format, x(), y(),
                          width(), height());
    ZX_DEBUG_ASSERT(chars == chars2);
    return std::string(char_array.get(), chars);
  }

 private:
  gfx::Point origin_;
  gfx::Size size_;
};

inline bool operator==(const Rect& lhs, const Rect& rhs) {
  return lhs.origin() == rhs.origin() && lhs.size() == rhs.size();
}

inline bool operator!=(const Rect& lhs, const Rect& rhs) {
  return !(lhs == rhs);
}

}  // namespace gfx

#endif  // SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_GEOMETRY_H_
