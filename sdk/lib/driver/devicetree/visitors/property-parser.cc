// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/property-parser.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

#include <cstdint>
#include <optional>

namespace fdf_devicetree {

zx::result<ParsedProperties> PropertyParser::Parse(Node& node) {
  std::map<PropertyName, std::any> all_values;

  for (auto& property : properties_) {
    auto status = property->Parse(node, all_values);
    if (status.is_error()) {
      if (status.status_value() == ZX_ERR_NOT_FOUND) {
        if (property->required()) {
          fdf::error("Node '{}' does not include the required property '{}'", node.name(),
                     property->name());

          return status.take_error();
        }
        continue;
      }
      fdf::error("Failed to parse property '{}' for node '{}' - {}", property->name(), node.name(),
                 status.status_value());

      return status.take_error();
    }
  }
  return zx::ok(ParsedProperties(std::move(all_values)));
}

void ParsedProperties::AddProperty(const PropertyName& name, std::any value) {
  auto [it, inserted] = properties_.emplace(name, std::move(value));
  if (!inserted) {
    fdf::warn("Property '{}' is being overwritten.", name);

    it->second = std::move(value);
  }
}

zx::result<> BoolProperty::Parse(Node& node, std::map<PropertyName, std::any>& values) const {
  if (node.GetProperty<bool>(name())) {
    values[name()] = true;
    return zx::ok();
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<> Uint32Property::Parse(Node& node, std::map<PropertyName, std::any>& values) const {
  auto property = node.GetProperty<uint32_t>(name());
  if (property.is_error()) {
    return property.take_error();
  }
  values[name()] = *property;
  return zx::ok();
}

zx::result<> Uint64Property::Parse(Node& node, std::map<PropertyName, std::any>& values) const {
  auto property = node.GetProperty<uint64_t>(name());
  if (property.is_error()) {
    return property.take_error();
  }
  values[name()] = *property;
  return zx::ok();
}

zx::result<> StringProperty::Parse(Node& node, std::map<PropertyName, std::any>& values) const {
  auto property = node.GetProperty<std::string>(name());
  if (property.is_error()) {
    return property.take_error();
  }
  values[name()] = *property;
  return zx::ok();
}

zx::result<> Uint32ArrayProperty::Parse(Node& node,
                                        std::map<PropertyName, std::any>& values) const {
  auto property = node.GetProperty<std::vector<uint32_t>>(name());
  if (property.is_error()) {
    return property.take_error();
  }
  values[name()] = *property;
  return zx::ok();
}

zx::result<> StringListProperty::Parse(Node& node, std::map<PropertyName, std::any>& values) const {
  auto property = node.GetProperty<std::vector<std::string>>(name());
  if (property.is_error()) {
    return property.take_error();
  }
  values[name()] = *property;
  return zx::ok();
}

zx::result<> ReferenceProperty::Parse(Node& node, std::map<PropertyName, std::any>& values) const {
  auto prop_value = node.properties().find(name());
  if (prop_value == node.properties().end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto bytes = prop_value->second.AsBytes();
  if (bytes.size() % sizeof(uint32_t) != 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto cells = Uint32Array(bytes);
  std::vector<Reference> references;
  for (size_t cell_offset = 0; cell_offset < cells.size();) {
    auto phandle = cells[cell_offset];
    auto reference = node.GetReferenceNode(phandle);
    if (reference.is_error()) {
      fdf::error("Node '{}' has invalid reference in '{}' property to {}.", node.name(), name(),
                 phandle);

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    // Advance past phandle.
    cell_offset++;
    uint32_t cell_count = 0;
    if (std::holds_alternative<PropertyName>(cell_specifier_)) {
      auto cells_prop_name = std::get<PropertyName>(cell_specifier_);
      auto cell_specifier = reference->GetProperty<uint32_t>(cells_prop_name);
      if (cell_specifier.is_error()) {
        fdf::error("Failed to parse reference with {} cells. reference node: {}. error: {}",
                   reference->name(), cells_prop_name, cell_specifier.status_value());

        return cell_specifier.take_error();
      }
      cell_count = *cell_specifier;
    } else {
      cell_count = std::get<uint32_t>(cell_specifier_);
    }

    size_t width_in_bytes = cell_count * sizeof(uint32_t);
    size_t byteview_offset = cell_offset * sizeof(uint32_t);
    cell_offset += cell_count;

    if (byteview_offset > bytes.size() || (width_in_bytes > bytes.size() - byteview_offset)) {
      fdf::error(
          "Reference node '{}' has less data than expected. Expected {} bytes, remaining {} bytes",
          reference->name(), width_in_bytes, bytes.size() - byteview_offset);

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    PropertyCells reference_cells = bytes.subspan(byteview_offset, width_in_bytes);
    references.emplace_back(*reference, reference_cells);
  }

  values[name()] = references;
  return zx::ok();
}

}  // namespace fdf_devicetree
