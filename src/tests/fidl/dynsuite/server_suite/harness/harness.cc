// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "harness.h"

#include <fidl/fidl.serversuite/cpp/common_types.h>
#include <fidl/fidl.serversuite/cpp/markers.h>
#include <fidl/fidl.serversuite/cpp/natural_ostream.h>
#include <fidl/fidl.serversuite/cpp/natural_types.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/object_traits.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/tests/fidl/dynsuite/channel_util/channel.h"

namespace server_suite {

namespace {

const char* RunnerEventName(RunnerEvent event) {
  switch (event) {
#define X(name)              \
  case RunnerEvent::k##name: \
    return #name;
    X_RUNNER_EVENT_NAMES
#undef X
  }
}

}  // namespace

void ServerTest::SetUp() {
  // Connect to the Runner.
  {
    auto client_end = component::Connect<fidl_serversuite::Runner>();
    ASSERT_OK(client_end.status_value());
    runner_ = fidl::SyncClient<fidl_serversuite::Runner>(std::move(*client_end));
  }

  // Get the version.
  {
    auto result = runner_->GetVersion();
    ASSERT_TRUE(result.is_ok()) << "Runner.GetVersion() failed: " << result.error_value();
    runner_version_ = result->version();
  }

  // TODO(https://fxbug.dev/42082237): Remove after updating Dart.
  SKIP_IF_VERSION_OLDER_THAN(1);

  // Start the Target server.
  {
    zx::channel client_end, server_end;
    ASSERT_OK(zx::channel::create(0, &client_end, &server_end));
    auto any_target = [&]() {
      switch (target_kind_) {
        case fidl_serversuite::AnyTarget::Tag::kClosed:
          return fidl_serversuite::AnyTarget::WithClosed(
              fidl::ServerEnd<fidl_serversuite::ClosedTarget>(std::move(server_end)));
        case fidl_serversuite::AnyTarget::Tag::kAjar:
          return fidl_serversuite::AnyTarget::WithAjar(
              fidl::ServerEnd<fidl_serversuite::AjarTarget>(std::move(server_end)));
        case fidl_serversuite::AnyTarget::Tag::kOpen:
          return fidl_serversuite::AnyTarget::WithOpen(
              fidl::ServerEnd<fidl_serversuite::OpenTarget>(std::move(server_end)));
      }
    }();
    auto result = runner_->Start({test_, std::move(any_target)});
    if (result.is_error() && result.error_value().is_domain_error() &&
        result.error_value().domain_error() == fidl_serversuite::StartError::kTestDisabled) {
      GTEST_SKIP() << "Skipping because Runner.Start() returned TEST_DISABLED";
    }
    ASSERT_TRUE(result.is_ok()) << "Runner.Start() failed: " << result.error_value();
    client_end_ = channel_util::Channel(std::move(client_end));
  }
}

void ServerTest::TearDown() {
  if (HasFailure() || IsSkipped()) {
    return;
  }
  if (!asserted_teardown_) {
    ASSERT_FALSE(client_end_.is_signal_present(ZX_CHANNEL_PEER_CLOSED))
        << "Target channel is unexpectedly closed at the end of the test";
    ASSERT_FALSE(client_end_.is_signal_present(ZX_CHANNEL_READABLE))
        << "Target channel is unexpectedly readable at the end of the test";
    client_end_.reset();
    ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kPeerClosed);
  }
  auto result = runner_->CheckAlive();
  ASSERT_TRUE(result.is_ok()) << "Runner.CheckAlive() failed! The Runner may have crashed. Error: "
                              << result.error_value();
}

class RunnerEventHandler : public fidl::SyncEventHandler<fidl_serversuite::Runner> {
 public:
  void OnTeardown(fidl_serversuite::RunnerOnTeardownRequest& request) override {
    event_ = RunnerEvent::kOnTeardown;
    payload_ = request.reason();
  }
  void OnReceivedUnknownMethod(fidl_serversuite::UnknownMethodInfo& info) override {
    event_ = RunnerEvent::kOnReceivedUnknownMethod;
    payload_ = info;
  }
  void OnReceivedClosedTargetOneWayNoPayload() override {
    event_ = RunnerEvent::kOnReceivedClosedTargetOneWayNoPayload;
  }
  void OnReceivedOpenTargetStrictOneWay() override {
    event_ = RunnerEvent::kOnReceivedOpenTargetStrictOneWay;
  }
  void OnReceivedOpenTargetFlexibleOneWay() override {
    event_ = RunnerEvent::kOnReceivedOpenTargetFlexibleOneWay;
  }

  RunnerEventPayload payload_;
  std::optional<RunnerEvent> event_;
};

void ServerTest::AssertRunnerEventImpl(RunnerEvent expected_event) {
  auto deadline = zx::deadline_after(channel_util::kWaitTimeout);
  zx_status_t status = runner_.client_end().channel().wait_one(
      ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, deadline, nullptr);
  ASSERT_OK(status) << "ASSERT_RUNNER_EVENT: Failed waiting for a Runner event";
  RunnerEventHandler handler;
  auto result = runner_.HandleOneEvent(handler);
  ASSERT_TRUE(result.ok()) << "ASSERT_RUNNER_EVENT: HandleOneEvent failed: " << result;
  // Assert on strings instead of integer values to make failure messages more useful.
  auto actual_event_name = RunnerEventName(handler.event_.value());
  auto expected_event_name = RunnerEventName(expected_event);
  ASSERT_STREQ(actual_event_name, expected_event_name);
  runner_event_payload_ = handler.payload_;
}

void ServerTest::AssertServerTeardownImpl(fidl_serversuite::TeardownReason expected_reason) {
  ASSERT_EQ(__builtin_popcount(static_cast<uint32_t>(expected_reason)), 1)
      << "ASSERT_SERVER_TEARDOWN: expected_reason must only have 1 bit set";
  ASSERT_FALSE(asserted_teardown_) << "Called ASSERT_SERVER_TEARDOWN more than once?";
  asserted_teardown_ = true;
  bool we_closed = !client_end_.get().is_valid();
  bool they_should_close = expected_reason != fidl_serversuite::TeardownReason::kPeerClosed;
  ASSERT_NE(we_closed, they_should_close)
      << "ASSERT_SERVER_TEARDOWN: Test's teardown expectation does not make sense";
  if (they_should_close) {
    ASSERT_OK(client_end_.wait_for_signal(ZX_CHANNEL_PEER_CLOSED))
        << "ASSERT_SERVER_TEARDOWN: Failed waiting for the target channel to close";
    ASSERT_FALSE(client_end_.is_signal_present(ZX_CHANNEL_READABLE))
        << "ASSERT_SERVER_TEARDOWN: Target channel is unexpectedly readable at the end of the test";
  }
  ASSERT_RUNNER_EVENT(RunnerEvent::kOnTeardown);
  auto actual_reason = std::get<fidl_serversuite::TeardownReason>(runner_event_payload_);
  if (!(actual_reason & expected_reason)) {
    FAIL() << "ASSERT_SERVER_TEARDOWN:\n  Expected teardown reason: "
           << fidl::ostream::Formatted(expected_reason)
           << "\n  Actual teardown reason: " << fidl::ostream::Formatted(actual_reason);
  }
}

}  // namespace server_suite
