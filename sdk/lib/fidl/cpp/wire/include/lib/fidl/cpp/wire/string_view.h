// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_STRING_VIEW_H_
#define LIB_FIDL_CPP_WIRE_STRING_VIEW_H_

#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fidl/walker.h>
#include <zircon/fidl.h>

#include <cstring>
#include <string_view>
#include <type_traits>
#include <utility>

namespace fidl {

// A FIDL string that borrows its contents.
//
// StringView instances can be passed by value, as copying is cheap.
class StringView final : private VectorView<const char> {
 public:
  constexpr StringView() noexcept = default;

  constexpr explicit StringView(const VectorView<char>& vector_view) : VectorView(vector_view) {}
  constexpr explicit StringView(const VectorView<const char>& vector_view)
      : VectorView(vector_view) {}

  // Allocates a string using an arena.
  StringView(AnyArena& allocator, std::string_view from) : VectorView(allocator, from.size()) {
    std::memcpy(const_cast<char*>(VectorView::data()), from.data(), from.size());
  }

  // Constructs a fidl::StringView referencing a string literal. For example:
  //
  //     fidl::StringView view("hello");
  //     view.size() == 5;
  //
  // |literal| must have static storage duration. If you need to borrow from a
  // char array, use |fidl::StringView::FromExternal|.
  template <
      size_t N, typename T,
      typename = std::enable_if_t<std::is_const_v<T> && std::is_same_v<std::remove_cv_t<T>, char>>>
  // Disable `explicit` requirements because we want implicit conversions from string literals.
  // NOLINTNEXTLINE
  constexpr StringView(T (&literal)[N]) : VectorView(static_cast<const char*>(literal), N - 1) {
    static_assert(N > 0, "String should not be empty");
  }

  // Constructs a fidl::StringView by unsafely borrowing other strings.
  //
  // These methods are the only way to reference data which is not managed by an |Arena|.
  // Their usage is discouraged. The lifetime of the referenced string must be longer than the
  // lifetime of the created StringView.
  //
  // For example:
  //
  //     std::string foo = path + "/foo";
  //     auto foo_view = fidl::StringView::FromExternal(foo);
  //
  //     char name[kLength];
  //     snprintf(name, sizeof(name), "Hello %d", 123);
  //     auto name_view = fidl::StringView::FromExternal(name);
  //
  static constexpr StringView FromExternal(std::string_view from) { return StringView(from); }
  static constexpr StringView FromExternal(const char* data, size_t size) {
    return StringView(data, size);
  }

  void Set(AnyArena& allocator, std::string_view from) {
    Allocate(allocator, from.size());
    memcpy(const_cast<char*>(VectorView::data()), from.data(), from.size());
  }

  constexpr std::string_view get() const { return {data(), size()}; }

  using VectorView::data;
  using VectorView::empty;
  using VectorView::set_size;
  using VectorView::size;

  using VectorView::at;
  using VectorView::operator[];
  using VectorView::begin;
  using VectorView::cbegin;
  using VectorView::cend;
  using VectorView::end;

  // TODO(https://fxbug.dev/42061094): |is_null| is used to check if an optional view type
  // is absent. This can be removed if optional view types switch to
  // |fidl::WireOptional|.
  using VectorView::is_null;

 private:
  explicit constexpr StringView(std::string_view from) : VectorView(from.data(), from.size()) {}
  explicit constexpr StringView(const char* data, uint64_t size) : VectorView(data, size) {}
};

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_STRING_VIEW_H_
