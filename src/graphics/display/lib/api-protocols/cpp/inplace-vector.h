// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_INPLACE_VECTOR_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_INPLACE_VECTOR_H_

#include <zircon/assert.h>

#include <array>
#include <memory>
#include <span>
#include <utility>

namespace display::internal {

// std::inplace_vector variation suitable for display drivers.
//
// The interface aims to stay as close as possible to std::inplace_vector.
// The following variations are intentional.
//
// * Vectors are non-copyable and non-moveable. These operations are expensive,
//   and must be carried out explicitly by underlying code.
// * Mutations that would invalidate iterators (removing individual elements,
//   inserting in the middle of the vector) are not currently supported. This
//   makes iterator invalidation easier to explain -- iterators remain valid
//   until clear() is called.
template <typename ValueType, int Capacity>
class InplaceVector {
 public:
  using value_type = ValueType;

  using size_type = typename std::span<ValueType>::size_type;
  using difference_type = typename std::span<ValueType>::difference_type;
  using reference = typename std::span<ValueType>::reference;
  using const_reference = typename std::span<const ValueType>::reference;
  using pointer = typename std::span<ValueType>::pointer;
  using const_pointer = typename std::span<const ValueType>::const_pointer;
  using iterator = typename std::span<ValueType>::iterator;
  using const_iterator = typename std::span<const ValueType>::iterator;
  using reverse_iterator = typename std::span<ValueType>::reverse_iterator;
  using const_reverse_iterator = typename std::span<const ValueType>::reverse_iterator;

  // The default constructor creates an empty container.
  constexpr InplaceVector() noexcept = default;

  constexpr InplaceVector(std::initializer_list<ValueType> initializer);

  ~InplaceVector() { clear(); }

  constexpr bool empty() const noexcept { return size_ == 0; }
  constexpr size_type size() const noexcept { return size_; }
  constexpr size_type max_size() const noexcept { return Capacity; }
  constexpr size_type capacity() const noexcept { return Capacity; }

  constexpr ValueType* data() noexcept { return &storage_[0].element; }
  constexpr const ValueType* data() const noexcept { return &storage_[0].element; }

  // span pointng to the vector's elements.
  constexpr std::span<ValueType> Elements() noexcept { return {data(), size()}; }
  constexpr std::span<const ValueType> Elements() const noexcept { return {data(), size()}; }

  constexpr reference operator[](size_type index) { return Elements()[index]; }
  constexpr const_reference operator[](size_type index) const { return Elements()[index]; }

  constexpr reference front() { return Elements().front(); }
  constexpr const_reference front() const { return Elements().front(); }
  constexpr reference back() { return Elements().back(); }
  constexpr const_reference back() const { return Elements().back(); }

  constexpr iterator begin() noexcept { return Elements().begin(); }
  constexpr const_iterator begin() const noexcept { return Elements().begin(); }
  constexpr const_iterator cbegin() const noexcept { return Elements().begin(); }

  constexpr iterator end() noexcept { return Elements().end(); }
  constexpr const_iterator end() const noexcept { return Elements().end(); }
  constexpr const_iterator cend() const noexcept { return Elements().end(); }

  constexpr reverse_iterator rbegin() noexcept { return Elements().rbegin(); }
  constexpr const_reverse_iterator rbegin() const noexcept { return Elements().rbegin(); }
  constexpr const_reverse_iterator crbegin() const noexcept { return Elements().rbegin(); }

  constexpr reverse_iterator rend() noexcept { return Elements().rend(); }
  constexpr const_reverse_iterator rend() const noexcept { return Elements().rend(); }
  constexpr const_reverse_iterator crend() const noexcept { return Elements().rend(); }

  template <class... Args>
  constexpr reference emplace_back(Args&&... args);

  constexpr void push_back(const ValueType& value) { emplace_back(value); }
  constexpr void push_back(ValueType&& value) { emplace_back(std::move(value)); }
  constexpr void clear();

 private:
  union ElementOrEmpty {
    // std::monostate equivalent that doesn't require including <variant>.
    struct Empty {};

    ValueType element;
    Empty empty = {};

    // The container is responsible for destroying union instances.
    ~ElementOrEmpty() noexcept {}
  };
  static_assert(sizeof(ElementOrEmpty) == sizeof(ValueType));

  static void StaticChecks();

  // The first `size_` elements are valid. The remaining elements are invalid.
  std::array<ElementOrEmpty, Capacity> storage_ = {};

  // Takes values in [0, Capacity].
  size_type size_ = 0;
};

template <typename ValueType, int Capacity>
constexpr InplaceVector<ValueType, Capacity>::InplaceVector(
    std::initializer_list<ValueType> initializer) {
  ZX_DEBUG_ASSERT(initializer.size() <= Capacity);

  for (auto& initializer_value : initializer) {
    new (&storage_[size_].element) ValueType(initializer_value);
    ++size_;
  }
}

template <typename ValueType, int Capacity>
template <class... Args>
constexpr typename InplaceVector<ValueType, Capacity>::reference
InplaceVector<ValueType, Capacity>::emplace_back(Args&&... args) {
  ZX_DEBUG_ASSERT(size_ < Capacity);

  new (&storage_[size_].element) ValueType(std::forward<Args>(args)...);
  InplaceVector::reference return_value = storage_[size_].element;
  ++size_;
  return return_value;
}

template <typename ValueType, int Capacity>
constexpr void InplaceVector<ValueType, Capacity>::clear() {
  for (InplaceVector::size_type index = 0; index < size_; ++index) {
    std::destroy_at(&storage_[index].element);
  }
  size_ = 0;
}

}  // namespace display::internal

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_PROTOCOLS_CPP_INPLACE_VECTOR_H_
