// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_PROPERTY_PARSER_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_PROPERTY_PARSER_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/fit/function.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <any>
#include <cstdint>
#include <map>
#include <memory>
#include <optional>
#include <utility>
#include <variant>
#include <vector>

namespace fdf_devicetree {

using PropertyName = std::string;
// Cells specific to the property represented in a prop-encoded-array.
using PropertyCells = devicetree::ByteView;

// Abstract class to represent a property type.
// To create an instance of Property, use the specific property types.
// Eg: Uint32ArrayProperty, StringListProperty, ReferenceProperty etc.
class Property {
 public:
  explicit Property(PropertyName name, bool required = false)
      : name_(std::move(name)), required_(required) {}

  virtual ~Property() = default;

  virtual zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const = 0;

  PropertyName name() const { return name_; }

  bool required() const { return required_; }

 private:
  PropertyName name_;
  bool required_;
};

using Properties = std::vector<std::unique_ptr<Property>>;

class BoolProperty : public Property {
 public:
  explicit BoolProperty(PropertyName name, bool required = false)
      : Property(std::move(name), required) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;
};

class Uint32Property : public Property {
 public:
  explicit Uint32Property(PropertyName name, bool required = false)
      : Property(std::move(name), required) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;
};

class Uint64Property : public Property {
 public:
  explicit Uint64Property(PropertyName name, bool required = false)
      : Property(std::move(name), required) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;
};

class StringProperty : public Property {
 public:
  explicit StringProperty(PropertyName name, bool required = false)
      : Property(std::move(name), required) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;
};

// Property of uint32 array type.
class Uint32ArrayProperty : public Property {
 public:
  explicit Uint32ArrayProperty(PropertyName name, bool required = false)
      : Property(std::move(name), required) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;
};

// Property of string list type.
class StringListProperty : public Property {
 public:
  explicit StringListProperty(PropertyName name, bool required = false)
      : Property(std::move(name), required) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;
};

// Represents a property that contains one or more references to other devicetree nodes (phandles),
// each potentially followed by a set of arguments (cells).
//
// For example: `gpios = <&gpio1 1 2>, <&gpio2 3 4 0>;`
// This property contains two references: one to `gpio1` with cells `{1, 2}` and one to `gpio2` with
// cells `{3, 4, 0}`.
class ReferenceProperty : public Property {
 public:
  // Constructs a `ReferenceProperty` parser.
  //
  // The `cell_specifier` determines how many cells (u32 values) follow each phandle in the
  // property. It can be either:
  //
  // 1. A `PropertyName` (e.g., "#interrupt-cells"): The number of cells is determined by reading
  //    the value of that property from the referenced node. This is the most common case.
  //
  //    Example:
  //      intc: interrupt-controller@... {
  //          #interrupt-cells = <2>;
  //      };
  //      my_device: my_device@... {
  //          interrupts = <&intc 1 8>; // Parsed using `ReferenceProperty("interrupts",
  //                                   // "#interrupt-cells")`.
  //      };
  //
  // 2. A `uint32_t`: A fixed number of cells that applies to all references within this property.
  //
  //    Example:
  //      my_device: my_device@... {
  //          mboxes = <&mbox 0>, <&mbox 1>; // Parsed using `ReferenceProperty("mboxes", 1)`.
  //      };
  explicit ReferenceProperty(PropertyName name, std::variant<PropertyName, uint32_t> cell_specifier,
                             bool required = false)
      : Property(std::move(name), required), cell_specifier_(std::move(cell_specifier)) {}

  zx::result<> Parse(Node& node, std::map<PropertyName, std::any>& values) const override;

 private:
  std::variant<PropertyName, uint32_t> cell_specifier_;
};

// A handle to a reference property. It consists of the referenced node and the property cells if
// any.
class Reference {
 public:
  Reference(ReferenceNode node, PropertyCells cells)
      : reference_node_(node), property_cells_(cells) {}

  // Returns the referenced devicetree node.
  //
  // For example, in the devicetree fragment:
  //   interrupt-parent = <&intc>;
  // this would return the node object for `intc`.
  ReferenceNode& reference_node() { return reference_node_; }

  // Returns the property cells associated with the reference. These are the
  // arguments that qualify the reference.
  //
  // For example, in the devicetree fragment:
  //   gpios = <&gpio1 1 8>, <&gpio2 2 8 7>;
  //
  // For the first reference, `property_cells()` would return a view of
  // the bytes corresponding to `{1, 8}`.
  //
  // For the second reference, `property_cells()` would return a view of
  // the bytes corresponding to `{2, 8, 7}`.
  PropertyCells property_cells() { return property_cells_; }

 private:
  ReferenceNode reference_node_;
  PropertyCells property_cells_;
};

using References = std::vector<Reference>;

// Helper class to hold the parsed properties and provide utility functions to access the values
// appropriately.
class ParsedProperties {
 public:
  explicit ParsedProperties(std::map<PropertyName, std::any> properties)
      : properties_(std::move(properties)) {}

  // Gets the parsed value for a property named `name`, cast to type `T`.
  //
  // Returns an `std::optional<T>` containing the value if the property exists
  // and can be cast to `T`. Otherwise, returns `std::nullopt`.
  //
  // Example usage:
  //
  // To get a single string property:
  //   auto str_prop = props.Get<std::string>("string-property");
  //
  // To get an array of uint32s:
  //   auto arr_prop = props.Get<std::vector<uint32_t>>("uint32-array-property");
  //
  // To get a list of references:
  //   auto ref_prop = props.Get<fdf_devicetree::References>("reference-property");
  //   if (ref_prop) {
  //     for (const auto& ref : *ref_prop) {
  //       // process reference
  //     }
  //
  // A special case is provided for `bool`. `Get<bool>("property-name")` will
  // return `true` if the property exists and is true, and `false` otherwise
  // (if it does not exist or is false).
  template <typename T>
  auto Get(const PropertyName& name) const {
    if constexpr (std::is_same_v<T, bool>) {
      auto it = properties_.find(name);
      if (it == properties_.end()) {
        return false;
      }
      if (auto* value = std::any_cast<bool>(&it->second)) {
        return *value;
      }
      return false;
    } else {
      auto it = properties_.find(name);
      if (it == properties_.end()) {
        return std::optional<T>(std::nullopt);
      }
      if (auto* value = std::any_cast<T>(&it->second)) {
        return std::optional<T>(*value);
      }
      return std::optional<T>(std::nullopt);
    }
  }

  void AddProperty(const PropertyName& name, std::any value);

 private:
  std::map<PropertyName, std::any> properties_;
};

// Helper class to parse properties concerning a visitor. A visitor can bunch all the necessary
// properties it needs to extract from a node and call the |PropertyParser::Parse| method.
// Properties can be a string list, uint32 array or reference properties - see below for classes
// that inherit from |Property| for the complete list of supported property types. The visitor can
// also use this collect all connected properties and associate them appropriately using the vector
// index.
class PropertyParser {
 public:
  explicit PropertyParser(Properties properties) : properties_(std::move(properties)) {}

  PropertyParser(PropertyParser&&) = default;
  PropertyParser& operator=(PropertyParser&&) = default;

  virtual ~PropertyParser() = default;

  virtual zx::result<ParsedProperties> Parse(Node& node);

 private:
  Properties properties_;
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_PROPERTY_PARSER_H_
