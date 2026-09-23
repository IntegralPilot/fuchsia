// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_NODE_CPP_PROVIDE_RESOURCE_H_
#define LIB_DRIVER_NODE_CPP_PROVIDE_RESOURCE_H_

#include <fidl/fuchsia.driver.framework/cpp/markers.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/stdcompat/span.h>

namespace fdf {

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
// Provides a resource under the given |parent|. Other nodes can depend on this resource.
//
// This is a synchronous call and requires that the dispatcher allow sync calls.
zx::result<fidl::ClientEnd<fuchsia_driver_framework::ResourceController>> ProvideResource(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers);
#endif

}  // namespace fdf

#endif  // LIB_DRIVER_NODE_CPP_PROVIDE_RESOURCE_H_
