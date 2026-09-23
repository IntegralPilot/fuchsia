// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/internal/symbols.h>
#include <lib/driver/devicetree/visitors/default/default.h>
#include <lib/driver/devicetree/visitors/load-visitors.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <memory>
#include <string_view>
#include <utility>

namespace fdf_devicetree {

zx::result<std::unique_ptr<VisitorRegistry>> LoadVisitors(
    const std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>>& symbols) {
  if (!symbols.has_value()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto visitors = std::make_unique<VisitorRegistry>();

  auto status = visitors->RegisterVisitor(std::make_unique<DefaultVisitors<>>());
  if (status.is_error()) {
    fdf::error("DefaultVisitors registration failed: {}", status.status_string());
    return status.take_error();
  }

  status = GlobalVisitorRegistry::Instance().RegisterAll(*visitors);
  if (status.is_error()) {
    fdf::error("Failed to register static visitors: {}", status.status_string());
    return status.take_error();
  }

  std::unordered_set modules = fdf_internal::GetModules(symbols);

  for (const auto& module_name : modules) {
    auto registration = fdf_internal::GetSymbol<const VisitorRegistration*>(
        symbols, module_name, "__devicetree_visitor_registration__");
    if (registration == nullptr) {
      fdf::error("Symbol __devicetree_visitor_registration__ not found in visitor: '{}'",
                 module_name);
      continue;
    }

    auto visitor = registration->v1.create_visitor(fdf::Logger::GlobalInstance());
    if (!visitor) {
      fdf::error("visitor '{}' creation failed", module_name);
      continue;
    }

    status = visitors->RegisterVisitor(std::move(visitor));
    if (status.is_error()) {
      fdf::error("visitor '{}' registration failed: {}", module_name, status.status_string());
      continue;
    }

    fdf::debug("visitor '{}' registered", module_name);
  }
  return zx::ok(std::move(visitors));
}

}  // namespace fdf_devicetree
