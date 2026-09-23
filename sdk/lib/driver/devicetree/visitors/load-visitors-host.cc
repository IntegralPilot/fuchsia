// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <lib/driver/devicetree/visitors/load-visitors-host.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fdf_devicetree {

zx::result<> LoadVisitorsHost(VisitorRegistry& registry,
                              const std::vector<std::string>& library_paths) {
  for (const auto& path : library_paths) {
    void* handle = dlopen(path.c_str(), RTLD_NOW);
    if (!handle) {
      char* error = dlerror();
      fdf::error("Failed to dlopen visitor '{}': {}", path, (error ? error : "unknown error"));
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto registration = reinterpret_cast<const VisitorRegistration*>(
        dlsym(handle, "__devicetree_visitor_registration__"));
    if (!registration) {
      fdf::error("Symbol __devicetree_visitor_registration__ not found in visitor: '{}'", path);
      return zx::error(ZX_ERR_NOT_FOUND);
    }

    if (registration->version != VISITOR_REGISTRATION_VERSION_1) {
      fdf::error("Unsupported visitor registration version: {} in '{}'", registration->version,
                 path);
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }

    auto visitor = registration->v1.create_visitor(fdf::Logger::GlobalInstance());
    if (!visitor) {
      fdf::error("Visitor creation failed for: '{}'", path);
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto status = registry.RegisterVisitor(std::move(visitor));
    if (status.is_error()) {
      fdf::error("Visitor registration failed for '{}': {}", path, status.status_value());
      return status.take_error();
    }
  }
  return GlobalVisitorRegistry::Instance().RegisterAll(registry);
}

}  // namespace fdf_devicetree
