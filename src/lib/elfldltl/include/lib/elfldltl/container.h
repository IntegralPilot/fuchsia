// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_CONTAINER_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_CONTAINER_H_

#include <optional>
#include <string_view>
#include <type_traits>
#include <vector>

// This file provides some adapters for container types defined elsewhere to
// be used with the diagnostics.h API for handling allocation failures.  These
// represent the container API expected by other elfldltl template code.
//
// elfldltl template code that needs containers uses a template template
// parameter (template<typename> class Container) to instantiate any
// Container<T> types it needs.  These are used like normal containers, except
// that the methods that can need to allocate (push_back, emplace_back,
// emplace, and insert) take additional Diagnostics& and std::string_view
// parameters first.  This parameter is a requirement for the Container API, but
// is not used by the StdContainer. The methods that usually return void
// (push_back, emplace_back) instead return an std::true_type to always indicate
// success. The methods that usually return an iterator (emplace, insert)
// wraps the underlying method's return value with an std::optional<...>.

namespace elfldltl {

// elfldltl::StdContainer<C, ...>::Container<T> just uses a standard container
// type C<T, ...>, e.g. elfldltl::StdContainer<std::vector>.
template <template <typename, typename...> class C, typename... P>
struct StdContainer {
  template <typename T>
  class Container : public C<T, P...> {
   public:
    using Base = C<T, P...>;

    static_assert(std::is_move_constructible_v<T> || std::is_copy_constructible_v<T>);
    static_assert(std::is_move_constructible_v<Base>);

    using Base::Base;
    using typename Base::size_type;

    constexpr Container(Container&&) noexcept = default;

    constexpr Container& operator=(Container&&) noexcept = default;

    template <class Diagnostics, typename U>
    constexpr std::true_type push_back(Diagnostics& diagnostics, std::string_view error,
                                       U&& value) {
      Base::push_back(std::forward<U>(value));
      return {};
    }

    template <class Diagnostics, typename... Args>
    constexpr std::true_type emplace_back(Diagnostics& diagnostics, std::string_view error,
                                          Args&&... args) {
      Base::emplace_back(std::forward<Args>(args)...);
      return {};
    }

    template <class Diagnostics, typename... Args>
    constexpr auto emplace(Diagnostics& diagnostics, std::string_view error, Args&&... args) {
      return std::make_optional(Base::emplace(std::forward<Args>(args)...));
    }

    template <class Diagnostics, typename... Args>
    constexpr auto insert(Diagnostics& diagnostics, std::string_view error, Args&&... args) {
      return std::make_optional(Base::insert(std::forward<Args>(args)...));
    }

    template <class Diagnostics>
    constexpr std::true_type reserve(Diagnostics& diagnostics, std::string_view error,
                                     size_type size) {
      Base::reserve(size);
      return {};
    }

    template <class Diagnostics>
    constexpr std::true_type resize(Diagnostics& diagnostics, std::string_view error,
                                    size_type size) {
      Base::resize(size);
      return {};
    }

   private:
    // Make the original methods unavailable.
    using Base::emplace;
    using Base::emplace_back;
    using Base::insert;
    using Base::push_back;
    using Base::reserve;
    using Base::resize;
  };
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_CONTAINER_H_
