// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_H_

#include <lib/fidl/cpp/wire/traits.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>
#include <span>
#include <string_view>
#include <type_traits>
#include <vector>

#include <fbl/alloc_checker.h>

namespace display {

// Gets driver metadata from the driver framework.
//
// TODO(https://fxbug.dev/323061435): This class should be only used when the
// display drivers are being migrated to DFv2. Drivers should replace
// `MetadataGetter` with `compat::GetMetadata()` when the migration is done.
class MetadataGetter {
 public:
  MetadataGetter() = default;
  virtual ~MetadataGetter() = default;

  MetadataGetter(const MetadataGetter&) = delete;
  MetadataGetter(MetadataGetter&&) = delete;
  MetadataGetter& operator=(const MetadataGetter&) = delete;
  MetadataGetter& operator=(MetadataGetter&&) = delete;

  // Equivalent to `compat::GetMetadata(incoming, type, instance)`.
  //
  // `ReturnType` must be a trivial type and must not be a FIDL object.
  //
  // Returns ZX_ERR_INTERNAL if the size of the metadata bytes provided by the
  // Driver Framework doesn't match the size of `ReturnType`.
  template <typename ReturnType>
  zx::result<std::unique_ptr<ReturnType>> Get(uint32_t type, std::string_view instance);

 private:
  // Gets the raw bytes of metadata of `type` provided by the Driver Framework.
  virtual zx::result<std::vector<uint8_t>> GetBytes(uint32_t type, std::string_view instance) = 0;
};

template <typename ReturnType>
zx::result<std::unique_ptr<ReturnType>> MetadataGetter::Get(uint32_t type,
                                                            std::string_view instance) {
  static_assert(!fidl::IsFidlObject<ReturnType>::value, "Return type must not be a FIDL object");
  static_assert(std::is_trivial_v<ReturnType>, "ReturnType must be a trivial type.");

  zx::result<std::vector<uint8_t>> bytes_result = GetBytes(type, instance);
  if (bytes_result.is_error()) {
    return bytes_result.take_error();
  }
  std::vector<uint8_t> bytes = std::move(bytes_result).value();
  if (bytes.size() != sizeof(ReturnType)) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  fbl::AllocChecker alloc_checker;
  auto metadata = fbl::make_unique_checked<ReturnType>(&alloc_checker);
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  std::span<uint8_t> metadata_bytes(reinterpret_cast<uint8_t*>(metadata.get()), sizeof(ReturnType));
  std::copy(bytes.begin(), bytes.end(), metadata_bytes.begin());
  return zx::ok(std::move(metadata));
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_METADATA_METADATA_GETTER_H_
