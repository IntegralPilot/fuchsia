// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_OUTGOING_CPP_STRUCTURED_CONFIG_H_
#define LIB_COMPONENT_OUTGOING_CPP_STRUCTURED_CONFIG_H_

#include <type_traits>

// These traits can be used to ensure that a given template argument
// is a structured config type.
namespace component {

// This is for regular elf components where the config comes from startup handles.
template <typename T, typename = void>
struct IsStructuredConfig : public ::std::false_type {};
template <typename T>
struct IsStructuredConfig<T, std::void_t<decltype(T::kIsStructuredConfig)>>
    : public std::true_type {};

template <typename T>
constexpr inline auto IsStructuredConfigV = IsStructuredConfig<T>::value;

}  // namespace component

#endif  // LIB_COMPONENT_OUTGOING_CPP_STRUCTURED_CONFIG_H_
