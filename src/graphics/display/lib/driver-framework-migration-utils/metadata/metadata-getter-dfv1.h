// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_DFV1_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_DFV1_H_

#include <lib/ddk/device.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include "src/graphics/display/lib/driver-framework-migration-utils/metadata/metadata-getter.h"

namespace display {

class MetadataGetterDfv1 final : public MetadataGetter {
 public:
  // Factory method used for production.
  //
  // `device` must be non-null.
  static zx::result<std::unique_ptr<MetadataGetter>> Create(zx_device_t* device);

  // Prefer the `Create()` factory method.
  //
  // `device` must be non-null.
  explicit MetadataGetterDfv1(zx_device_t* device);

  ~MetadataGetterDfv1() override = default;

  MetadataGetterDfv1(const MetadataGetterDfv1&) = delete;
  MetadataGetterDfv1(MetadataGetterDfv1&&) = delete;
  MetadataGetterDfv1& operator=(const MetadataGetterDfv1&) = delete;
  MetadataGetterDfv1& operator=(MetadataGetterDfv1&&) = delete;

 private:
  // MetadataGetter:
  zx::result<std::vector<uint8_t>> GetBytes(uint32_t type, std::string_view instance) override;

  // Guaranteed to be non-null.
  zx_device_t* const device_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_DFV1_H_
