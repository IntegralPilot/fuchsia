// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/visitors/registration.h>

namespace fdf_devicetree {

GlobalVisitorRegistry& GlobalVisitorRegistry::Instance() {
  static GlobalVisitorRegistry instance;
  return instance;
}

void GlobalVisitorRegistry::Register(std::unique_ptr<Visitor> visitor) {
  visitors_.push_back(std::move(visitor));
}

std::vector<std::unique_ptr<Visitor>> GlobalVisitorRegistry::TakeVisitors() {
  return std::move(visitors_);
}

zx::result<> GlobalVisitorRegistry::RegisterAll(VisitorRegistry& registry) {
  for (auto& visitor : visitors_) {
    if (zx::result<> status = registry.RegisterVisitor(std::move(visitor)); status.is_error()) {
      return status;
    }
  }
  visitors_.clear();
  return zx::ok();
}

}  // namespace fdf_devicetree
