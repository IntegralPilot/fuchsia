// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_TESTS_TRACE_MANAGER_TEST_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_TESTS_TRACE_MANAGER_TEST_H_

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/zx/socket.h>

#include <memory>
#include <string>
#include <utility>
#include <variant>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/performance/trace_manager/tests/fake_provider.h"
#include "src/performance/trace_manager/tests/fake_provider_v2.h"
#include "src/performance/trace_manager/trace_manager.h"

namespace tracing {
namespace test {

class TraceManagerTest : public gtest::TestLoopFixture,
                         public fidl::AsyncEventHandler<fuchsia_tracing_controller::Session> {
 public:
  // |TraceSession| intentionally doesn't have |kTerminated| as that is
  // represented by the session being non-existent. However, it's helpful in
  // tests to have a value to represent this state so we have our own copy of
  // |TraceSession::State|. It is not named |State| to help avoid confusion.
  enum class SessionState {
    // These values are all copies of |TraceSession::State|.
    kReady,
    kInitialized,
    kStarting,
    kStarted,
    kStopping,
    kStopped,
    kTerminating,
    // This is the new value to represent |TraceManager::session_| == nullptr.
    // This isn't called |kTerminated|, though that is what it usually means,
    // because this is also the state before any session has been created.
    kNonexistent,
  };

  static constexpr unsigned kDefaultBufferSizeMegabytes = 1;

  // This is effectively infinite.
  static constexpr unsigned kDefaultStartTimeoutMilliseconds = 3600 * 1000;

  static constexpr char kConfigFile[] = "/pkg/data/tracing.config";

  static constexpr char kTestUmbrellaCategory[] = "test_umbrella";

  static constexpr zx_koid_t kProvider1Pid = 1234;
  static constexpr char kProvider1Name[] = "test-provider1";

  static constexpr zx_koid_t kProvider2Pid = 1235;
  static constexpr char kProvider2Name[] = "test-provider2";

  static constexpr zx_koid_t kProvider3Pid = 1236;
  static constexpr char kProvider3Name[] = "test-provider3";

  static fuchsia_tracing_controller::TraceConfig GetDefaultTraceConfig();
  static fuchsia_tracing_controller::StartOptions GetDefaultStartOptions();
  static fuchsia_tracing_controller::StopOptions GetDefaultStopOptions();

  TraceManagerTest();

  const TraceManager* trace_manager() const { return trace_manager_.get(); }

  sys::testing::ComponentContextProvider& context_provider() { return context_provider_; }

  fidl::Client<fuchsia_tracing_controller::Session>& session_client() { return controller_; }

  fidl::Client<fuchsia_tracing_controller::Provisioner>& provisioner_client() {
    return provisioner_;
  }

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_tracing_controller::Session> metadata) override {}

  int on_session_state_change_event_count() const { return on_session_state_change_event_count_; }
  int begin_session_state_change_event_count() const {
    return begin_session_state_change_event_count_;
  }
  fuchsia_tracing_controller::SessionState last_session_state_event() const {
    return last_session_state_event_;
  }

  using ProviderBinding =
      std::variant<std::unique_ptr<FakeProviderBinding>, std::unique_ptr<FakeProviderV2Binding>>;

  const std::vector<ProviderBinding>& fake_provider_bindings() const {
    return fake_provider_bindings_;
  }

  void ConnectToProvisionerService();
  void DisconnectFromControllerService();

  // The caller must run the loop to complete the registration.
  // If |*out_provider| is non-NULL, a borrowed copy of the pointer is
  // stored there, and is valid for the duration of the test.
  bool AddFakeProvider(zx_koid_t pid, const std::string& name,
                       FakeProvider** out_provider = nullptr);
  bool AddFakeProviderV2(zx_koid_t pid, const std::string& name,
                         FakeProviderV2** out_provider = nullptr);

  // Called from within |TraceManager| on |TraceSession| state changes.
  void OnSessionStateChange();

  // Fetch the session's state.
  SessionState GetSessionState() const;

  // Mark the beginning of an operation.
  void MarkBeginOperation() {
    begin_session_state_change_event_count_ = on_session_state_change_event_count_;
  }

  // Wrappers to simplify the standard operations.
  // These are only to be called at times when they're expected to succeed.
  // E.g., Don't call |TerminateSession()| if session is already terminated,
  // and so on. If you want to perform these operations outside of this
  // constraint, roll your own or use something else.
  // These assume the controller connection is already made.
  // The default session doesn't have a consumer.
  bool InitializeSession(fuchsia_tracing_controller::TraceConfig config = GetDefaultTraceConfig());
  bool TerminateSession();

  // Terminating a session involves two steps: Initiating the termination
  // and waiting for it to terminate. Call these when you want to access
  // intermediate state.
  void BeginTerminateSession();
  // Returns true on success.
  // If |result| is non-NULL the result is stored there.
  bool FinishTerminateSession();

  // Wrappers to simplify |Start,Stop| operations.
  // These are only to be called at times when they're expected to succeed.
  // E.g., Don't call |StartSession*()| if session is already started,
  // and so on. If you want to perform these operations outside of this
  // constraint, roll your own or use something else.
  // These assume the needed interface is already connected.
  // These combine both the "begin" and "finish" operations.
  bool StartSession(fuchsia_tracing_controller::StartOptions options = GetDefaultStartOptions());
  bool StopSession(fuchsia_tracing_controller::StopOptions options = GetDefaultStopOptions());

  // Starting and stopping a session involves two steps: initiating the request
  // and waiting for it to complete. Call these when you want to access
  // intermediate state.
  void BeginStartSession(
      fuchsia_tracing_controller::StartOptions options = GetDefaultStartOptions());
  bool FinishStartSession();
  void BeginStopSession(fuchsia_tracing_controller::StopOptions options = GetDefaultStopOptions());
  bool FinishStopSession();

  // Helpers to advance provider state.
  // These can only be called when all providers are in the immediately
  // preceding state (e.g., kStarting->kStarted).
  void MarkAllProvidersStarted();
  void MarkAllProvidersStopped();
  void MarkAllProvidersTerminated();

  // Helper to verify reception and processing of initialize, start, stop,
  // and terminate fidl requests.
  // This is not intended to be called before |Initialize*|.
  void VerifyCounts(int expected_start_count, int expected_stop_count);

  // Publically accessible copies of test fixture methods.
  void QuitLoop() { gtest::TestLoopFixture::QuitLoop(); }
  void RunLoopUntilIdle() { gtest::TestLoopFixture::RunLoopUntilIdle(); }
  void RunLoopFor(zx::duration duration) {
    gtest::TestLoopFixture::RunLoopFor(std::move(duration));
  }

  void DropSocket() { destination_.reset(); }

 private:
  // For communication between |BeginStart(),FinishStart()|.
  // This value is only valid between those calls.
  struct StartState {
    bool start_completed = false;
    fit::result<fuchsia_tracing_controller::StartError> start_result = fit::ok();
  };

  // For communication between |BeginStop(),FinishStop()|.
  // This value is only valid between those calls.
  struct StopState {
    bool stop_completed = false;
    fit::result<fuchsia_tracing_controller::StopError, fuchsia_tracing_controller::StopResult>
        stop_result = fit::ok(fuchsia_tracing_controller::StopResult{});
  };

  void SetUp() override;
  void TearDown() override;

  // Helper functions to cope with functions calling |ASSERT_*|: they have to
  // return void.
  void InitializeSessionWorker(fuchsia_tracing_controller::TraceConfig config, bool* out_success);

  // Handler for OnSessionStateChange fidl event.
  void OnSessionStateChange(
      fidl::Event<fuchsia_tracing_controller::Session::OnSessionStateChange>& event) override;

  sys::testing::ComponentContextProvider context_provider_;
  std::unique_ptr<TraceManager> trace_manager_;

  // Interfaces to make service requests.
  fidl::Client<fuchsia_tracing_controller::Session> controller_;
  fidl::Client<fuchsia_tracing_controller::Provisioner> provisioner_;

  // Running count of session state changes.
  int on_session_state_change_event_count_ = 0;
  // The value of |on_session_state_change_event_count_| at the start of an
  // operation (e.g., termination). This is used to very cases where multiple
  // state changes happen during an operation
  // (e.g., terminating -> terminated).
  int begin_session_state_change_event_count_ = 0;
  // The last recorded session state, for verification purposes.
  fuchsia_tracing_controller::SessionState last_session_state_event_{};

  async::Executor executor_;
  // Socket for communication with controller.
  zx::socket destination_;

  // Most tests don't care about the intermediate state (the state after the
  // |Begin*Session()| function. But some do. To avoid duplicating a bunch of
  // code to handle both cases we manage the intermediate state here.
  StartState start_state_;
  StopState stop_state_;

  // Containers for provider bindings so that they get cleaned up at the end of the test.
  std::vector<ProviderBinding> fake_provider_bindings_;
};

}  // namespace test
}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_TESTS_TRACE_MANAGER_TEST_H_
