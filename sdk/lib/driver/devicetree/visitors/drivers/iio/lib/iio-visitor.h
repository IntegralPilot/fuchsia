// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_IIO_LIB_IIO_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_IIO_LIB_IIO_VISITOR_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace iio_dt {

class IioVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kIioChannelReference[] = "io-channels";
  static constexpr char kIioChannelCells[] = "#io-channel-cells";
  static constexpr char kIioChannelNames[] = "io-channel-names";

  IioVisitor();

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 protected:
  virtual bool is_match(fdf_devicetree::Node& node);
  bool is_match(fdf_devicetree::ReferenceNode& node) { return is_match(*node.GetNode()); }

 private:
  virtual zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                           fdf_devicetree::ReferenceNode& parent,
                                           fdf_devicetree::PropertyCells specifiers,
                                           std::optional<std::string_view> name) = 0;

  std::unique_ptr<fdf_devicetree::PropertyParser> iio_parser_;
};

}  // namespace iio_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_IIO_LIB_IIO_VISITOR_H_
