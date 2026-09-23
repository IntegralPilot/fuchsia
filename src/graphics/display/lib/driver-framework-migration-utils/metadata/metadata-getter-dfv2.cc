// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/metadata/metadata-getter-dfv2.h"

#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <string_view>

#include <fbl/alloc_checker.h>

namespace display {

// static
zx::result<std::unique_ptr<MetadataGetter>> MetadataGetterDfv2::Create(
    std::shared_ptr<fdf::Namespace> fdf_namespace) {
  ZX_DEBUG_ASSERT(fdf_namespace != nullptr);

  fbl::AllocChecker alloc_checker;
  auto namespace_dfv2 =
      fbl::make_unique_checked<MetadataGetterDfv2>(&alloc_checker, std::move(fdf_namespace));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for MetadataGetterDfv2.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(namespace_dfv2));
}

MetadataGetterDfv2::MetadataGetterDfv2(std::shared_ptr<fdf::Namespace> fdf_namespace)
    : fdf_namespace_(std::move(fdf_namespace)) {
  ZX_DEBUG_ASSERT(fdf_namespace_ != nullptr);
}

zx::result<std::vector<uint8_t>> MetadataGetterDfv2::GetBytes(uint32_t type,
                                                              std::string_view instance) {
  return compat::GetMetadataArray<uint8_t>(fdf_namespace_, type, instance);
}

}  // namespace display
