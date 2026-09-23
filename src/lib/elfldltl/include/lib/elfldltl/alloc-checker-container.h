// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ALLOC_CHECKER_CONTAINER_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ALLOC_CHECKER_CONTAINER_H_

#include <string_view>

#include <fbl/alloc_checker.h>

namespace elfldltl {

// Similar to elfldltl::StdContainer (see container.h), except
// AllocCheckerContainer leverages fbl::AllocChecker to check the allocations
// performed by the underlying type method. The string_view error parameter
// should contain a description of the allocation and is included in the
// diagnostics error message. The boolean return value indicates allocation
// success or failure. If allocation fails the Diagnostic's object's OutofMemory
// error is called.
template <template <typename, typename...> class C, typename... P>
struct AllocCheckerContainer {
  template <typename T>
  class Container : public C<T, P...> {
   public:
    using Base = C<T, P...>;

    using Base::Base;
    using typename Base::size_type;
    using typename Base::value_type;

    // Callers may use the base class methods with this container instance.
    using Base::insert;
    using Base::push_back;
    using Base::reserve;
    using Base::resize;

    constexpr Container(Container&&) noexcept = default;

    constexpr Container& operator=(Container&&) noexcept = default;

    template <class Diagnostics, typename U>
    bool push_back(Diagnostics& diagnostics, std::string_view error, U&& value) {
      fbl::AllocChecker ac;
      Base::push_back(std::forward<U>(value), &ac);
      if (!ac.check()) {
        diagnostics.OutOfMemory(error, sizeof(value_type));
        return false;
      }
      return true;
    }

    template <class Diagnostics, typename U>
    bool insert(Diagnostics& diagnostics, std::string_view error, size_type index, U&& value) {
      fbl::AllocChecker ac;
      Base::insert(index, std::forward<U>(value), &ac);
      if (!ac.check()) {
        diagnostics.OutOfMemory(error, sizeof(value_type));
        return false;
      }
      return true;
    }

    template <class Diagnostics>
    bool reserve(Diagnostics& diagnostics, std::string_view error, size_type capacity) {
      fbl::AllocChecker ac;
      Base::reserve(capacity, &ac);
      if (!ac.check()) {
        diagnostics.OutOfMemory(error, capacity * sizeof(value_type));
        return false;
      }
      return true;
    }

    template <class Diagnostics>
    bool resize(Diagnostics& diagnostics, std::string_view error, size_type size) {
      fbl::AllocChecker ac;
      Base::resize(size, &ac);
      if (!ac.check()) {
        diagnostics.OutOfMemory(error, size * sizeof(value_type));
        return false;
      }
      return true;
    }
  };
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ALLOC_CHECKER_CONTAINER_H_
