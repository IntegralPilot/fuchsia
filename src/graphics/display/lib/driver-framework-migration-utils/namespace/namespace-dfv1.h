// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_DFV1_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_DFV1_H_

#include <lib/ddk/device.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include "src/graphics/display/lib/driver-framework-migration-utils/namespace/namespace.h"

namespace display {

class NamespaceDfv1 final : public Namespace {
 public:
  // Factory method used for production.
  //
  // `device` must be non-null.
  static zx::result<std::unique_ptr<Namespace>> Create(zx_device_t* device);

  // Production code should prefer the `Create()` factory method.
  //
  // `device` must be non-null.
  explicit NamespaceDfv1(zx_device_t* device);

  ~NamespaceDfv1() override = default;

  NamespaceDfv1(const NamespaceDfv1&) = delete;
  NamespaceDfv1(NamespaceDfv1&&) = delete;
  NamespaceDfv1& operator=(const NamespaceDfv1&) = delete;
  NamespaceDfv1& operator=(NamespaceDfv1&&) = delete;

 private:
  // implements `Namespace`.
  zx::result<> ConnectServerEndToFidlProtocol(zx::channel server_end, std::string_view service,
                                              std::string_view service_member,
                                              std::string_view instance) const override;

  zx_device_t* const device_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_NAMESPACE_NAMESPACE_DFV1_H_
