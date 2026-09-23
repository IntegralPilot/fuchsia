// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_DFV2_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_DFV2_H_

#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include "src/graphics/display/lib/driver-framework-migration-utils/namespace/namespace.h"

namespace display {

class NamespaceDfv2 final : public Namespace {
 public:
  // Factory method used for production.
  //
  // `fdf_namespace` must be non-null and must outlive `NamespaceDfv2`.
  static zx::result<std::unique_ptr<Namespace>> Create(fdf::Namespace* fdf_namespace);

  // Production code should prefer the `Create()` factory method.
  //
  // `fdf_namespace` must be non-null and must outlive `NamespaceDfv2`.
  explicit NamespaceDfv2(fdf::Namespace* fdf_namespace);

  ~NamespaceDfv2() override = default;

  NamespaceDfv2(const NamespaceDfv2&) = delete;
  NamespaceDfv2(NamespaceDfv2&&) = delete;
  NamespaceDfv2& operator=(const NamespaceDfv2&) = delete;
  NamespaceDfv2& operator=(NamespaceDfv2&&) = delete;

 private:
  // implements `Namespace`.
  zx::result<> ConnectServerEndToFidlProtocol(zx::channel server_end, std::string_view service,
                                              std::string_view service_member,
                                              std::string_view instance) const override;

  fdf::Namespace& fdf_namespace_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_DFV2_H_
