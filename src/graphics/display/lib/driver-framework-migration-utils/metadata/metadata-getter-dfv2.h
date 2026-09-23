// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_DFV2_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_DFV2_H_

#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include "src/graphics/display/lib/driver-framework-migration-utils/metadata/metadata-getter.h"

namespace display {

class MetadataGetterDfv2 final : public MetadataGetter {
 public:
  // Factory method used for production.
  //
  // `fdf_namespace` must not be null.
  static zx::result<std::unique_ptr<MetadataGetter>> Create(
      std::shared_ptr<fdf::Namespace> fdf_namespace);

  // Prefer the `Create()` factory method.
  //
  // `fdf_namespace` must be non-null and must outlive `MetadataGetterDfv2`.
  explicit MetadataGetterDfv2(std::shared_ptr<fdf::Namespace> fdf_namespace);

  ~MetadataGetterDfv2() override = default;

  MetadataGetterDfv2(const MetadataGetterDfv2&) = delete;
  MetadataGetterDfv2(MetadataGetterDfv2&&) = delete;
  MetadataGetterDfv2& operator=(const MetadataGetterDfv2&) = delete;
  MetadataGetterDfv2& operator=(MetadataGetterDfv2&&) = delete;

 private:
  // MetadataGetter:
  zx::result<std::vector<uint8_t>> GetBytes(uint32_t type, std::string_view instance) override;

  const std::shared_ptr<fdf::Namespace> fdf_namespace_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_DFV2_H_
