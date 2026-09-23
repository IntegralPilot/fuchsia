// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_WAKE_LEASE_H_
#define LIB_DRIVER_POWER_CPP_WAKE_LEASE_H_

#include <fidl/fuchsia.power.system/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/clock.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <string>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_power {

// This is probably not the implementation you're looking for, consider using
// `WakeLeaseProvider`. `ManualWakeLease` may be appropriate if
// `WakeLeaseProvider` does not fit your needs.
//
// ManualWakeLease can be used to prevent the system from suspending. After a
// call to `Start()` returns, the system will keep running until after `End()`
// is called or the instance is dropped. Users can use the same instance to
// perform multiple atomic operations, for example by calling `Start()` after
// `End()`.
//
// If doing multiple atomic operations, a single `ManualWakeLease` can have
// performance advantages over using a WakeLease directly because the
// `ManualWakeLease` monitors system state over its lifetime, allowing it to
// avoid certain operations vs a series of shorter-lived `WakeLease` instances.
class ManualWakeLease : public fidl::WireServer<fuchsia_power_system::SuspendBlocker> {
 public:
  ManualWakeLease(async_dispatcher_t* dispatcher, std::string_view name,
                  fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag,
                  inspect::Node* parent_node = nullptr, bool log = false,
                  uint32_t long_duration_threshold_ms = 0);

  // Start an atomic operation. If `ActivityGovernor` protocol connection is
  // valid the system is guaranteed to stay running until `End()` is called or
  // this instance is dropped.
  //
  // Returns `true` if one of the following is true:
  //   * It was not necessary to take a SAG wake lease at this time because the
  //     system is resumed
  //   * A SAG wake lease was acquired
  //   * A SAG wake lease was previously acquired
  // Returns `false` if it was necessary to acquire a SAG wake lease, but
  // an error occurred during acquisition. In this case, the `ManualWakeLease`
  // instance should be replaced with a new one.
  bool Start(bool ignore_system_state = false);

  // Indicate that the operation is complete. The system may suspend after this
  // call is made. Calling `End()` makes sense in the case the caller wants to
  // use this instance to start a new atomic operation in the future. Returns
  // the wake lease currently currently held, if any.
  zx::result<zx::eventpair> End();

  // Stores the SAG wake lease in the instance. The wake lease will be retained
  // until `End` or `TakeWakeLease` is called.
  void DepositWakeLease(zx::eventpair wake_lease);

  // Consider whether End() is more appropriate for the use case.
  //
  // Returns ZX_ERR_BAD_HANDLE if we don't currently have a wake lease.
  // IMPORTANT: This does not implicitly call `End()`, meaning that at the next
  // system transition to suspend, this class will take a SAG wake lease and
  // cause the suspend to abort.
  zx::result<zx::eventpair> TakeWakeLease();

  // Get a duplicate of the stored SAG wake lease. Returns ZX_ERR_BAD_HANDLE if
  // we don't currently have a wake lease.
  zx::result<zx::eventpair> GetWakeLeaseCopy();

  // fuchsia.power.system/SuspendBlocker implementation. This is used to avoid
  // creatingwake leases in cases where the system is resumed and `HandleInterrupt`
  // is called.
  void AfterResume(AfterResumeCompleter::Sync& completer) override;

  void BeforeSuspend(BeforeSuspendCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_system::SuspendBlocker> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void SetSuspended(bool suspended) { system_suspended_ = suspended; }
  bool IsSuspended() { return system_suspended_; }

 private:
  void ResetSagClient();

  bool AcquireLease(bool ignore_system_state = false);

  bool LongDurationRecordingActive() const { return long_threshold_.get() > 0; }

  std::string lease_name_;
  bool log_;
  fidl::WireSyncClient<fuchsia_power_system::ActivityGovernor> sag_client_;
  std::optional<fidl::ServerBinding<fuchsia_power_system::SuspendBlocker>> suspend_blocker_binding_;
  bool system_suspended_ = true;
  bool active_ = false;

  zx::eventpair lease_;

  // Leases requested or acquired for longer than this duration are recorded.
  zx::duration long_threshold_;
  // The last time a wake lease was requested, i.e. `Start` was called.
  zx::time last_request_time_;
  // The last time a lease was acquired.
  zx::time last_acquisition_time_;

  inspect::UintProperty total_lease_acquisitions_;
  inspect::BoolProperty wake_lease_held_;
  inspect::BoolProperty wake_lease_grabbable_;
  inspect::UintProperty wake_lease_last_attempted_acquisition_timestamp_;
  inspect::UintProperty wake_lease_last_acquired_timestamp_;
  inspect::UintProperty wake_lease_last_refreshed_timestamp_;
  inspect::UintProperty requested_duration_exceeding_threshold_;
  inspect::UintProperty acquisitions_exceeding_threshold_;
  inspect::UintProperty long_duration_threshold_;
};

// Wrapper around usage of fuchsia.power.system/ActivityGovernor.AcquireWakeLease. The wrapper
// reduces SAG wake lease creation by allowing callers to set timeout after which to drop the
// lease and provides mechanisms to extend that timeout.
class TimeoutWakeLease {
 public:
  // If |log| is set to true, logs will be emitted when acquiring leases and when lease times out.
  // An invalid |sag_client| will result in silently disabling wake lease acquisition.
  TimeoutWakeLease(async_dispatcher_t* dispatcher, std::string_view lease_name,
                   fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag_client,
                   inspect::Node* parent_node = nullptr, bool log = false,
                   uint32_t long_duration_threshold = 0);

  // Ensure that the system stays awake until the timeout is reached. To accomplish this we either:
  //   * obtain a SAG wake lease immediately if the system is currently suspended
  //   * obtain a SAG wake lease in the future if the system begins suspension before the timeout
  //     is reached
  // If a SAG wake lease is acquired, it is dropped when the timeout is reached. The timeout may be
  // extended by additional calls to `HandleInterrupt` or `AcquireWakeLease`. If the system is not
  // suspended and does not attempt suspension during the timeout period, no wake lease is obtained.
  //
  // Ideally this is only called after we wake up due to an interrupt. Note that a duration is
  // taken because the deadline is computed once the lease is acquired, rather than at the point
  // this method is called.
  //
  // Returns `false` if it was necessary to obtain a SAG wake lease and it failed to do so. Returns
  // `true` if it already had a wake lease or if it did not and it succeeded in obtaining one.
  bool HandleInterrupt(zx::duration timeout);

  // Acquire a SAG wake lease and automatically drop it after the specified timeout. If a lease was
  // still already held, it will be extended until the new timeout. Note that a duration is taken
  // because the deadline is computed once the SAG lease is acquired,rather than at the point this
  // method is called.
  //
  // Returns `false` if it was necessary to obtain a SAG wake lease and it failed to do so. Returns
  // `true` if it already had a wake lease or if it did not and it succeeded in obtaining one.
  bool AcquireWakeLease(zx::duration timeout);

  // Provide a SAG wake lease to the `TimeWakeLease` which will be dropped either:
  //   * immediately if there is already a SAG wake lease with a later deadline
  //   * at the specified deadline
  // In the latter case any previous SAG lease held by this object is dropped immediately.
  void DepositWakeLease(zx::eventpair wake_lease, zx::time timeout_deadline);

  // Cancel timeout and take the SAG wake lease. Returns ZX_ERR_BAD_HANDLE if we don't currently
  // have a wake lease.
  zx::result<zx::eventpair> TakeWakeLease();

  // Get a duplicate of the stored SAG wake lease. Returns ZX_ERR_BAD_HANDLE if we don't currently
  // have a SAG wake lease.
  zx::result<zx::eventpair> GetWakeLeaseCopy();

  // Get the time our next timeout will occur. If there is no currently active
  // timeout this will be ZX_TIME_INFINITE.
  zx_time_t GetNextTimeout();

  // Returns `true` if the instance thinks the system is currently resumed.
  // This value may be wrong if the instance has not observed any system state
  // changes since it was created.
  bool IsResumed() { return !lease_.IsSuspended(); }

  void SetSuspended(bool suspended) { lease_.SetSuspended(suspended); }

 private:
  void HandleTimeout();
  void ResetTimeout(zx::duration timeout);
  void ResetTimeout(zx::time time);

  ManualWakeLease lease_;
  async_dispatcher_t* dispatcher_;
  bool log_;
  std::string lease_name_;

  async::TaskClosureMethod<TimeoutWakeLease, &TimeoutWakeLease::HandleTimeout> lease_task_{this};
};

// `WakeLease` wraps a WakeLease. When `WakeLease`
// destructs, the underlying SAG wake lease, if any currently held, is
// dropped.
class WakeLease {
 public:
  explicit WakeLease(const std::shared_ptr<ManualWakeLease>& lease) : lease_(lease) {}
  zx::result<zx::eventpair> GetDuplicateLeaseHandle() { return lease_->GetWakeLeaseCopy(); }
  // Intended for testing, gets a shared pointer to the wrapped
  // fdf_power::TimeoutWakeLease object.
  std::shared_ptr<ManualWakeLease> GetWakeLease() { return lease_; }
  ~WakeLease() { zx::result<zx::eventpair> obsolete_lease = lease_->End(); }

 private:
  std::shared_ptr<ManualWakeLease> lease_;
};

// This class is **not** threadsafe! `WakeLeaseProvider` and
// `WakeLease` pointers returned from `StartOperation` must be used
// on the same thread!
//
// Provides shared pointers to `WakeLeases` which are effectively
// fdf_power::WakeLease objects. This provides RAII semantics to manage the
// lifecycle of the underlying wake lease such that for a given
// `WakeLeaseProvider` there is _at_ _most_ one WakeLease in
// existence at any moment. It is worth noting that WakeLease only acquires a
// wake lease from the system if the system starts to suspend while the
// WakeLease is held.
//
// Using `WakeLeaseProvider` can have performance advantages over
// using a WakeLease directly because the provider monitors system state
// over its lifetime, allowing it to avoid certain operations vs a series of
// shorter-lived `WakeLease` instances which would each only observe system
// state while they exist.
//
// The main difference between the shared pointer to a `WakeLease`
// from `WakeLeaseProvider` and an `ManualWakeLease` is that with
// `WakeLease` we get long-lived system state monitoring and
// ergonomic RAII management of the actual lease. With `ManualWakeLease` you
// can get long-lived system state monitoring with `End` calls and sacrifice
// RAII ergonomics **or** get RAII ergonomics, but lose system state knowledge
// after the `ManualWakeLease` is destroyed.
class WakeLeaseProvider {
 public:
  // See comment in ManualWakeLease::Start
  WakeLeaseProvider(async_dispatcher_t* dispatcher, std::string_view name,
                    fidl::ClientEnd<fuchsia_power_system::ActivityGovernor> sag,
                    bool ignore_system_state = false, inspect::Node* parent_node = nullptr,
                    bool log = false)
      : fdf_lease_(
            std::make_shared<ManualWakeLease>(dispatcher, name, std::move(sag), parent_node, log)),
        ignore_system_state_(ignore_system_state) {}
  std::shared_ptr<WakeLease> StartOperation() {
    // WakeLeaseProvider works by holding and owning a
    // fdf_power::WakeLease and creating, but not owning, a WakeLease.
    // The provider vends pointers to the WakeLease, if it exists,
    // otherwise creating it. When the WakeLease destructs, it drops the
    // actual wake lease held by the fdf_power::WakeLease that it wraps.

    // If we currently hold a WakeLease, return a pointer to it,
    // otherwise create a new one with a reference to the wake lease we hold.
    std::shared_ptr<WakeLease> op = atomic_op_.lock();
    if (!op) {
      // If we don't have a WakeLease, call HandleInterrupt on the
      // WakeLease so that it is "active". Any previous WakeLease
      // that destructed would have retrieved and dropped the actual wake lease
      // from the WakeLease object.
      fdf_lease_->Start(ignore_system_state_);
      op = std::make_shared<WakeLease>(fdf_lease_);
      atomic_op_ = op;
    }
    return op;
  }

 private:
  std::weak_ptr<WakeLease> atomic_op_;
  std::shared_ptr<ManualWakeLease> fdf_lease_;
  const bool ignore_system_state_;
};

}  // namespace fdf_power

#endif

#endif  // LIB_DRIVER_POWER_CPP_WAKE_LEASE_H_
