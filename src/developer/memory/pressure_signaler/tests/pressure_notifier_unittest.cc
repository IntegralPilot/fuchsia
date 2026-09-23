// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/pressure_signaler/pressure_notifier.h"

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <fidl/fuchsia.feedback/cpp/test_base.h>
#include <fidl/fuchsia.memory.debug/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <cstddef>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace pressure_signaler::test {

namespace fmp = fuchsia_memorypressure;

namespace {
class CrashReporterForTest : public fidl::testing::TestBase<fuchsia_feedback::CrashReporter> {
 public:
  void FileReport(FileReportRequest& request, FileReportCompleter::Sync& completer) override {
    num_crash_reports_++;
    completer.Reply(fit::ok(fuchsia_feedback::CrashReporterFileReportResponse{}));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_NOTIMPLEMENTED() << name << " is not implemented";
  }

  size_t num_crash_reports() const { return num_crash_reports_; }

 private:
  size_t num_crash_reports_ = 0;
};

class PressureWatcherForTest : public fidl::Server<fmp::Watcher> {
 public:
  PressureWatcherForTest(bool send_responses, async_dispatcher_t* dispatcher)
      : send_responses_(send_responses) {
    auto endpoints = fidl::CreateEndpoints<fmp::Watcher>();
    client_ = std::move(endpoints->client);
    binding_ = fidl::BindServer<fmp::Watcher>(dispatcher, std::move(endpoints->server), this);
  }

  ~PressureWatcherForTest() override {
    if (binding_)
      binding_->Close({});
  }

  void OnLevelChanged(OnLevelChangedRequest& request,
                      OnLevelChangedCompleter::Sync& completer) override {
    changes_++;
    last_level_ = request.level();
    if (send_responses_) {
      completer.Reply();
    } else {
      stashed_cb_ = completer.ToAsync();
    }
  }

  void Respond() { stashed_cb_->Reply(); }

  void Register(fidl::Client<fmp::Provider> provider) {
    auto result = provider->RegisterWatcher({{.watcher = std::move(client_)}});
    ASSERT_TRUE(result.is_ok());
  }

  int NumChanges() const { return changes_; }

  fmp::Level LastLevel() const { return last_level_; }

 private:
  fmp::Level last_level_;
  int changes_ = 0;
  bool send_responses_;
  std::optional<OnLevelChangedCompleter::Async> stashed_cb_;
  fidl::ClientEnd<fmp::Watcher> client_;
  std::optional<fidl::ServerBindingRef<fmp::Watcher>> binding_;
};
}  // namespace

class PressureNotifierUnitTest : public fidl::Server<fuchsia_memory_debug::MemoryPressure>,
                                 public gtest::TestLoopFixture {
 public:
  void Signal(SignalRequest& request, SignalCompleter::Sync& completer) override {
    notifier_->DebugNotify(request.level());
  }

 protected:
  void SetUp() override {
    SetUpNewPressureNotifier(true /*notify_crash_reporter*/);
    last_level_ = Level::kNormal;
  }

  void SetUpNewPressureNotifier(bool send_critical_pressure_crash_reports) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_feedback::CrashReporter>().value();
    auto _ = fidl::BindServer<fuchsia_feedback::CrashReporter>(
        dispatcher(), std::move(endpoints.server), &crash_reporter_);
    auto client =
        fidl::Client<fuchsia_feedback::CrashReporter>(std::move(endpoints.client), dispatcher());
    notifier_ =
        std::make_unique<PressureNotifier>(false, send_critical_pressure_crash_reports,
                                           std::move(client), async_get_default_dispatcher());
    // Set up initial pressure level.
    notifier_->observer_.WaitOnLevelChange();
  }

  fidl::Client<fmp::Provider> Provider() {
    auto endpoints = fidl::CreateEndpoints<fmp::Provider>().value();
    auto _ =
        fidl::BindServer<fmp::Provider>(dispatcher(), std::move(endpoints.server), notifier_.get());
    return fidl::Client<fmp::Provider>(std::move(endpoints.client), dispatcher());
  }

  size_t GetWatcherCount() { return notifier_->watchers_.size(); }

  void ReleaseWatchers() {
    std::vector<PressureNotifier::WatcherState*> targets;
    targets.reserve(notifier_->watchers_.size());
    for (auto& w : notifier_->watchers_) {
      targets.push_back(w.get());
    }
    for (auto* t : targets) {
      notifier_->ReleaseWatcher(t);
    }
  }

  void TriggerLevelChange(Level level) {
    if (level >= Level::kNumLevels) {
      return;
    }
    FX_LOGS(INFO) << "PressureNotifierUnitTest: Triggering level change to " << kLevelNames[level];
    notifier_->observer_.OnLevelChanged(notifier_->observer_.wait_items_[level].handle);
    RunLoopUntilIdle();
  }

  void SetupMemDebugService() {
    auto endpoints = fidl::CreateEndpoints<fuchsia_memory_debug::MemoryPressure>().value();
    auto _ = fidl::BindServer<fuchsia_memory_debug::MemoryPressure>(
        dispatcher(), std::move(endpoints.server), this);
    memdebug_ = fidl::Client<fuchsia_memory_debug::MemoryPressure>(std::move(endpoints.client),
                                                                   dispatcher());
  }

  void TestSimulatedPressure(fmp::Level level) {
    auto result = memdebug_->Signal({{.level = level}});
    ASSERT_TRUE(result.is_ok());
  }

  void SetCrashReportInterval(uint32_t mins) {
    notifier_->critical_crash_report_interval_ = zx::min(mins);
  }

  bool CanGenerateNewCriticalCrashReports() const {
    return notifier_->CanGenerateNewCriticalCrashReports();
  }

  size_t num_crash_reports() const { return crash_reporter_.num_crash_reports(); }

  Level last_level() const { return last_level_; }

 private:
  std::unique_ptr<PressureNotifier> notifier_;
  CrashReporterForTest crash_reporter_;
  Level last_level_;
  fidl::Client<fuchsia_memory_debug::MemoryPressure> memdebug_;
};

namespace {
TEST_F(PressureNotifierUnitTest, Watcher) {
  // Scoped so that the Watcher gets deleted. We can then verify that the Provider has no watchers
  // remaining.
  {
    PressureWatcherForTest watcher(true, dispatcher());
    // Registering the watcher should call OnLevelChanged().
    watcher.Register(Provider());
    RunLoopUntilIdle();
    ASSERT_EQ(GetWatcherCount(), 1ul);
    ASSERT_EQ(watcher.NumChanges(), 1);

    // Trigger a pressure level change, causing another call to OnLevelChanged().
    TriggerLevelChange(Level::kNormal);
    RunLoopUntilIdle();
    ASSERT_EQ(watcher.NumChanges(), 2);
  }

  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 0ul);
}

TEST_F(PressureNotifierUnitTest, NoResponse) {
  PressureWatcherForTest watcher(false, dispatcher());
  watcher.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 1ul);
  ASSERT_EQ(watcher.NumChanges(), 1);

  // This should not trigger a new notification as the watcher has not responded to the last one.
  TriggerLevelChange(Level::kNormal);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 1);
}

TEST_F(PressureNotifierUnitTest, DelayedResponse) {
  PressureWatcherForTest watcher(false, dispatcher());
  // Signal a specific pressure level here, so that the next one can be different. Delayed callbacks
  // will only come through if the client has missed a level that wasn't the same as the previous
  // one it received a signal for.
  TriggerLevelChange(Level::kNormal);
  watcher.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 1ul);
  ASSERT_EQ(watcher.NumChanges(), 1);

  // This should not trigger a new notification as the watcher has not responded to the last one.
  TriggerLevelChange(Level::kWarning);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 1);

  // Respond to the last message. This should send a new notification to the watcher.
  watcher.Respond();
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 2);
}

TEST_F(PressureNotifierUnitTest, MultipleWatchers) {
  // Scoped so that the Watcher gets deleted. We can then verify that the Provider has no watchers
  // remaining.
  {
    PressureWatcherForTest watcher1(true, dispatcher());
    PressureWatcherForTest watcher2(true, dispatcher());

    // Registering the watchers should call OnLevelChanged().
    watcher1.Register(Provider());
    watcher2.Register(Provider());
    RunLoopUntilIdle();
    ASSERT_EQ(GetWatcherCount(), 2ul);
    ASSERT_EQ(watcher1.NumChanges(), 1);
    ASSERT_EQ(watcher2.NumChanges(), 1);

    // Trigger pressure level change, causing another call to OnLevelChanged().
    TriggerLevelChange(Level::kNormal);
    RunLoopUntilIdle();
    ASSERT_EQ(watcher1.NumChanges(), 2);
    ASSERT_EQ(watcher2.NumChanges(), 2);
  }

  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 0ul);
}

TEST_F(PressureNotifierUnitTest, MultipleWatchersNoResponse) {
  PressureWatcherForTest watcher1(false, dispatcher());
  PressureWatcherForTest watcher2(false, dispatcher());

  watcher1.Register(Provider());
  watcher2.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 2ul);
  ASSERT_EQ(watcher1.NumChanges(), 1);
  ASSERT_EQ(watcher2.NumChanges(), 1);

  // This should not trigger new notifications as the watchers have not responded to the last one.
  TriggerLevelChange(Level::kNormal);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher1.NumChanges(), 1);
  ASSERT_EQ(watcher2.NumChanges(), 1);
}

TEST_F(PressureNotifierUnitTest, MultipleWatchersDelayedResponse) {
  PressureWatcherForTest watcher1(false, dispatcher());
  PressureWatcherForTest watcher2(false, dispatcher());

  // Signal a specific pressure level here, so that the next one can be different. Delayed callbacks
  // will only come through if the client has missed a level that wasn't the same as the previous
  // one it received a signal for.
  TriggerLevelChange(Level::kNormal);

  watcher1.Register(Provider());
  watcher2.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 2ul);
  ASSERT_EQ(watcher1.NumChanges(), 1);
  ASSERT_EQ(watcher2.NumChanges(), 1);

  // This should not trigger new notifications as the watchers have not responded to the last one.
  TriggerLevelChange(Level::kWarning);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher1.NumChanges(), 1);
  ASSERT_EQ(watcher2.NumChanges(), 1);

  // Respond to the last message. This should send new notifications to the watchers.
  watcher1.Respond();
  watcher2.Respond();
  RunLoopUntilIdle();
  ASSERT_EQ(watcher1.NumChanges(), 2);
  ASSERT_EQ(watcher2.NumChanges(), 2);
}

TEST_F(PressureNotifierUnitTest, MultipleWatchersMixedResponse) {
  // Set up watcher1 to not respond immediately, and watcher2 to respond immediately.
  PressureWatcherForTest watcher1(false, dispatcher());
  PressureWatcherForTest watcher2(true, dispatcher());

  // Signal a specific pressure level here, so that the next one can be different. Delayed callbacks
  // will only come through if the client has missed a level that wasn't the same as the previous
  // one it received a signal for.
  TriggerLevelChange(Level::kNormal);

  watcher1.Register(Provider());
  watcher2.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 2ul);
  ASSERT_EQ(watcher1.NumChanges(), 1);
  ASSERT_EQ(watcher2.NumChanges(), 1);

  // Trigger pressure level change.
  TriggerLevelChange(Level::kWarning);
  RunLoopUntilIdle();
  // Since watcher1 did not respond to the previous change, it will not see this change.
  ASSERT_EQ(watcher1.NumChanges(), 1);
  // Since watcher2 responded to the previous change, it will see it.
  ASSERT_EQ(watcher2.NumChanges(), 2);

  // watcher1 responds now.
  watcher1.Respond();
  RunLoopUntilIdle();
  // watcher1 sees the previous change now.
  ASSERT_EQ(watcher1.NumChanges(), 2);
  ASSERT_EQ(watcher2.NumChanges(), 2);
}

TEST_F(PressureNotifierUnitTest, ReleaseWatcherNoPendingCallback) {
  PressureWatcherForTest watcher(true, dispatcher());

  watcher.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 1ul);
  ASSERT_EQ(watcher.NumChanges(), 1);

  // Trigger pressure level change, causing another call to OnLevelChanged().
  TriggerLevelChange(Level::kNormal);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 2);

  // Release all registered watchers, so that the watcher is now invalid.
  ReleaseWatchers();
  RunLoopUntilIdle();
  // There were no outstanding callbacks, so ReleaseWatchers() sould have freed all watchers.
  ASSERT_EQ(GetWatcherCount(), 0ul);
}

TEST_F(PressureNotifierUnitTest, ReleaseWatcherPendingCallback) {
  PressureWatcherForTest watcher(false, dispatcher());

  watcher.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 1ul);
  ASSERT_EQ(watcher.NumChanges(), 1);

  // This should not trigger a new notification as the watcher has not responded to the last one.
  TriggerLevelChange(Level::kNormal);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 1);

  // Release all registered watchers, so that the watcher is now invalid.
  ReleaseWatchers();
  RunLoopUntilIdle();
  // Verify that the watcher has not been freed yet, since a callback is outstanding.
  ASSERT_EQ(GetWatcherCount(), 1ul);

  // Respond now. This should free the watcher as well.
  watcher.Respond();
  RunLoopUntilIdle();
  // Verify that the watcher has been freed.
  ASSERT_EQ(GetWatcherCount(), 0ul);
}

TEST_F(PressureNotifierUnitTest, WatcherDoesNotSeeImminentOOM) {
  PressureWatcherForTest watcher(true, dispatcher());

  TriggerLevelChange(Level::kImminentOOM);
  watcher.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 1ul);
  ASSERT_EQ(watcher.NumChanges(), 1);
  // Watcher sees the initial level as Critical even though it was Imminent-OOM.
  ASSERT_EQ(watcher.LastLevel(), fmp::Level::kCritical);

  TriggerLevelChange(Level::kWarning);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 2);
  // Non Imminent-OOM levels come through as expected.
  ASSERT_EQ(watcher.LastLevel(), fmp::Level::kWarning);

  TriggerLevelChange(Level::kImminentOOM);
  RunLoopUntilIdle();
  // Watcher does not see this change as the PressureNotifier won't signal it.
  ASSERT_EQ(watcher.NumChanges(), 2);
  ASSERT_EQ(watcher.LastLevel(), fmp::Level::kWarning);
}

TEST_F(PressureNotifierUnitTest, DelayedWatcherDoesNotSeeImminentOOM) {
  // Don't send responses right away, but wait for the delayed callback to come through.
  PressureWatcherForTest watcher(false, dispatcher());

  TriggerLevelChange(Level::kNormal);
  watcher.Register(Provider());
  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 1ul);
  ASSERT_EQ(watcher.NumChanges(), 1);
  ASSERT_EQ(watcher.LastLevel(), fmp::Level::kNormal);

  // This should not trigger a new notification as the watcher has not responded to the last one.
  TriggerLevelChange(Level::kImminentOOM);
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 1);
  ASSERT_EQ(watcher.LastLevel(), fmp::Level::kNormal);

  // Respond to the last message. This should send a new notification to the watcher.
  watcher.Respond();
  RunLoopUntilIdle();
  ASSERT_EQ(watcher.NumChanges(), 2);
  // Watcher will see the delayed Imminent-OOM level as Critical.
  ASSERT_EQ(watcher.LastLevel(), fmp::Level::kCritical);
}

TEST_F(PressureNotifierUnitTest, CrashReportOnCritical) {
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());
}

TEST_F(PressureNotifierUnitTest, NoCrashReportOnWarning) {
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kWarning);

  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());
}

TEST_F(PressureNotifierUnitTest, NoCrashReportOnNormal) {
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kNormal);

  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());
}

TEST_F(PressureNotifierUnitTest, NoCrashReportOnCriticalToWarning) {
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kWarning);

  // No new crash reports for Critical -> Warning
  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  // No new crash reports for Warning -> Critical
  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());
}

TEST_F(PressureNotifierUnitTest, CrashReportOnCriticalToNormal) {
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kNormal);

  // No new crash reports for Critical -> Normal, but can generate future reports.
  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  // New crash report generated on Critical, but cannot generate any more reports.
  ASSERT_EQ(num_crash_reports(), 2ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());
}

TEST_F(PressureNotifierUnitTest, CrashReportOnCriticalAfterLong) {
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kWarning);

  // No new crash reports for Critical -> Warning
  ASSERT_EQ(num_crash_reports(), 1ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  // Crash report interval set to zero. Can generate new reports.
  SetCrashReportInterval(0);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  // New crash report generated on Critical, and can generate future reports.
  ASSERT_EQ(num_crash_reports(), 2ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  // Crash report interval set to 30 mins. Cannot generate new reports.
  SetCrashReportInterval(30);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kWarning);

  // No new crash reports for Critical -> Warning
  ASSERT_EQ(num_crash_reports(), 2ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());

  TriggerLevelChange(Level::kCritical);

  // No new crash reports for Warning -> Critical
  ASSERT_EQ(num_crash_reports(), 2ul);
  ASSERT_FALSE(CanGenerateNewCriticalCrashReports());
}

TEST_F(PressureNotifierUnitTest, DoNotSendCriticalPressureCrashReport) {
  SetUpNewPressureNotifier(false /*send_critical_pressure_crash_reports*/);
  ASSERT_EQ(num_crash_reports(), 0ul);
  ASSERT_TRUE(CanGenerateNewCriticalCrashReports());

  // Cannot write critical crash reports.
  TriggerLevelChange(Level::kCritical);
  ASSERT_EQ(num_crash_reports(), 0ul);

  // Cannot write imminent-OOM crash reports.
  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 0ul);
}

TEST_F(PressureNotifierUnitTest, CrashReportOnOOM) {
  ASSERT_EQ(num_crash_reports(), 0ul);

  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 1ul);
}

TEST_F(PressureNotifierUnitTest, RepeatedCrashReportOnOOM) {
  ASSERT_EQ(num_crash_reports(), 0ul);

  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 1ul);

  // Can generate repeated imminent-OOM crash reports (unlike critical ones).
  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 2ul);

  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 3ul);
}

TEST_F(PressureNotifierUnitTest, CrashReportOnCriticalAndOOM) {
  ASSERT_EQ(num_crash_reports(), 0ul);

  // Critical crash reports don't affect imminent-OOM reports.
  TriggerLevelChange(Level::kCritical);
  ASSERT_EQ(num_crash_reports(), 1ul);

  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 2ul);
}

TEST_F(PressureNotifierUnitTest, CrashReportOnOOMAndCritical) {
  ASSERT_EQ(num_crash_reports(), 0ul);

  // Imminent-OOM crash reports don't affect critical reports.
  TriggerLevelChange(Level::kImminentOOM);
  ASSERT_EQ(num_crash_reports(), 1ul);

  TriggerLevelChange(Level::kCritical);
  ASSERT_EQ(num_crash_reports(), 2ul);
}

TEST_F(PressureNotifierUnitTest, SimulatePressure) {
  // Scoped so that the Watcher gets deleted. We can then verify that the Provider has no watchers
  // remaining.
  {
    PressureWatcherForTest watcher1(true, dispatcher());
    PressureWatcherForTest watcher2(true, dispatcher());

    // Registering the watchers should call OnLevelChanged().
    watcher1.Register(Provider());
    watcher2.Register(Provider());
    RunLoopUntilIdle();
    ASSERT_EQ(GetWatcherCount(), 2ul);
    ASSERT_EQ(watcher1.NumChanges(), 1);
    ASSERT_EQ(watcher2.NumChanges(), 1);

    // Start the fuchsia.memory.debug.MemoryPressure service.
    SetupMemDebugService();

    // Simulate pressure via the fuchsia.memory.debug.MemoryPressure service.
    TestSimulatedPressure(fmp::Level::kCritical);
    RunLoopUntilIdle();
    // Verify that watchers saw the change.
    ASSERT_EQ(watcher1.NumChanges(), 2);
    ASSERT_EQ(watcher2.NumChanges(), 2);

    TestSimulatedPressure(fmp::Level::kWarning);
    RunLoopUntilIdle();
    ASSERT_EQ(watcher1.NumChanges(), 3);
    ASSERT_EQ(watcher2.NumChanges(), 3);

    // Repeating the same level should count too.
    TestSimulatedPressure(fmp::Level::kWarning);
    RunLoopUntilIdle();
    ASSERT_EQ(watcher1.NumChanges(), 4);
    ASSERT_EQ(watcher2.NumChanges(), 4);

    TestSimulatedPressure(fmp::Level::kNormal);
    RunLoopUntilIdle();
    ASSERT_EQ(watcher1.NumChanges(), 5);
    ASSERT_EQ(watcher2.NumChanges(), 5);

    // Verify that simulated signals don't affect the real signaling mechanism.
    TriggerLevelChange(Level::kNormal);
    RunLoopUntilIdle();
    ASSERT_EQ(watcher1.NumChanges(), 6);
    ASSERT_EQ(watcher2.NumChanges(), 6);
  }

  RunLoopUntilIdle();
  ASSERT_EQ(GetWatcherCount(), 0ul);
}
}  // namespace
}  // namespace pressure_signaler::test
