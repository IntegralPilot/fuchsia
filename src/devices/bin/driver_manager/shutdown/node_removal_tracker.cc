// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/shutdown/node_removal_tracker.h"

#include <zircon/errors.h>
#include <zircon/status.h>

#include <src/devices/lib/log/log.h>

#include "src/devices/bin/driver_manager/node.h"

namespace driver_manager {

namespace {

const char* GetNodeStateDescription(NodeState state) {
  switch (state) {
    case NodeState::kWaitingOnDriverBind:
      // This log message is used by tefmocheck to detect driver start/bind hangs.
      // LINT.IfChange
      return "waiting for driver to finish binding";
      // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go)
    case NodeState::kRunning:
      return "in normal running state";
    case NodeState::kPrestop:
      return "in running state, but flagged for removal soon.";
    case NodeState::kWaitingOnChildren:
      return "waiting for children to complete shutdown";
    case NodeState::kWaitingOnDriver:
      // This message is load-bearing server-side as it's used to identify the hanging driver.
      // It is also used by tefmocheck to detect driver removal hangs.
      // Please notify //src/developer/forensics/OWNERS upon changing.
      // LINT.IfChange
      return "waiting for driver's Stop() function and destructor finish running";
      // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go)
    case NodeState::kWaitingOnDriverComponent:
      return "waiting for the driver component to stop";
    case NodeState::kStopped:
      return "node component instance stop is completed";
    case NodeState::kWaitingOnDestroy:
      return "waiting for the component to be destroyed.";
    case NodeState::kDestroyed:
      return "node shutdown is completed";
  }
}

}  // namespace

NodeId NodeRemovalTracker::RegisterNode(NodeInfo info) {
  if (info.state == NodeState::kDestroyed) {
    return next_node_id_;
  }

  if (info.collection == Collection::kPackage) {
    remaining_pkg_nodes_.emplace(next_node_id_);
  } else {
    remaining_non_pkg_nodes_.emplace(next_node_id_);
  }
  nodes_[next_node_id_] = info;
  return next_node_id_++;
}

void NodeRemovalTracker::Notify(NodeId id, NodeState state) {
  auto itr = nodes_.find(id);
  if (itr == nodes_.end()) {
    fdf_log::error("Tried to Notify without registering!");
    return;
  }
  itr->second.state = state;

  if (check_removal_task_.is_pending()) {
    check_removal_task_.Cancel();
    check_removal_task_.PostDelayed(dispatcher_, kRemovalCheckDelay);
  }

  if (state != NodeState::kDestroyed) {
    return;
  }

  if (itr->second.collection == Collection::kPackage) {
    remaining_pkg_nodes_.erase(id);
  } else {
    remaining_non_pkg_nodes_.erase(id);
  }
  CheckRemovalDone();
}

void NodeRemovalTracker::CheckRemoval() {
  timeout_count_++;
  const bool pkg_only = !all_callback_ && pkg_callback_;

  // This log message is used by tefmocheck to detect driver removal hangs.
  // LINT.IfChange
  if (pkg_callback_ && all_callback_) {
    fdf_log::info("Full node removal hanging: Waiting on {} nodes ({} package nodes)",
                  remaining_node_count(), remaining_pkg_node_count());
  } else if (pkg_only) {
    fdf_log::info("Package node removal hanging: Waiting on {} package nodes",
                  remaining_pkg_node_count());
  } else {
    fdf_log::info("Full node removal hanging: Waiting on {} nodes", remaining_node_count());
    if (!all_callback_ && !pkg_callback_) {
      fdf_log::warn("Node-removal tracker is running but no one is listening");
    }
  }
  // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go)

  for (auto& [id, node] : nodes_) {
    if (node.state == NodeState::kDestroyed || node.state == NodeState::kPrestop) {
      continue;
    }
    if (pkg_only && node.collection != Collection::kPackage) {
      // Don't print non-package nodes if we are only waiting for package nodes to be removed.
      continue;
    }

    if (node.state == NodeState::kWaitingOnDriver) {
      if (auto locked_node = node.node.lock()) {
        if (auto host = locked_node->driver_host()) {
          host->TriggerStackTrace();
        }
      }
    }

    // This log message is load-bearing server-side as it's used to identify the hanging driver.
    // Please notify //src/developer/forensics/OWNERS upon changing.
    fdf_log::info("  '{}' ('{}'): {}", node.name, node.driver_url,
                  GetNodeStateDescription(node.state));
  }
  if (timeout_count_ >= kMaxRemovalCheckCount) {
    on_removal_timeout_callback_();
  }
  check_removal_task_.PostDelayed(dispatcher_, kRemovalCheckDelay);
}

void NodeRemovalTracker::CheckRemovalDone() {
  if (fully_enumerated_ == false) {
    return;
  };

  if (pkg_callback_ && remaining_pkg_node_count() == 0) {
    fdf_log::info("NodeRemovalTracker: package removal completed");
    pkg_callback_();
    pkg_callback_ = nullptr;
    // If we are not waiting for all nodes to be removed, then the removal is complete and we can
    // cancel the task.
    if (!all_callback_) {
      check_removal_task_.Cancel();
    }
  }
  if (all_callback_ && remaining_node_count() == 0) {
    fdf_log::info("NodeRemovalTracker: all nodes removed");
    all_callback_();
    all_callback_ = nullptr;
    check_removal_task_.Cancel();
    nodes_.clear();
  }
}

void NodeRemovalTracker::set_pkg_callback(fit::callback<void()> callback) {
  pkg_callback_ = std::move(callback);
}
void NodeRemovalTracker::set_all_callback(fit::callback<void()> callback) {
  all_callback_ = std::move(callback);
}
void NodeRemovalTracker::SetOnRemovalTimeoutCallback(fit::callback<void()> callback) {
  on_removal_timeout_callback_ = std::move(callback);
}

void NodeRemovalTracker::FinishEnumeration() {
  fully_enumerated_ = true;
  check_removal_task_.PostDelayed(dispatcher_, kRemovalCheckDelay);
  CheckRemovalDone();
}

}  // namespace driver_manager
