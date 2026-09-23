// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/bootup_tracker.h"

#include <lib/async/cpp/time.h>

#include <unordered_set>

#include <src/devices/lib/log/log.h>

#include "src/devices/bin/driver_manager/bind/bind_manager.h"
#include "src/devices/bin/driver_manager/driver_host.h"
#include "src/devices/bin/driver_manager/node.h"

namespace driver_manager {

namespace {

zx::duration kBootupTimeoutDuration = zx::sec(2);
zx::duration kLastUpdatedTimeoutDuration = zx::sec(20);
zx::duration kMaxTimeoutDuration = zx::sec(60);

}  // namespace

void BootupTracker::Start() {
  current_timeout_ = kBootupTimeoutDuration;
  UpdateTrackerAndResetTimer();
}

void BootupTracker::WaitForBootup(fit::callback<void()> callback) {
  if (bootup_done_) {
    callback();
  } else {
    callbacks_.push_back(std::move(callback));
  }
}

void BootupTracker::NotifyNewStartRequest(std::string node_moniker, std::string driver_url,
                                          std::weak_ptr<Node> node) {
  if (outstanding_start_requests_.find(node_moniker) != outstanding_start_requests_.end()) {
    fdf_log::warn("Bootup tracker received conflicting start requests for node {}", node_moniker);
  }
  outstanding_start_requests_[node_moniker] = {
      .driver_url = std::move(driver_url),
      .node = std::move(node),
  };
  UpdateTrackerAndResetTimer();
}

void BootupTracker::NotifyStartComplete(std::string node_moniker) {
  if (auto itr = outstanding_start_requests_.find(node_moniker);
      itr != outstanding_start_requests_.end()) {
    outstanding_start_requests_.erase(itr);
  } else {
    fdf_log::info("Bootup tracker notified for an unknown start request for {}", node_moniker);
  }
  UpdateTrackerAndResetTimer();
}

void BootupTracker::NotifyBindingChanged() { UpdateTrackerAndResetTimer(); }

void BootupTracker::BootupDoneForTesting() {
  for (auto& callback : callbacks_) {
    callback();
  }
  callbacks_.clear();
  bootup_done_ = true;
}

bool BootupTracker::BootupComplete() const { return bootup_done_; }

void BootupTracker::CheckBootupDone() {
  if (IsUpdateDeadlineExceeded() &&
      (!outstanding_start_requests_.empty() || bind_manager_->HasOngoingBind())) {
    // This log message is used by tefmocheck to detect driver start/bind hangs.
    // LINT.IfChange
    fdf_log::warn("Deadline exceeded in the bootup tracker with:");
    // LINT.ThenChange(/tools/testing/tefmocheck/string_in_log_check.go)
    fdf_log::warn("    {} unfinished start requests:", outstanding_start_requests_.size());
    std::unordered_set<const DriverHost*> driver_hosts;
    for (const auto& [moniker, request] : outstanding_start_requests_) {
      fdf_log::warn("         - {} - {}", moniker, request.driver_url);
      if (auto node = request.node.lock()) {
        if (auto host = node->driver_host()) {
          if (driver_hosts.find(host) == driver_hosts.end()) {
            host->TriggerStackTrace();
            driver_hosts.insert(host);
          }
        }
      }
    }
    if (bind_manager_->HasOngoingBind()) {
      fdf_log::warn("    a hanging bind process in the bind manager");
    }

    current_timeout_ *= 2;
    if (current_timeout_ > kMaxTimeoutDuration) {
      current_timeout_ = kMaxTimeoutDuration;
    }
  }

  if (!outstanding_start_requests_.empty() || bind_manager_->HasOngoingBind()) {
    ResetBootupTimer();
    return;
  }

  // LINT.IfChange
  fdf_log::info("Bootup completed.");
  // LINT.ThenChange(//tools/testing/testrunner/tester.go)

  for (auto& callback : callbacks_) {
    callback();
  }
  callbacks_.clear();
  bootup_done_ = true;
}

void BootupTracker::UpdateTrackerAndResetTimer() {
  last_update_timestamp_ = async::Now(dispatcher_);
  current_timeout_ = kBootupTimeoutDuration;
  ResetBootupTimer();
}

void BootupTracker::OnBootupTimeout() {
  bootup_timeout_ = true;
  CheckBootupDone();
}

bool BootupTracker::IsUpdateDeadlineExceeded() const {
  auto time_delta = async::Now(dispatcher_) - last_update_timestamp_;
  return time_delta >= kLastUpdatedTimeoutDuration;
}

void BootupTracker::ResetBootupTimer() {
  if (bootup_done_) {
    return;
  }
  if (bootup_timeout_task_.is_pending()) {
    bootup_timeout_task_.Cancel();
  }
  bootup_timeout_task_.PostDelayed(dispatcher_, current_timeout_);
}

}  // namespace driver_manager
