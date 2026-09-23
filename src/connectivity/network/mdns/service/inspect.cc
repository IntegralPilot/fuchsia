// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "inspect.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>

namespace mdns {

// Inspect Node names
constexpr std::string_view kNode_Time = "@time";
constexpr std::string_view kNode_MdnsStatus = "mdns_status";
constexpr std::string_view kNode_MdnsStatus_State = "state";

constexpr int kMaxMdnsStatusEntries = 10;

// Inspect Node values
constexpr MdnsInspector::MdnsState MdnsInspector::kActive = "Active";
constexpr MdnsInspector::MdnsState MdnsInspector::kWaitingForInterfaces = "WaitingForInterfaces";
constexpr MdnsInspector::MdnsState MdnsInspector::kAddressProbeInProgress =
    "AddressProbeInProgress";
constexpr MdnsInspector::MdnsState MdnsInspector::kNotStarted = "NotStarted";

MdnsInspector& MdnsInspector::GetMdnsInspector() {
  static MdnsInspector mdns_inspector;
  return mdns_inspector;
}

MdnsInspector::MdnsInspector(async_dispatcher_t* dispatcher)
    : inspector_(
          std::make_unique<inspect::ComponentInspector>(dispatcher, inspect::PublishOptions{})),
      mdns_status_(inspector_->root()) {}

void MdnsInspector::NotifyStateChange(const MdnsState& new_state) {
  mdns_status_.RecordStateChange(new_state);
}

MdnsInspector::MdnsStatusNode::MdnsStatusEntry::MdnsStatusEntry(
    inspect::Node& parent_node, const MdnsCurrentStatus& current_status)
    : mdns_status_entry_node_(parent_node.CreateChild(parent_node.UniqueName(""))) {
  state_property_ =
      mdns_status_entry_node_.CreateString(kNode_MdnsStatus_State, current_status.state_);
  timestamp_property_ =
      mdns_status_entry_node_.CreateUint(kNode_Time, zx::clock::get_monotonic().get());
}

MdnsInspector::MdnsStatusNode::MdnsCurrentStatus::MdnsCurrentStatus()
    : state_(MdnsInspector::kNotStarted) {}

MdnsInspector::MdnsStatusNode::MdnsStatusNode(inspect::Node& parent_node)
    : node_(parent_node.CreateChild(kNode_MdnsStatus)),
      mdns_status_entries_(kMaxMdnsStatusEntries) {}

void MdnsInspector::MdnsStatusNode::LogCurrentStatus() {
  mdns_status_entries_.AddEntry(node_, current_status_);
}

void MdnsInspector::MdnsStatusNode::RecordStateChange(const MdnsInspector::MdnsState& new_state) {
  if (current_status_.state_ == new_state) {
    return;
  }
  current_status_.state_ = new_state;
  LogCurrentStatus();
}

}  // namespace mdns
