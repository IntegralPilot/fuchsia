// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "interconnect-visitor.h"

#include <fidl/fuchsia.hardware.interconnect/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/property-parser.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstdint>
#include <ranges>

#include <bind/fuchsia/cpp/bind.h>

namespace interconnect_dt {

namespace {

class InterconnectCells {
 public:
  explicit InterconnectCells(fdf_devicetree::PropertyCells cells)
      : interconnect_cells_(cells, 1, 1, 1) {}

  uint32_t src_interconnect_phandle() const { return (uint32_t)interconnect_cells_[0][1].value(); }
  uint32_t src_node_id() const { return (uint32_t)interconnect_cells_[0][0].value(); }
  uint32_t dst_interconnect_phandle() const { return (uint32_t)interconnect_cells_[0][1].value(); }
  uint32_t dst_node_id() const { return (uint32_t)interconnect_cells_[0][2].value(); }

 private:
  using InterconnectElement = devicetree::PropEncodedArrayElement<3>;
  devicetree::PropEncodedArray<InterconnectElement> interconnect_cells_;
};

// This class attempts to parse the following devicetree structure:
// <src_phandle src_id dst_phandle dst_id>
class InterconnectReferenceProperty : public fdf_devicetree::Property {
 public:
  explicit InterconnectReferenceProperty(fdf_devicetree::PropertyName name)
      : Property(std::move(name), false) {}

  zx::result<> Parse(fdf_devicetree::Node& node,
                     std::map<fdf_devicetree::PropertyName, std::any>& values) const override {
    auto prop = node.properties().find(name());
    if (prop == node.properties().end()) {
      return zx::ok();
    }

    devicetree::ByteView bytes = prop->second.AsBytes();
    if (bytes.size() % sizeof(uint32_t) != 0) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    auto cells = fdf_devicetree::Uint32Array(bytes);

    fdf_devicetree::References references;
    for (size_t cell_offset = 0; cell_offset < cells.size();) {
      auto phandle = cells[cell_offset];
      zx::result reference_node = node.GetReferenceNode(phandle);
      if (reference_node.is_error()) {
        fdf::error("Node '{}' has invalid reference in '{}' property to {}.", node.name(), name(),
                   phandle);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      // Advance past phandle.
      cell_offset++;
      uint32_t cell_count = 0;
      constexpr char kInterconnectCells[] = "#interconnect-cells";
      auto cell_count_prop = reference_node->GetProperty<uint32_t>(kInterconnectCells);
      if (cell_count_prop.is_error()) {
        fdf::error("Reference node '{}' does not have'{}' property: {}",
                   reference_node->name().c_str(), kInterconnectCells,
                   cell_count_prop.status_value());
        return cell_count_prop.take_error();
      }
      cell_count = *cell_count_prop;

      // Each tuple contains 3 values: (src_node_id, src_interconnect_phandle, dst_node_id).
      // The node ids are cell count and we have 2 of those, whereas the phandle is 1 cell. Each
      // cell is a uint32_t.
      size_t width_in_bytes = cell_count * 2 * sizeof(uint32_t) + 4;
      size_t byteview_offset = cell_offset * sizeof(uint32_t);
      cell_offset += cell_count;

      if (byteview_offset > bytes.size() || (width_in_bytes > bytes.size() - byteview_offset)) {
        fdf::error(
            "Reference node '{}' has less data than expected. Expected {} bytes, remaining {} bytes",
            reference_node->name().c_str(), width_in_bytes, bytes.size() - byteview_offset);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      auto phandle2 = cells[cell_offset];
      if (phandle2 != phandle) {
        fdf::error("Node '{}' has invalid reference in '{}' property to {}.", node.name(), name(),
                   phandle2);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      cell_offset++;
      cell_offset += cell_count;

      fdf_devicetree::PropertyCells reference_cells =
          bytes.subspan(byteview_offset, width_in_bytes);

      references.emplace_back(*reference_node, reference_cells);
    }

    values[name()] = references;
    return zx::ok();
  }
};

fdf_devicetree::Properties MakeProperties() {
  fdf_devicetree::Properties props;
  props.emplace_back(std::make_unique<fdf_devicetree::StringListProperty>(
      InterconnectVisitor::kInterconnectNames));
  props.emplace_back(
      std::make_unique<InterconnectReferenceProperty>(InterconnectVisitor::kInterconnectReference));
  props.emplace_back(std::make_unique<fdf_devicetree::Uint32ArrayProperty>(
      InterconnectVisitor::kInterconnectTags));
  return props;
}

}  // namespace

InterconnectVisitor::InterconnectVisitor() : parser_(MakeProperties()) {}

bool InterconnectVisitor::IsMatch(std::string_view name) {
  return name.starts_with("interconnect");
}

zx::result<> InterconnectVisitor::Visit(fdf_devicetree::Node& node,
                                        const devicetree::PropertyDecoder& decoder) {
  zx::result<fdf_devicetree::ParsedProperties> parse_result = parser_.Parse(node);
  if (parse_result.is_error()) {
    fdf::error("Interconnect visitor failed for node '{}' : {}", node.name(), parse_result);

    return parse_result.take_error();
  }

  auto references = parse_result->Get<fdf_devicetree::References>(kInterconnectReference);
  if (!references) {
    return zx::ok();
  }

  fdf::debug("Found node with interconnect reference: {}", node.name());

  auto names = parse_result->Get<std::vector<std::string>>(kInterconnectNames);
  if (!names) {
    fdf::error(
        "Interconnect reference '{}' does not have valid interconnect names property. Name is required to generate bind rules.",
        node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (references->size() != names->size()) {
    fdf::error(
        "Interconnect reference '{}' does not have valid number of interconnect names. {} interconnects found, and {} interconnect names found, they must be equal.",
        node.name(), references->size(), names->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto tags = parse_result->Get<std::vector<uint32_t>>(kInterconnectTags);
  if (tags && references->size() != tags->size()) {
    fdf::error(
        "Interconnect reference '{}' does not have valid number of interconnect tags. {} interconnects found, and {} interconnect tags found, they must be equal.",
        node.name(), references->size(), tags->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (size_t index = 0; index < references->size(); index++) {
    auto& reference = (*references)[index];
    auto& parent = reference.reference_node();
    auto cells = reference.property_cells();
    auto tag = tags ? (*tags)[index] : std::optional<uint32_t>{};
    if (IsMatch(parent.name())) {
      zx::result result = ParseReferenceChild(node, parent, cells, (*names)[index], tag);
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }

  return zx::ok();
}

zx::result<> InterconnectVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t id,
                                                   std::string_view path_name) {
  auto interconnect_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                      "fuchsia.hardware.interconnect.PathService"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE,
                                 "fuchsia.hardware.interconnect.PathService"),
              fdf::MakeProperty2(bind_fuchsia::NAME, std::string(path_name)),
          },
  }};

  child.AddNodeSpec(interconnect_node);
  return zx::ok();
}

InterconnectVisitor::Interconnect& InterconnectVisitor::GetInterconnect(
    fdf_devicetree::Phandle phandle) {
  if (!interconnects_.contains(phandle)) {
    interconnects_[phandle] = Interconnect{};
  }
  return interconnects_[phandle];
}

zx::result<> InterconnectVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                                      const fdf_devicetree::ReferenceNode& parent,
                                                      fdf_devicetree::PropertyCells specifiers,
                                                      std::string_view path_name,
                                                      std::optional<uint32_t> tag) {
  fdf::debug("Parsing reference child: {}", child.name());
  auto& interconnect = GetInterconnect(parent.phandle().value());

  if (specifiers.size_bytes() != 3 * sizeof(uint32_t)) {
    fdf::error(
        "Interconnect reference '{}' has incorrect number of interconnect specifiers ({}) - expected 3.",
        child.name(), specifiers.size_bytes() / sizeof(uint32_t));
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  InterconnectCells prop(specifiers);
  fuchsia_hardware_interconnect::PathEndpoints path{{
      .name = std::string(path_name),
      .id = id_++,
      .src_node_id = prop.src_node_id(),
      .dst_node_id = prop.dst_node_id(),
  }};

  if (tag.has_value()) {
    path.tag() = tag.value();
  }

  fdf::debug("Interconnect ID added - ID 0x{:x} name '{}' to interconnect '{}'", *path.id(),
             path_name, parent.name());

  auto& paths = interconnect.metadata.paths();
  if (!paths.has_value()) {
    paths.emplace(std::vector<fuchsia_hardware_interconnect::PathEndpoints>{});
  }
  paths->emplace_back(path);

  return AddChildNodeSpec(child, path.id().value(), path_name);
}

zx::result<> InterconnectVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that it is indeed a interconnect that we support.
  if (!IsMatch(node.name())) {
    return zx::ok();
  }

  if (!node.phandle()) {
    return zx::ok();
  }

  if (!interconnects_.contains(*node.phandle())) {
    fdf::debug("Interconnect '{}' is not being used. Not adding any metadata for it.", node.name());
    return zx::ok();
  }
  const Interconnect& interconnect = interconnects_.at(*node.phandle());

  const auto& paths = interconnect.metadata.paths();
  if (paths.has_value() && !paths.value().empty()) {
    const fit::result encoded_metadata = fidl::Persist(interconnect.metadata);
    if (!encoded_metadata.is_ok()) {
      fdf::error("Failed to encode interconnect paths: {}",
                 encoded_metadata.error_value().FormatDescription());
      return zx::error(encoded_metadata.error_value().status());
    }
    node.AddMetadata(fuchsia_hardware_platform_bus::Metadata{{
        .id = fuchsia_hardware_interconnect::Metadata::kSerializableName,
        .data = encoded_metadata.value(),
    }});

    fdf::debug("Interconnect node ID's metadata added to node '{}'", node.name());
  }

  return zx::ok();
}

}  // namespace interconnect_dt

REGISTER_DEVICETREE_VISITOR(interconnect_dt::InterconnectVisitor);
