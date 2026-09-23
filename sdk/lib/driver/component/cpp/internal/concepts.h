// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_INTERNAL_CONCEPTS_H_
#define LIB_DRIVER_COMPONENT_CPP_INTERNAL_CONCEPTS_H_

#include <fidl/fuchsia.driver.framework/cpp/driver/fidl.h>

#include <type_traits>

namespace fdf {
class DriverBase;
}

namespace fdf_internal {

// A driver must:
// * Derive from fdf::DriverBase
// * Not be abstract (consider marking your driver as final)
// * Implement a constructor that takes in the arguments (DriverStartArgs,
//   fdf::UnownedSynchronizedDispatcher)
template <typename T>
concept IsDriver = std::is_base_of_v<fdf::DriverBase, T> && !std::is_abstract_v<T> &&
                   std::is_constructible_v<T, fuchsia_driver_framework::DriverStartArgs,
                                           fdf::UnownedSynchronizedDispatcher>;
}  // namespace fdf_internal

#endif  // LIB_DRIVER_COMPONENT_CPP_INTERNAL_CONCEPTS_H_
