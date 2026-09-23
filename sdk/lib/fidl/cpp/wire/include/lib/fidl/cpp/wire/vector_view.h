// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_VECTOR_VIEW_H_
#define LIB_FIDL_CPP_WIRE_VECTOR_VIEW_H_

#include <lib/fidl/cpp/wire/arena.h>
#include <lib/stdcompat/span.h>
#include <zircon/availability.h>
#include <zircon/fidl.h>

#include <algorithm>
#include <iterator>
#include <type_traits>
#include <vector>

namespace {
class LayoutChecker;
}  // namespace

namespace fidl {

// VectorView is the representation of a FIDL vector in wire domain objects.
//
// VectorViews provide limited functionality to access and set fields of the
// vector and other methods like fidl::Arena, std::array, or std::vector must be
// used to construct it.
//
// VectorView instances can be passed by value, as copying is cheap.
//
// VectorView's layout and data format must match fidl_vector_t as it will be
// reinterpret_casted into/from fidl_vector_t during encoding and decoding.
//
// Example:
//
//     uint32_t arr[5] = { 1, 2, 3 };
//     fuchsia_some_lib::wire::SomeFidlObject obj;
//     // Sets the field to a vector view borrowing from |arr|.
//     obj.set_vec_field(fidl::VectorView<uint32_t>::FromExternal(arr));
//
template <typename T>
class VectorView {
 private:
  template <typename>
  friend class VectorView;

 public:
  using elem_type = T;
  using value_type = T;

  constexpr VectorView() = default;

  // Allocates a vector using an arena. |T| is default constructed.
  VectorView(AnyArena& allocator, size_t size)
      : size_(size), data_(allocator.AllocateVector<T>(size)) {}
  VectorView(AnyArena& allocator, size_t initial_size, size_t capacity)
      : size_(initial_size), data_(allocator.AllocateVector<T>(capacity)) {
    ZX_DEBUG_ASSERT(initial_size <= capacity);
  }
  constexpr VectorView(std::nullptr_t data, size_t size) {}

  // Allocates a vector using an arena and copies the data from the supplied iterators.
  // The iterator must satisfy the random_access_iterator concept.
  //
  // Example:
  //
  //     fidl::Arena arena;
  //     std::vector<int32_t> vec(...);
  //     // Copy contents of |vec| into |arena|, and return a view of the copies content.
  //     fidl::VectorView<int32_t> vv(arena, vec.begin(), vec.end());
  //
  template <typename InputIterator>
  VectorView(AnyArena& arena, InputIterator first, InputIterator last)
      : size_(last - first), data_(arena.AllocateVector<T>(size_)) {
    using Traits = std::iterator_traits<InputIterator>;
    constexpr bool kIsIterator = has_difference_type<Traits>::value;
    static_assert(
        kIsIterator,
        "|InputIterator| is not an iterator. "
        "Ensure that the last two arguments to this constructor are random access iterators.");
    std::copy(first, last, data_);
  }

  // Allocates a vector using an arena and copies the data from the supplied |span|.
  VectorView(AnyArena& arena, cpp20::span<const T> span)
      : VectorView(arena, span.begin(), span.end()) {}

  // Allocates a vector using an arena and copies the data from the supplied |std::vector|.
  VectorView(AnyArena& arena, const std::vector<std::remove_cv_t<T>>& vector)
      : VectorView(arena, cpp20::span(vector)) {}

  constexpr VectorView(const VectorView&) = default;
  constexpr VectorView& operator=(const VectorView&) = default;

  template <typename _ = std::enable_if<!std::is_same_v<T, std::remove_cv_t<T>>>>
  // NOLINTNEXTLINE(google-explicit-constructor) Intentionally implicit
  constexpr VectorView(const VectorView<std::remove_cv_t<T>>& other) noexcept
      : size_(other.size_), data_(other.data_) {}

  template <typename _ = std::enable_if<!std::is_same_v<T, std::remove_cv_t<T>>>>
  // NOLINTNEXTLINE(google-explicit-constructor) Intentionally implicit
  constexpr VectorView& operator=(const VectorView<std::remove_cv_t<T>>& other) noexcept {
    size_ = other.size_;
    data_ = other.data_;
    return *this;
  }

  // Constructs a fidl::VectorView by unsafely borrowing other sequences.
  //
  // |FromExternal| methods are the only way to reference data which is not
  // managed by an arena. Their usage is discouraged. The lifetime of the
  // referenced vector must be longer than the lifetime of the created
  // |VectorView|.
  //
  // For example:
  //
  //     std::vector<int32_t> my_vector = { 1, 2, 3 };
  //     auto my_view =
  //         fidl::VectorView<int32_t>::FromExternal(my_vector);
  //
  static constexpr VectorView<T> FromExternal(std::vector<std::remove_cv_t<T>>& from) {
    return VectorView<T>(from);
  }
  template <size_t size>
  static constexpr VectorView<T> FromExternal(std::array<T, size>& from) {
    return VectorView<T>(from.data(), size);
  }
  template <size_t size>
  static constexpr VectorView<T> FromExternal(T (&data)[size]) {
    return VectorView<T>(data, size);
  }
  static VectorView<T> constexpr FromExternal(T* data, size_t size) {
    return VectorView<T>(data, size);
  }

  constexpr cpp20::span<T> get() const { return {data(), size()}; }

  constexpr size_t size() const { return size_; }

  // TODO(https://fxbug.dev/430121763): Migrate away from this setter.
  constexpr void set_size(size_t size) { size_ = size; }

  // Deprecated in favor of `size()`.
  //
  // The Banjo convention was to use `count()` to express quantities of
  // elements, and use `size()` to express quantities of bytes. This method
  // facilitates migrating from Banjo to FIDL.
  ZX_REMOVED_SINCE(1, 28, HEAD, "Use size()")
  constexpr size_t count() const { return size(); }

  // Deprecated in favor of `set_size()`. See `count()` for historical context.
  ZX_REMOVED_SINCE(1, 28, HEAD, "Use set_size()")
  void set_count(size_t size) { set_size(size); }

  constexpr T* data() const { return data_; }

  // Returns if the vector view is empty.
  constexpr bool empty() const { return size() == 0; }

  // TODO(https://fxbug.dev/42061094): |is_null| is used to check if an optional view type
  // is absent. This can be removed if optional view types switch to
  // |fidl::WireOptional|.
  bool is_null() const { return data() == nullptr; }

  T& at(size_t offset) const { return data()[offset]; }
  T& operator[](size_t offset) const { return at(offset); }

  constexpr T* begin() const { return data(); }
  constexpr const T* cbegin() const { return data(); }

  constexpr T* end() const { return data() + size(); }
  constexpr const T* cend() const { return data() + size(); }

  // Allocates |size| items of |T| from the |arena|, forgetting any values
  // currently held by the vector view. |T| is default constructed.
  void Allocate(AnyArena& arena, size_t size) {
    size_ = size;
    data_ = arena.AllocateVector<T>(size);
  }

 protected:
  constexpr explicit VectorView(std::vector<std::remove_cv_t<T>>& from)
      : size_(from.size()), data_(const_cast<T*>(from.data())) {}
  constexpr explicit VectorView(T* data, size_t size) : size_(size), data_(data) {}

 private:
  template <class I>
  struct has_difference_type {
    template <class U>
    static std::false_type test(...);
    template <class U>
    static std::true_type test(std::void_t<typename U::difference_type>* = 0);
    static const bool value = decltype(test<I>(0))::value;
  };

  friend ::LayoutChecker;
  size_t size_ = 0;
  T* data_ = nullptr;
};

template <typename T>
VectorView(fidl::AnyArena&, cpp20::span<T>) -> VectorView<T>;

template <typename T>
VectorView(fidl::AnyArena&, const std::vector<T>&) -> VectorView<T>;

}  // namespace fidl

namespace {
class LayoutChecker {
  static_assert(sizeof(fidl::VectorView<uint8_t>) == sizeof(fidl_vector_t),
                "VectorView size should match fidl_vector_t");
  static_assert(offsetof(fidl::VectorView<uint8_t>, size_) == offsetof(fidl_vector_t, count),
                "VectorView size offset should match fidl_vector_t");
  static_assert(sizeof(fidl::VectorView<uint8_t>::size_) == sizeof(fidl_vector_t::count),
                "VectorView size size should match fidl_vector_t");
  static_assert(offsetof(fidl::VectorView<uint8_t>, data_) == offsetof(fidl_vector_t, data),
                "VectorView data offset should match fidl_vector_t");
  static_assert(sizeof(fidl::VectorView<uint8_t>::data_) == sizeof(fidl_vector_t::data),
                "VectorView data size should match fidl_vector_t");
};

}  // namespace

#endif  // LIB_FIDL_CPP_WIRE_VECTOR_VIEW_H_
