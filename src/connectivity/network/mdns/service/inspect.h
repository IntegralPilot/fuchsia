// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_INSPECT_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_INSPECT_H_

#include <lib/inspect/component/cpp/component.h>

#include <src/connectivity/network/mdns/service/common/bounded_queue.h>

#include "lib/async/default.h"

namespace mdns {

namespace testing {
class MdnsInspectorTest;
}  // namespace testing

// This class defines the structure of the Inspect hierarchy for Mdns. Components
// of Mdns can invoke the MdnsInspector to log events.
// For example:
//
// auto inspector = mdns::MdnsInspector::GetMdnsInspector();
// inspector.NotifyStateChange(
//     mdns::MdnsInspector::MdnsStatusNode::kWaitingForInterfaces);
//
// The MdnsInspector holds types of MdnsStateNode as sub-nodes
// of the Inspect hierarchy. MdnsStatusNode will contain multiple MdnsStatusEntry
// sub-nodes. These sub-nodes are added whenever an Mdns state change is notified by
// Mdns and will contain complete information of current mdns status. To obtain
// history of Mdns status changes MdnsStatusNode will hold last |kMaxMdnsStatusEntries|.
//
// Sample Inspect hierarchy:
//   "root": {
//       "mdns_status": {
//         "0x13": {
//           "state": "kWaitingForInterfaces",
//           "@time": 479097104375,
//         },
//         "0x12": {
//           ..
//           ..
//         },
//         ..
//         ..
//       },
//   }

class MdnsInspector final {
 public:
  using MdnsState = std::string;
  static const MdnsState kNotStarted;
  static const MdnsState kWaitingForInterfaces;
  static const MdnsState kAddressProbeInProgress;
  static const MdnsState kActive;

  // Creates/Returns the pointer to MdnsInspector singleton object.
  static MdnsInspector& GetMdnsInspector();
  MdnsInspector(MdnsInspector const&) = delete;
  void operator=(MdnsInspector const&) = delete;

  // Function to be called when state changes.
  // This will add a new entry with all current statuses to mdns_status node.
  void NotifyStateChange(const MdnsState& new_state);

  friend class testing::MdnsInspectorTest;

 private:
  MdnsInspector() : MdnsInspector(async_get_default_dispatcher()) {}
  explicit MdnsInspector(async_dispatcher_t* dispatcher);

  class MdnsStatusNode {
   public:
    explicit MdnsStatusNode(inspect::Node& parent_node);

    // This structure holds current Mdns status at any point of time.
    struct MdnsCurrentStatus {
      MdnsCurrentStatus();
      MdnsState state_;
    };

    // An entry in MdnsStatusNode. Each entry is timestamped.
    struct MdnsStatusEntry {
      MdnsStatusEntry(inspect::Node& parent_node, const MdnsCurrentStatus& current_status);

      inspect::Node mdns_status_entry_node_;
      inspect::StringProperty state_property_;

      inspect::UintProperty timestamp_property_;
    };

    // Add a new entry in mdns_status with current status values.
    void LogCurrentStatus();

    // Return current mdns status.
    MdnsCurrentStatus GetCurrentMdnsStatus() { return current_status_; }

    // Update current state and add a new entry for mdns status.
    void RecordStateChange(const MdnsState& new_state);

   private:
    inspect::Node node_;
    BoundedQueue<MdnsStatusEntry> mdns_status_entries_;
    MdnsCurrentStatus current_status_;
  };

  std::unique_ptr<inspect::ComponentInspector> inspector_;
  MdnsStatusNode mdns_status_;
};

}  // namespace mdns
#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_INSPECT_H_
