// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fuchsia_power_manager.h"

#include <lib/fit/defer.h>

FuchsiaPowerManager::FuchsiaPowerManager(Owner* owner) : owner_(owner) {}

bool FuchsiaPowerManager::Initialize(inspect::Node& node) {
  lazy_ = node.CreateLazyValues("lazy_fuchsia_power_manager", [this] {
    inspect::Inspector inspector;
    inspector.GetRoot().CreateBool(kIsSystemSuspendingInspectNode, in_suspend_, &inspector);
    inspector.GetRoot().CreateBool(kPoweredOnInspectNode, powered_on_, &inspector);
    inspector.GetRoot().CreateBool(kPowerOnAfterSuspendInspectNode, power_on_after_suspend_,
                                   &inspector);
    return fpromise::make_ok_promise(std::move(inspector));
  });
  return true;
}

TimeoutSource::Clock::time_point FuchsiaPowerManager::GetCurrentTimeoutPoint() {
  // If we are off or going off there's no timeout.
  if (!powered_on_ || powering_down_ || !owner_->GetPowerManager()) {
    return Clock::time_point::max();
  }
  return owner_->GetPowerManager()->GetGpuPowerdownTimeout();
}

void FuchsiaPowerManager::EnablePower() {
  // Do nothing if we are on or in the process of being on.
  if (powered_on_ || powering_up_) {
    return;
  }
  // Do nothing if we are in suspend, we will power back on during resume.
  if (in_suspend_) {
    power_on_after_suspend_ = true;
    return;
  }
  PowerUp([]() {});
}

void FuchsiaPowerManager::DisablePower() {
  // Do nothing if we are off or in the process of being off.
  if (!powered_on_ || powering_down_) {
    return;
  }
  PowerDown([]() {});
}

void FuchsiaPowerManager::PowerUp(fit::callback<void()> callback) {
  powering_up_ = true;
  owner_->PostPowerStateChange(true,
                               [this, callback = std::move(callback)](bool powered_on) mutable {
                                 powering_up_ = false;
                                 powered_on_ = powered_on;
                                 callback();
                               });
}

void FuchsiaPowerManager::PowerDown(fit::callback<void()> callback) {
  powering_down_ = true;
  owner_->PostPowerStateChange(false,
                               [this, callback = std::move(callback)](bool powered_on) mutable {
                                 powering_down_ = false;
                                 powered_on_ = powered_on;
                                 callback();
                               });
}

void FuchsiaPowerManager::Suspend(fit::callback<void()> completer) {
  in_suspend_ = true;
  power_on_after_suspend_ = powered_on_;
  PowerDown([completer = std::move(completer)]() mutable { completer(); });
}

void FuchsiaPowerManager::Resume(fit::callback<void()> completer) {
  in_suspend_ = false;
  if (power_on_after_suspend_) {
    PowerUp([completer = std::move(completer)]() mutable { completer(); });
  } else {
    completer();
  }
  power_on_after_suspend_ = false;
}
