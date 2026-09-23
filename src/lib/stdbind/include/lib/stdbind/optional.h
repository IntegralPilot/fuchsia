// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_STDBIND_INCLUDE_LIB_STDBIND_OPTIONAL_H_
#define SRC_LIB_STDBIND_INCLUDE_LIB_STDBIND_OPTIONAL_H_

#include <compare>
#include <cstdint>
#include <optional>
#include <type_traits>
#include <utility>

namespace stdbind {

// An ABI-stable, standard-layout adapter for `std::optional<T>`, designed for
// FFI interoperability with the Rust `stdbind::Optional<T>`.
template <typename T>
class optional {
 public:
  static_assert(std::is_standard_layout_v<T>,
                "stdbind::optional<T> requires a standard-layout type");
  static_assert(std::is_trivially_copyable_v<T>,
                "stdbind::optional<T> requires a trivially copyable type");

  constexpr optional() noexcept : tag_(Tag::kNone), empty_{} {}
  constexpr optional(std::nullopt_t) noexcept : tag_(Tag::kNone), empty_{} {}

  template <typename U = T>
    requires std::is_constructible_v<T, U>
  constexpr optional(U&& val) noexcept : tag_(Tag::kSome), value_(std::forward<U>(val)) {}

  explicit constexpr optional(const std::optional<T>& opt) noexcept {
    if (opt.has_value()) {
      tag_ = Tag::kSome;
      value_ = *opt;
    } else {
      tag_ = Tag::kNone;
      empty_ = {};
    }
  }

  explicit constexpr optional(std::optional<T>&& opt) noexcept {
    if (opt.has_value()) {
      tag_ = Tag::kSome;
      value_ = *std::move(opt);
    } else {
      tag_ = Tag::kNone;
      empty_ = {};
    }
  }

  constexpr optional(const optional&) = default;
  constexpr optional(optional&&) = default;
  constexpr optional& operator=(const optional&) = default;
  constexpr optional& operator=(optional&&) = default;
  constexpr ~optional() = default;

  constexpr optional& operator=(std::nullopt_t) noexcept {
    tag_ = Tag::kNone;
    return *this;
  }

  constexpr optional& operator=(const T& val) noexcept {
    tag_ = Tag::kSome;
    value_ = val;
    return *this;
  }

  constexpr optional& operator=(T&& val) noexcept {
    tag_ = Tag::kSome;
    value_ = std::move(val);
    return *this;
  }

  constexpr optional& operator=(const std::optional<T>& opt) noexcept {
    if (opt.has_value()) {
      tag_ = Tag::kSome;
      value_ = *opt;
    } else {
      tag_ = Tag::kNone;
    }
    return *this;
  }

  constexpr optional& operator=(std::optional<T>&& opt) noexcept {
    if (opt.has_value()) {
      tag_ = Tag::kSome;
      value_ = *std::move(opt);
    } else {
      tag_ = Tag::kNone;
    }
    return *this;
  }

  [[nodiscard]] constexpr bool has_value() const noexcept { return tag_ == Tag::kSome; }

  explicit constexpr operator bool() const noexcept { return has_value(); }

  [[nodiscard]] constexpr std::optional<T> to_std() const noexcept {
    if (tag_ == Tag::kSome) {
      return value_;
    }
    return std::nullopt;
  }

  explicit constexpr operator std::optional<T>() const noexcept { return to_std(); }

  constexpr bool operator==(const optional& other) const noexcept {
    if (tag_ != other.tag_) {
      return false;
    }
    if (tag_ == Tag::kNone) {
      return true;
    }
    return value_ == other.value_;
  }

  constexpr auto operator<=>(const optional& other) const noexcept {
    if (auto cmp = tag_ <=> other.tag_; cmp != 0) {
      return cmp;
    }
    if (tag_ == Tag::kNone) {
      return std::strong_ordering::equal;
    }
    return value_ <=> other.value_;
  }

 private:
  enum class Tag : uint64_t {
    kNone = 0,
    kSome = 1,
  };

  Tag tag_ = Tag::kNone;
  union {
    struct {
    } empty_{};
    T value_;
  };
};

}  // namespace stdbind

#endif  // SRC_LIB_STDBIND_INCLUDE_LIB_STDBIND_OPTIONAL_H_
