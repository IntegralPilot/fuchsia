// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/pressure_signaler/pressure_notifier.h"

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>

#include <algorithm>

namespace pressure_signaler {

namespace {

// Convert monitor::Level to the Level type signalled by the fuchsia.memorypressure service.
zx::result<fuchsia_memorypressure::Level> ConvertToMemoryPressureServiceLevel(Level level) {
  switch (level) {
    case Level::kCritical:
      return zx::ok(fuchsia_memorypressure::Level::kCritical);
    case Level::kWarning:
      return zx::ok(fuchsia_memorypressure::Level::kWarning);
    case Level::kNormal:
      return zx::ok(fuchsia_memorypressure::Level::kNormal);
    default:
      return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
}

// Convert from the Level type signalled by the fuchsia.memorypressure service to monitor::Level.
Level ConvertFromMemoryPressureServiceLevel(fuchsia_memorypressure::Level level) {
  switch (level) {
    case fuchsia_memorypressure::Level::kCritical:
      return Level::kCritical;
    case fuchsia_memorypressure::Level::kWarning:
      return Level::kWarning;
    case fuchsia_memorypressure::Level::kNormal:
      return Level::kNormal;
  }
}

}  // namespace

// |dispatcher| is the dispatcher associated with memory_pressure_signaler's main thread.
// The fuchsia::memorypressure::Provider service which the |PressureNotifier| class implements runs
// on this thread.
PressureNotifier::PressureNotifier(bool watch_for_changes,
                                   bool send_critical_pressure_crash_reports,
                                   fidl::Client<fuchsia_feedback::CrashReporter> crash_reporter,
                                   async_dispatcher_t* dispatcher)
    : provider_dispatcher_(dispatcher),
      observer_(watch_for_changes, this),
      send_critical_pressure_crash_reports_(send_critical_pressure_crash_reports),
      crash_reporter_(std::move(crash_reporter)) {
  FX_CHECK(dispatcher);
}

void PressureNotifier::Notify() {
  if (provider_dispatcher_) {
    post_task_.Post(provider_dispatcher_);
  }
}

void PressureNotifier::PostLevelChange() {
  Level level_to_send = observer_.GetCurrentLevel();
  if (level_to_send == Level::kImminentOOM) {
    // We condition sending a crash report for imminent OOM the same way as for critical memory
    // pressure.
    if (send_critical_pressure_crash_reports_) {
      FileCrashReport(CrashReportType::kImminentOOM);
    }

    // Nothing else to do. This is a diagnostic-only level that is not signaled to watchers.
    return;
  }

  if (level_to_send == Level::kNormal) {
    // See comments about |observed_normal_level_| in the definition of |FileCrashReport()|.
    observed_normal_level_ = true;
  } else if (send_critical_pressure_crash_reports_ && level_to_send == Level::kCritical &&
             CanGenerateNewCriticalCrashReports()) {
    // File crash report before notifying watchers, so that we can capture the state *before*
    // watchers can respond to memory pressure, thereby changing the state that caused the memory
    // pressure in the first place.
    FileCrashReport(CrashReportType::kCritical);
  }

  // TODO(rashaeqbal): Throttle notifications to prevent thrashing.
  for (auto& watcher : watchers_) {
    // Notify the watcher only if we received a response for the previous level change, i.e. there
    // is no pending callback.
    if (!watcher->pending_callback) {
      watcher->pending_callback = true;
      NotifyWatcher(watcher.get(), level_to_send);
    }
  }
}

void PressureNotifier::DebugNotify(fuchsia_memorypressure::Level level) const {
  FX_LOGS(INFO) << "Simulating memory pressure level "
                << kLevelNames[ConvertFromMemoryPressureServiceLevel(level)];
  for (auto& watcher : watchers_) {
    watcher->proxy.value()
        ->OnLevelChanged({{.level = level}})
        .Then([](fidl::Result<fuchsia_memorypressure::Watcher::OnLevelChanged> result) {
          if (!result.is_ok()) {
            FX_LOGS(ERROR) << "Failed to simulate pressure level signal: " << result.error_value();
          }
        });
  }
}

void PressureNotifier::NotifyWatcher(WatcherState* watcher, Level level) {
  // We should already have set |pending_callback| when the notification (call to NotifyWatcher())
  // was posted, to prevent removing |WatcherState| from |watchers_| in the error handler.
  ZX_ASSERT(watcher->pending_callback);

  // We should not be notifying a watcher if |needs_free| is set - indicating that a delayed free is
  // required. This can only happen if there was a pending callback when we tried to release the
  // watcher. No new notifications can be sent out while there is a pending callback. And when the
  // callback is invoked, the |WatcherState| is removed from the |watchers_| vector, so we won't
  // post any new notifications after that.
  ZX_ASSERT(!watcher->needs_free);

  watcher->level_sent = level;
  auto level_or_error = ConvertToMemoryPressureServiceLevel(level);
  ZX_ASSERT(level_or_error.is_ok());
  watcher->proxy.value()
      ->OnLevelChanged({{.level = level_or_error.value()}})
      .Then([watcher, this](fidl::Result<fuchsia_memorypressure::Watcher::OnLevelChanged> result) {
        if (result.is_ok()) {
          OnLevelChangedCallback(watcher);
        } else {
          FX_LOGS(ERROR) << "Failed to signal pressure change: " << result.error_value();
        }
      });
}

void PressureNotifier::OnLevelChangedCallback(WatcherState* watcher) {
  watcher->pending_callback = false;

  // The error handler invoked ReleaseWatcher(), but we could not free the |WatcherState| because of
  // this outstanding callback. It is safe to free the watcher now. There are no more outstanding
  // callbacks, and no new notifications (since a new notification is posted only if there is no
  // pending callback).
  if (watcher->needs_free) {
    ReleaseWatcher(watcher);
    return;
  }

  Level current_level = observer_.GetCurrentLevelForWatcher();
  // The watcher might have missed a level change if it occurred before this callback. If the
  // level has changed, notify the watcher.
  if (watcher->level_sent != current_level) {
    // Set |pending_callback| to true here before posting the NotifyWatcher() call. This ensures
    // that if ReleaseWatcher() is called (via the error handler) after we post the call, but before
    // we dispatch it, we don't access a freed |WatcherState*| in the NotifyWatcher() call.
    // ReleaseWatcher() will find |pending_callback| set, hence delay freeing the watcher and set
    // |needs_free| to true. NotifyWatcher() will operate on a valid |WatcherState*|, the next
    // callback will find |needs_free| set and free the watcher.
    watcher->pending_callback = true;
    async::PostTask(provider_dispatcher_,
                    [watcher, current_level, this]() { NotifyWatcher(watcher, current_level); });
  }
}

void PressureNotifier::RegisterWatcher(RegisterWatcherRequest& request,
                                       RegisterWatcherCompleter::Sync& completer) {
  Level current_level = observer_.GetCurrentLevelForWatcher();
  // Create the watcher's state, which will hold the client and its handler.
  auto& watcher = watchers_.emplace_back(std::make_unique<WatcherState>(current_level, this));

  watcher->proxy = fidl::Client<fuchsia_memorypressure::Watcher>(
      std::move(request.watcher()), provider_dispatcher_, watcher.get());

  // Set |pending_callback| and notify the current level.
  watcher->pending_callback = true;
  NotifyWatcher(watcher.get(), current_level);
}

void PressureNotifier::ReleaseWatcher(WatcherState* watcher) {
  auto predicate = [watcher](const auto& target) { return target.get() == watcher; };
  auto watcher_to_free = std::ranges::find_if(watchers_, predicate);
  if (watcher_to_free == watchers_.end()) {
    // Not found.
    return;
  }

  // There is a pending callback, which also means that the Watcher (client) holds a reference to
  // the |WatcherState| unique pointer (the callback captures a raw pointer - |WatcherState*|).
  // Freeing it now can lead to a use-after-free. Set |needs_free| to indicate that we need a
  // delayed free, when the pending callback is executed.
  //
  // NOTE: It is possible that a Watcher exits (closes its connection) and never invokes the
  // callback. In that case, we will never be able to free the corresponding |WatcherState|, which
  // is fine, since this is the only way we can safeguard against a use-after-free.
  if ((*watcher_to_free)->pending_callback) {
    (*watcher_to_free)->needs_free = true;
  } else {
    watchers_.erase(watcher_to_free);
  }
}

bool PressureNotifier::CanGenerateNewCriticalCrashReports() {
  // Generate a new Critical crash report only if any of these two conditions hold:
  // 1. |observed_normal_level_| is set to true, which indicates that a Normal level
  // was observed after the last Critical crash report.
  // 2. At least |critical_crash_report_interval_| time has elapsed since the last Critical crash
  // report.
  //
  // This is done for two reasons:
  // 1) It helps limit the number of Critical crash reports we generate.
  // 2) If the memory pressure changes to Critical again after going via Normal, we're
  // presumably observing a different memory usage pattern / use case, so it makes sense to
  // generate a new crash report. Instead if we're only observing Critical -> Warning ->
  // Critical transitions, we might be seeing the same memory usage pattern repeat.
  return (observed_normal_level_ ||
          zx::clock::get_monotonic() >=
              (prev_critical_crash_report_time_ + critical_crash_report_interval_));
}

void PressureNotifier::FileCrashReport(CrashReportType type) {
  fuchsia_feedback::CrashReport report{
      {.program_name = "system", .program_uptime = zx_clock_get_monotonic(), .is_fatal = false}};
  switch (type) {
    case CrashReportType::kImminentOOM:
      report.crash_signature("fuchsia-imminent-oom");
      break;
    case CrashReportType::kCritical:
      report.crash_signature("fuchsia-critical-memory-pressure");
      break;
  }

  crash_reporter_->FileReport({{.report = std::move(report)}})
      .Then([](fidl::Result<fuchsia_feedback::CrashReporter::FileReport> result) {
        if (!result.is_ok()) {
          FX_LOGS(ERROR) << "Failed to file a report: " << result.error_value();
        };
      });

  // Logic to control rate of Critical crash report generation.
  if (type == CrashReportType::kCritical) {
    prev_critical_crash_report_time_ = zx::clock::get_monotonic();

    // Clear |observed_normal_level_| and wait for another normal level change to occur.
    observed_normal_level_ = false;
  }
}

}  // namespace pressure_signaler
