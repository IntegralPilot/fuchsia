// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MAILBOX_CONTROLLERS_MAILBOX_MAILBOX_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MAILBOX_CONTROLLERS_MAILBOX_MAILBOX_VISITOR_H_

#include <fidl/fuchsia.hardware.mailbox/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <map>

namespace mailbox_dt {

class MailboxVisitor : public fdf_devicetree::Visitor {
 public:
  MailboxVisitor();

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

 private:
  fuchsia_hardware_mailbox::ControllerInfo& GetOrCreateController(uint32_t node_id);

  std::unique_ptr<fdf_devicetree::PropertyParser> mailbox_parser_;

  std::map<uint32_t, std::vector<fuchsia_hardware_mailbox::ChannelInfo>> controller_info_;
};

}  // namespace mailbox_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MAILBOX_CONTROLLERS_MAILBOX_MAILBOX_VISITOR_H_
