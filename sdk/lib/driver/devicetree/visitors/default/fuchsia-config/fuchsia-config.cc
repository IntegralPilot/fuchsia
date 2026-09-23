// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/visitors/default/fuchsia-config/fuchsia-config.h"

#include <endian.h>
#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/driver/devicetree/manager/node.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fdf_devicetree {

namespace {

void FlattenProperties(fdf_devicetree::Node& node, const std::string& prefix,
                       std::vector<fuchsia_driver_metadata::DictionaryEntry>& entries) {
  for (const auto& [name, prop_value] : node.properties()) {
    std::string key = prefix.empty() ? std::string(name) : prefix + "." + std::string(name);

    std::optional<fuchsia_driver_metadata::DictionaryValue> value;

    // Try to parse as integers.
    auto bytes = prop_value.AsBytes();
    if (!bytes.empty()) {
      std::vector<int64_t> vec;
      size_t padded_size = (bytes.size() + 3) / 4 * 4;
      std::vector<uint8_t> padded_bytes(padded_size, 0);
      memcpy(padded_bytes.data(), bytes.data(), bytes.size());

      vec.reserve(padded_size / 4);
      for (size_t i = 0; i < padded_size; i += 4) {
        uint32_t val;
        memcpy(&val, padded_bytes.data() + i, 4);
        vec.push_back(be32toh(val));
      }

      if (vec.size() == 1) {
        value = fuchsia_driver_metadata::DictionaryValue::WithInt64(vec[0]);
      } else {
        value = fuchsia_driver_metadata::DictionaryValue::WithInt64Vec(std::move(vec));
      }
    } else {
      // Fallback or empty property.
      value = fuchsia_driver_metadata::DictionaryValue::WithInt64Vec({});
    }

    entries.push_back(fuchsia_driver_metadata::DictionaryEntry{{
        .key = std::move(key),
        .value = std::move(*value),
    }});
  }

  for (auto& child : node.children()) {
    FlattenProperties(
        *child.GetNode(),
        prefix.empty() ? std::string(child.name()) : prefix + "." + std::string(child.name()),
        entries);
  }
}

}  // namespace

zx::result<> FuchsiaConfigVisitor::Visit(fdf_devicetree::Node& node,
                                         const devicetree::PropertyDecoder& decoder) {
  for (auto& child : node.children()) {
    if (child.name() == "fuchsia,config") {
      FDF_LOG(DEBUG, "Found fuchsia,config child in node '%s'", node.name().c_str());

      fuchsia_driver_metadata::Dictionary dictionary;
      std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;

      FlattenProperties(*child.GetNode(), "", entries);

      dictionary.entries() = std::move(entries);

      auto persisted = fidl::Persist(dictionary);
      if (!persisted.is_ok()) {
        FDF_LOG(ERROR, "Failed to persist dictionary metadata: %s",
                persisted.error_value().FormatDescription().c_str());
        return zx::error(persisted.error_value().status());
      }

      fuchsia_hardware_platform_bus::Metadata metadata;
      metadata.id() = "fuchsia.driver.metadata.Dictionary";
      metadata.data() = std::move(persisted.value());

      node.AddMetadata(std::move(metadata));
      FDF_LOG(INFO, "Added dictionary metadata to node '%s'", node.name().c_str());
    }
  }
  return zx::ok();
}

}  // namespace fdf_devicetree

REGISTER_DEVICETREE_VISITOR(fdf_devicetree::FuchsiaConfigVisitor);
