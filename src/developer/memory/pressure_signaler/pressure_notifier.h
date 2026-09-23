// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_PRESSURE_SIGNALER_PRESSURE_NOTIFIER_H_
#define SRC_DEVELOPER_MEMORY_PRESSURE_SIGNALER_PRESSURE_NOTIFIER_H_

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <fidl/fuchsia.memorypressure/cpp/fidl.h>

#include <vector>

#include "src/developer/memory/pressure_signaler/pressure_observer.h"

namespace pressure_signaler {

class PressureNotifier : public fidl::Server<fuchsia_memorypressure::Provider> {
 public:
  // This struct contains all the state related to a single registered watchers.
  //
  // Note: `proxy` needs an event handler to be initialized, which is why its initialization needs
  // to be deferred.
  struct WatcherState : public fidl::AsyncEventHandler<fuchsia_memorypressure::Watcher> {
   public:
    WatcherState(Level level_sent, PressureNotifier* notifier)
        : level_sent(level_sent), notifier(notifier) {}
    std::optional<fidl::Client<fuchsia_memorypressure::Watcher>> proxy;
    Level level_sent;
    bool needs_free = false;
    bool pending_callback = false;
    PressureNotifier* notifier;
    void on_fidl_error(fidl::UnbindInfo info) override { notifier->ReleaseWatcher(this); }
  };

  explicit PressureNotifier(bool watch_for_changes, bool send_critical_pressure_crash_reports,
                            fidl::Client<fuchsia_feedback::CrashReporter> crash_reporter,
                            async_dispatcher_t* dispatcher = nullptr);

  PressureNotifier(const PressureNotifier&) = delete;
  PressureNotifier& operator=(const PressureNotifier&) = delete;

  // fuchsia::memorypressure::Provider interface
  void RegisterWatcher(RegisterWatcherRequest& request,
                       RegisterWatcherCompleter::Sync& completer) override;

  // Notify watchers of a pressure level change.
  void Notify();

  // Notify watchers with a simulated memory pressure |level|. For diagnostic use by MemoryDebugger.
  void DebugNotify(fuchsia_memorypressure::Level level) const;

 private:
  void PostLevelChange();
  void ReleaseWatcher(WatcherState* watcher_state);
  void OnLevelChangedCallback(WatcherState* watcher);
  void NotifyWatcher(WatcherState* watcher, Level level);

  bool CanGenerateNewCriticalCrashReports();
  enum CrashReportType : uint8_t {
    kImminentOOM,
    kCritical,
  };
  void FileCrashReport(CrashReportType type);

  async::TaskClosureMethod<PressureNotifier, &PressureNotifier::PostLevelChange> post_task_{this};
  async_dispatcher_t* const provider_dispatcher_;
  std::vector<std::unique_ptr<WatcherState>> watchers_;
  PressureObserver observer_;

  bool observed_normal_level_ = true;
  zx::time prev_critical_crash_report_time_ = zx::time(ZX_TIME_INFINITE_PAST);
  zx::duration critical_crash_report_interval_ = zx::min(30);
  const bool send_critical_pressure_crash_reports_;
  fidl::Client<fuchsia_feedback::CrashReporter> crash_reporter_;

  friend class test::PressureNotifierUnitTest;
};

}  // namespace pressure_signaler

#endif  // SRC_DEVELOPER_MEMORY_PRESSURE_SIGNALER_PRESSURE_NOTIFIER_H_
