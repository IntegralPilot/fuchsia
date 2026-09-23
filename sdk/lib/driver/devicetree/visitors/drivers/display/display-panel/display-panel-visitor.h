// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_DISPLAY_DISPLAY_PANEL_DISPLAY_PANEL_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_DISPLAY_DISPLAY_PANEL_DISPLAY_PANEL_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace display_panel_visitor_dt {

// TODO(https://fxbug.dev/342016112): This visitor will have to be redone or removed once the
// display panel metadata is updated or moved into structured config.
class DisplayPanelVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kPanelType[] = "panel-type";

  DisplayPanelVisitor();
  DisplayPanelVisitor(const DisplayPanelVisitor&) = delete;
  DisplayPanelVisitor& operator=(const DisplayPanelVisitor&) = delete;
  ~DisplayPanelVisitor() override = default;

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  bool IsMatch(const std::string& node_name);

  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace display_panel_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_DISPLAY_DISPLAY_PANEL_DISPLAY_PANEL_VISITOR_H_
