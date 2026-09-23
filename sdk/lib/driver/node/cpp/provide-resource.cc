// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/natural_messaging.h>
#include <lib/driver/node/cpp/provide-resource.h>

namespace fdf {

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
zx::result<fidl::ClientEnd<fuchsia_driver_framework::ResourceController>> ProvideResource(
    fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent, fdf::Logger& logger,
    std::string_view node_name,
    cpp20::span<const fuchsia_driver_framework::NodeProperty2> properties,
    cpp20::span<const fuchsia_driver_framework::Offer> offers) {
  auto [resource_controller_client_end, resource_controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::ResourceController>::Create();

  std::vector<fuchsia_driver_framework::NodeProperty2> props{properties.begin(), properties.end()};
  std::vector<fuchsia_driver_framework::Offer> offers_vec(offers.begin(), offers.end());

  fuchsia_driver_framework::ResourceArgs args{{
      .name = std::string(node_name),
      .properties = std::move(props),
      .offers = std::move(offers_vec),
  }};

  fidl::Result<fuchsia_driver_framework::Node::ProvideResource> result =
      fidl::Call(parent)->ProvideResource(
          {std::move(args), std::move(resource_controller_server_end)});

  if (result.is_error()) {
    logger.log(fdf::LogSeverity::ERROR, "Failed to provide resource {}. Error: {}", node_name,
               result.error_value().FormatDescription());
    return zx::error(result.error_value().is_framework_error()
                         ? result.error_value().framework_error().status()
                         : ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(resource_controller_client_end));
}
#endif

}  // namespace fdf
