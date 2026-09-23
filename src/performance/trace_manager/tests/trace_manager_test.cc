// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/tests/trace_manager_test.h"

#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <utility>

namespace tracing {
namespace test {

TraceManagerTest::TraceManagerTest() : executor_(dispatcher()) {}

void TraceManagerTest::SetUp() {
  TestLoopFixture::SetUp();

  Config config;
  ASSERT_TRUE(config.ReadFrom(kConfigFile));

  trace_manager_ = std::make_unique<TraceManager>(std::move(config), executor_);

  context_provider_.context()->outgoing()->AddProtocol<fuchsia_tracing_provider::Registry>(
      trace_manager_->GetRegistryHandler());
  context_provider_.context()->outgoing()->AddProtocol<fuchsia_tracing_controller::Provisioner>(
      trace_manager_->GetProvisionerHandler());
}

void TraceManagerTest::TearDown() {
  fake_provider_bindings_.clear();
  trace_manager_.reset();
  TestLoopFixture::TearDown();
}

void TraceManagerTest::ConnectToProvisionerService() {
  FX_LOGS(DEBUG) << "ConnectToProvisionerService";
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_tracing_controller::Provisioner>::Create();
  context_provider().public_service_directory()->Connect(
      fidl::DiscoverableProtocolName<fuchsia_tracing_controller::Provisioner>,
      server_end.TakeChannel());
  provisioner_ = fidl::Client(std::move(client_end), dispatcher());
}

void TraceManagerTest::DisconnectFromControllerService() {
  FX_LOGS(DEBUG) << "DisconnectFromControllerService";
  controller_ = {};
}

bool TraceManagerTest::AddFakeProvider(zx_koid_t pid, const std::string& name,
                                       FakeProvider** out_provider) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::Registry>::Create();
  context_provider().public_service_directory()->Connect(
      fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>, server_end.TakeChannel());
  fidl::Client<fuchsia_tracing_provider::Registry> registry(std::move(client_end), dispatcher());

  auto provider_impl = std::make_unique<FakeProvider>(pid, name);
  auto provider = std::make_unique<FakeProviderBinding>(std::move(provider_impl));

  auto provider_client = provider->NewBinding(dispatcher());
  if (!provider_client.is_valid()) {
    return false;
  }

  auto result = registry->RegisterProvider({{std::move(provider_client), pid, name}});
  if (result.is_error()) {
    return false;
  }
  if (out_provider) {
    *out_provider = provider->provider.get();
  }
  fake_provider_bindings_.push_back(std::move(provider));
  return true;
}

bool TraceManagerTest::AddFakeProviderV2(zx_koid_t pid, const std::string& name,
                                         FakeProviderV2** out_provider) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_provider::Registry>::Create();
  context_provider().public_service_directory()->Connect(
      fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>, server_end.TakeChannel());
  fidl::Client<fuchsia_tracing_provider::Registry> registry(std::move(client_end), dispatcher());

  auto provider_impl = std::make_unique<FakeProviderV2>(pid, name);
  auto provider = std::make_unique<FakeProviderV2Binding>(std::move(provider_impl));

  auto provider_client = provider->NewBinding(dispatcher());
  if (!provider_client.is_valid()) {
    return false;
  }

  auto result = registry->RegisterV2({{std::move(provider_client), pid, name}});
  if (result.is_error()) {
    return false;
  }
  if (out_provider) {
    *out_provider = provider->provider.get();
  }
  fake_provider_bindings_.push_back(std::move(provider));
  return true;
}

TraceManagerTest::SessionState TraceManagerTest::GetSessionState() const {
  if (trace_manager()->session()) {
    switch (trace_manager()->session()->state()) {
#define TRANSLATE_STATE(state)     \
  case TraceSession::State::state: \
    return SessionState::state;
      TRANSLATE_STATE(kInitialized);
      TRANSLATE_STATE(kStarting);
      TRANSLATE_STATE(kStarted);
      TRANSLATE_STATE(kStopping);
      TRANSLATE_STATE(kStopped);
      TRANSLATE_STATE(kTerminating);
#undef TRANSLATE_STATE
    }
  }
  return SessionState::kNonexistent;
}

// static
fuchsia_tracing_controller::TraceConfig TraceManagerTest::GetDefaultTraceConfig() {
  std::vector<std::string> categories{kTestUmbrellaCategory};
  fuchsia_tracing_controller::TraceConfig config;
  config.categories(std::move(categories));
  config.buffer_size_megabytes_hint(kDefaultBufferSizeMegabytes);
  config.start_timeout_milliseconds(kDefaultStartTimeoutMilliseconds);
  config.buffering_mode(fuchsia_tracing::BufferingMode::kOneshot);
  return config;
}

void TraceManagerTest::InitializeSessionWorker(fuchsia_tracing_controller::TraceConfig config,
                                               bool* success) {
  zx::socket our_socket, their_socket;
  zx_status_t status = zx::socket::create(0u, &our_socket, &their_socket);
  ASSERT_EQ(status, ZX_OK);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_tracing_controller::Session>::Create();
  auto result = provisioner_->InitializeTracing(
      {{std::move(server_end), std::move(config), std::move(their_socket)}});
  ASSERT_TRUE(result.is_ok());

  controller_ = fidl::Client(std::move(client_end), dispatcher(), this);

  RunLoopUntilIdle();
  FX_LOGS(DEBUG) << "Loop done, expecting session initialized";
  ASSERT_EQ(GetSessionState(), SessionState::kInitialized);

  // Run one more time to finish up provider initialization. This happens
  // after the session transitions to the initialized state, but after all
  // providers have been told to initialize. Since everything is happening
  // on one thread, we can assume that when the loop is idle all registered
  // providers have initialized.
  // This doesn't run forever as there's no session state change involved.
  RunLoopUntilIdle();

  // The counts always have a fixed value here.
  VerifyCounts(0, 0);

  destination_ = std::move(our_socket);

  *success = true;
}

bool TraceManagerTest::InitializeSession(fuchsia_tracing_controller::TraceConfig config) {
  bool success{};
  FX_LOGS(DEBUG) << "Initializing session";
  InitializeSessionWorker(std::move(config), &success);
  if (success) {
    FX_LOGS(DEBUG) << "Session initialized";
  }
  return success;
}

// static
fuchsia_tracing_controller::StartOptions TraceManagerTest::GetDefaultStartOptions() {
  std::vector<std::string> additional_categories{};
  fuchsia_tracing_controller::StartOptions options;
  options.buffer_disposition(fuchsia_tracing::BufferDisposition::kRetain);
  options.additional_categories(std::move(additional_categories));
  return options;
}

void TraceManagerTest::BeginStartSession(fuchsia_tracing_controller::StartOptions options) {
  FX_LOGS(DEBUG) << "Starting session";

  MarkBeginOperation();

  start_state_.start_completed = false;
  controller_->StartTracing({{std::move(options)}})
      .ThenExactlyOnce(
          [this](fidl::Result<fuchsia_tracing_controller::Session::StartTracing>& result) {
            start_state_.start_completed = true;
            if (result.is_error()) {
              if (result.error_value().is_domain_error()) {
                start_state_.start_result = fit::error(result.error_value().domain_error());
              } else {
                start_state_.start_result =
                    fit::error(fuchsia_tracing_controller::StartError::kNotInitialized);
              }
            } else {
              start_state_.start_result = fit::ok();
            }
          });

  RunLoopUntilIdle();
  // The loop will exit for the transition to kStarting.
}

bool TraceManagerTest::FinishStartSession() {
  // If there are no tracees then it will also subsequently transition to
  // kStarted before the loop exits. If there are tracees then we need to
  // wait for them to start.
  if (fake_provider_bindings().size() > 0) {
    FX_LOGS(DEBUG) << "Loop done, expecting session starting";
    SessionState state = GetSessionState();
    EXPECT_EQ(state, SessionState::kStarting);
    if (state != SessionState::kStarting) {
      return false;
    }

    // Make sure all providers are marked kStarting.
    // The loop exits when we transition to kStarting, but providers won't have
    // processed their Start requests yet.
    RunLoopUntilIdle();

    MarkAllProvidersStarted();
    // Wait until all providers are started.
    RunLoopUntilIdle();
  }

  // The loop will exit for the transition to kStarted.
  FX_LOGS(DEBUG) << "Loop done, expecting all providers started";
  SessionState state = GetSessionState();
  EXPECT_EQ(state, SessionState::kStarted);
  if (state != SessionState::kStarted) {
    return false;
  }

  // Run the loop one more time to ensure we pick up the result.
  // Remember the loop prematurely exits on session state changes.
  RunLoopUntilIdle();
  EXPECT_TRUE(start_state_.start_completed);
  if (!start_state_.start_completed) {
    return false;
  }
  EXPECT_TRUE(start_state_.start_result.is_ok());

  FX_LOGS(DEBUG) << "Session started";
  return true;
}

bool TraceManagerTest::StartSession(fuchsia_tracing_controller::StartOptions options) {
  BeginStartSession(std::move(options));
  return FinishStartSession();
}

// static
fuchsia_tracing_controller::StopOptions TraceManagerTest::GetDefaultStopOptions() {
  fuchsia_tracing_controller::StopOptions options;
  options.write_results(true);
  return options;
}

void TraceManagerTest::BeginStopSession(fuchsia_tracing_controller::StopOptions options) {
  FX_LOGS(DEBUG) << "Stopping session";

  MarkBeginOperation();

  stop_state_.stop_completed = false;
  controller_->StopTracing({{std::move(options)}})
      .ThenExactlyOnce(
          [this](fidl::Result<fuchsia_tracing_controller::Session::StopTracing>& result) {
            stop_state_.stop_completed = true;
            if (result.is_error()) {
              if (result.error_value().is_domain_error()) {
                stop_state_.stop_result = fit::error(result.error_value().domain_error());
              } else {
                stop_state_.stop_result =
                    fit::error(fuchsia_tracing_controller::StopError::kNotInitialized);
              }
            } else {
              stop_state_.stop_result = fit::ok(std::move(result.value()));
            }
            // We need to run the loop one last time to pick up the result.
            // Be sure to exit it now that we have the result.
            QuitLoop();
          });

  RunLoopUntilIdle();
  // The loop will exit for the transition to kStopping.
}

bool TraceManagerTest::FinishStopSession() {
  // If there are no tracees then it will also subsequently transition to
  // kStopped before the loop exits. If there are tracees then we need to
  // wait for them to stop.
  if (fake_provider_bindings().size() > 0) {
    FX_LOGS(DEBUG) << "Loop done, expecting session stopping";
    SessionState state = GetSessionState();
    EXPECT_EQ(state, SessionState::kStopping);
    if (state != SessionState::kStopping) {
      return false;
    }

    // Make sure all providers are marked kStopping.
    // The loop exits when we transition to kStopping, but providers won't have
    // processed their Stop requests yet.
    RunLoopUntilIdle();

    MarkAllProvidersStopped();
    // Wait until all providers are stopped.
    RunLoopUntilIdle();
  }

  // The loop will exit for the transition to kStopped.
  FX_LOGS(DEBUG) << "Loop done, expecting session stopped";
  SessionState state = GetSessionState();
  EXPECT_EQ(state, SessionState::kStopped);
  if (state != SessionState::kStopped) {
    return false;
  }

  // Run one more time to ensure we pick up the stop result.
  RunLoopUntilIdle();
  EXPECT_TRUE(stop_state_.stop_completed);
  if (!stop_state_.stop_completed) {
    return false;
  }

  FX_LOGS(DEBUG) << "Session stopped";
  return true;
}

bool TraceManagerTest::StopSession(fuchsia_tracing_controller::StopOptions options) {
  BeginStopSession(std::move(options));
  return FinishStopSession();
}

void TraceManagerTest::BeginTerminateSession() {
  FX_LOGS(DEBUG) << "Terminating session";

  MarkBeginOperation();

  // Disconnecting from the controller will terminate the session
  DisconnectFromControllerService();

  RunLoopUntilIdle();
  // The loop will exit for the transition to kTerminating.
  // Note: If there are no providers then the state will transition again
  // to kNonexistent(== "terminated") before the loop exits.
}

bool TraceManagerTest::FinishTerminateSession() {
  // If there are no tracees then it will also subsequently transition to
  // kTerminated before the loop exits. If there are tracees then we need to
  // wait for them to terminate.
  if (fake_provider_bindings().size() > 0) {
    FX_LOGS(DEBUG) << "Loop done, expecting session terminating";
    SessionState state = GetSessionState();
    state = GetSessionState();
    EXPECT_EQ(state, SessionState::kTerminating);
    if (state != SessionState::kTerminating) {
      return false;
    }

    // Make sure all providers are marked kTerminating.
    RunLoopUntilIdle();

    MarkAllProvidersTerminated();
    RunLoopUntilIdle();
  }

  FX_LOGS(DEBUG) << "Loop done, expecting session terminated";
  EXPECT_EQ(GetSessionState(), SessionState::kNonexistent);

  return true;
}

bool TraceManagerTest::TerminateSession() {
  BeginTerminateSession();
  return FinishTerminateSession();
}

void TraceManagerTest::MarkAllProvidersStarted() {
  FX_LOGS(DEBUG) << "Marking all providers started";
  for (const auto& binding : fake_provider_bindings_) {
    std::visit([](const auto& p) { p->provider->MarkStarted(); }, binding);
  }
}

void TraceManagerTest::MarkAllProvidersStopped() {
  FX_LOGS(DEBUG) << "Marking all providers stopped";
  for (const auto& binding : fake_provider_bindings_) {
    std::visit([](const auto& p) { p->provider->MarkStopped(); }, binding);
  }
}

void TraceManagerTest::MarkAllProvidersTerminated() {
  FX_LOGS(DEBUG) << "Marking all providers terminated";
  for (const auto& binding : fake_provider_bindings_) {
    std::visit([](const auto& p) { p->provider->MarkTerminated(); }, binding);
  }
}

void TraceManagerTest::VerifyCounts(int expected_start_count, int expected_stop_count) {
  SessionState state{GetSessionState()};
  for (const auto& binding : fake_provider_bindings_) {
    std::visit(
        [state, expected_start_count, expected_stop_count](const auto& p) {
          const std::string& name = p->provider->name();
          EXPECT_EQ(p->provider->initialize_count(), 1) << name;
          EXPECT_EQ(p->provider->start_count(), expected_start_count) << name;
          EXPECT_EQ(p->provider->stop_count(), expected_stop_count) << name;
          if (state != SessionState::kNonexistent) {
            EXPECT_EQ(p->provider->terminate_count(), 0) << name;
          } else {
            EXPECT_EQ(p->provider->terminate_count(), 1) << name;
          }
        },
        binding);
  }
}

// fidl event
void TraceManagerTest::OnSessionStateChange(
    fidl::Event<fuchsia_tracing_controller::Session::OnSessionStateChange>& event) {
  FX_LOGS(DEBUG) << "OnSessionStateChange " << static_cast<uint32_t>(event.state());
  ++on_session_state_change_event_count_;
  last_session_state_event_ = event.state();
}

}  // namespace test
}  // namespace tracing
