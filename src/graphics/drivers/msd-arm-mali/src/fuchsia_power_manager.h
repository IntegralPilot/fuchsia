// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_FUCHSIA_POWER_MANAGER_H_
#define SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_FUCHSIA_POWER_MANAGER_H_

#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>

#include <optional>

#include "parent_device.h"
#include "power_manager.h"
#include "timeout_source.h"

// FuchsiaPowerManager manages power state transitions (power-on, power-down, suspend, resume).
//
// State Machine & Suspend Mechanics:
// - When system suspend is requested via Suspend(), |in_suspend_| becomes true. If the GPU is
//   currently powered on, |power_on_after_suspend_| is recorded as true and PowerDown() is
//   executed.
// - While |in_suspend_| is true, calls to EnablePower() do not power on the hardware immediately,
//   but set |power_on_after_suspend_ = true|.
// - When system resume occurs via Resume(), |in_suspend_| becomes false. If
// |power_on_after_suspend_|
//   is true, hardware power-up is initiated and |power_on_after_suspend_| is reset to false.
class FuchsiaPowerManager final : public TimeoutSource {
 public:
  class Owner {
   public:
    using PowerStateCallback = fit::callback<void(bool)>;
    // Posts a request to change the GPU hardware power state.
    // |completer| is invoked with the resulting power state once hardware transition completes.
    virtual void PostPowerStateChange(bool enabled, PowerStateCallback completer) = 0;
    // Returns pointer to the driver's PowerManager instance (or nullptr if uninitialized).
    virtual PowerManager* GetPowerManager() = 0;
  };

  explicit FuchsiaPowerManager(Owner* owner);

  bool Initialize(inspect::Node& node);

  TimeoutSource::Clock::time_point GetCurrentTimeoutPoint() override;
  void TimeoutTriggered() override { DisablePower(); }

  // Requests powering on GPU hardware. If system is currently suspended, defers power-on until
  // Resume().
  void EnablePower();
  // Requests powering down GPU hardware (e.g. idle timeout or suspend).
  void DisablePower();

  // Suspends GPU power management during driver framework suspend transitions.
  // Powers down hardware if currently on and records state for resumption.
  void Suspend(fit::callback<void()> completer);

  // Resumes GPU power management after driver framework suspend transitions.
  // Restores hardware power if active prior to suspend or requested during suspend.
  void Resume(fit::callback<void()> completer);

  static constexpr char kIsSystemSuspendingInspectNode[] = "is_system_suspending";
  static constexpr char kPoweredOnInspectNode[] = "powered_on";
  static constexpr char kPowerOnAfterSuspendInspectNode[] = "power_on_after_suspend";
  static constexpr char kSuspendBlockerName[] = "mali-gpu-suspend-blocker";

 private:
  void PowerUp(fit::callback<void()> callback);
  void PowerDown(fit::callback<void()> callback);

  Owner* owner_;
  // Our current power state (true if GPU cores are powered on).
  bool powered_on_ = false;

  // In-flight state transition flags to avoid redundant power state change requests.
  bool powering_up_ = false;
  bool powering_down_ = false;

  // Whether the device is currently in system suspend. While true, power-on requests are queued.
  bool in_suspend_ = false;

  // Whether we should power on or not after suspend. This is set based on the state we were
  // in when we went into suspend, and any requests we get during suspend.
  bool power_on_after_suspend_ = false;

  inspect::LazyNode lazy_;
};

#endif  // SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_FUCHSIA_POWER_MANAGER_H_
