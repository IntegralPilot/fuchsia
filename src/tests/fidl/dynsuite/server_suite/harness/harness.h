// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_HARNESS_HARNESS_H_
#define SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_HARNESS_HARNESS_H_

#include <fidl/fidl.serversuite/cpp/fidl.h>
#include <fidl/fidl.serversuite/cpp/markers.h>
#include <fidl/fidl.serversuite/cpp/natural_types.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/client.h>

#include <optional>

#include <gtest/gtest.h>

#include "src/tests/fidl/dynsuite/channel_util/channel.h"

namespace server_suite {

// Defines a server test using GoogleTest.
// Use {CLOSED,AJAR,OPEN}_SERVER_TEST for the {Closed,Ajar,Open}Target protocol.
// (1) test_number must be a fidl.serversuite/Test enum member value.
// (2) test_name must be the enum member's name in UpperCamelCase.
// We include (1) to make it easier to correlate logs when a test fails.
#define CLOSED_SERVER_TEST(test_number, test_name) \
  SERVER_TEST_IMPL(test_number, test_name, fidl_serversuite::AnyTarget::Tag::kClosed)
#define AJAR_SERVER_TEST(test_number, test_name) \
  SERVER_TEST_IMPL(test_number, test_name, fidl_serversuite::AnyTarget::Tag::kAjar)
#define OPEN_SERVER_TEST(test_number, test_name) \
  SERVER_TEST_IMPL(test_number, test_name, fidl_serversuite::AnyTarget::Tag::kOpen)

#define SERVER_TEST_IMPL(test_number, test_name, target_kind)                                 \
  static_assert((test_number) == static_cast<uint32_t>(fidl_serversuite::Test::k##test_name), \
                "In SERVER_TEST macro: test number " #test_number                             \
                " does not match test name \"" #test_name "\"");                              \
  struct ServerTest_##test_number : public ServerTest {                                       \
    explicit ServerTest_##test_number()                                                       \
        : ServerTest(fidl_serversuite::Test::k##test_name, target_kind) {}                    \
  };                                                                                          \
  /* NOLINTNEXTLINE(google-readability-avoid-underscore-in-googletest-name) */                \
  TEST_F(ServerTest_##test_number, test_name)

// Skips the test if Runner.GetVersion() is older than min_version.
#define SKIP_IF_VERSION_OLDER_THAN(min_version)                                          \
  if (runner_version() < (min_version)) {                                                \
    GTEST_SKIP() << "Skipping because Runner.GetVersion() returned " << runner_version() \
                 << " which is older than the minimum version " << (min_version);        \
  }

// Asserts that a fit::result<fidl::Error> is ok. Useful for Runner FIDL calls.
#define ASSERT_RESULT_OK(result)                                              \
  {                                                                           \
    auto the_result = (result);                                               \
    ASSERT_TRUE(the_result.is_ok()) << "Error: " << the_result.error_value(); \
  }

// X macro for Runner event names.
#define X_RUNNER_EVENT_NAMES               \
  X(OnTeardown)                            \
  X(OnReceivedUnknownMethod)               \
  X(OnReceivedClosedTargetOneWayNoPayload) \
  X(OnReceivedOpenTargetStrictOneWay)      \
  X(OnReceivedOpenTargetFlexibleOneWay)

// Enum of events on the Runner protocol.
enum struct RunnerEvent : uint8_t {
#define X(name) k##name,
  X_RUNNER_EVENT_NAMES
#undef X
};

// Variant of payload types for RunnerEvent.
using RunnerEventPayload = std::variant<std::monostate, fidl_serversuite::TeardownReason,
                                        fidl_serversuite::UnknownMethodInfo>;

// Asserts that the harness receives the given RunnerEvent.
#define ASSERT_RUNNER_EVENT(expected_event)     \
  {                                             \
    SCOPED_TRACE("ASSERT_RUNNER_EVENT failed"); \
    AssertRunnerEventImpl(expected_event);      \
    if (HasFatalFailure())                      \
      return;                                   \
  }

// Asserts that the Target server bindings get torn down. This tests both the
// external behavior (observed PEER_CLOSED on the client end) and the internal
// behavior (Runner sent OnTeardown event reporting the reason).
//
// By default, the test fixture automatically does this at the end of each test:
//
//     client_end().reset();
//     ASSERT_SERVER_TEARDOWN(fidl_serversuite::TeardownReason::kPeerClosed);
//
// It won't do that if the test calls ASSERT_SERVER_TEARDOWN manually.
#define ASSERT_SERVER_TEARDOWN(expected_reason)    \
  {                                                \
    SCOPED_TRACE("ASSERT_SERVER_TEARDOWN failed"); \
    AssertServerTeardownImpl(expected_reason);     \
    if (HasFatalFailure())                         \
      return;                                      \
  }

// The test fixture used by the SERVER_TEST macros.
class ServerTest : public testing::Test {
 public:
  explicit ServerTest(fidl_serversuite::Test test, fidl_serversuite::AnyTarget::Tag target_kind)
      : test_(test), target_kind_(target_kind) {}

  void SetUp() override;
  void TearDown() override;

  // Returns the sync client for the Runner.
  const fidl::SyncClient<fidl_serversuite::Runner>& runner() { return runner_; }

  // Returns the client end of the Target channel.
  channel_util::Channel& client_end() { return client_end_; }

  // This can be accessed after ASSERT_RUNNER_EVENT.
  const RunnerEventPayload& runner_event_payload() { return runner_event_payload_; }

  // Do not call these directly. Use the macros instead.
  uint64_t runner_version() { return runner_version_.value(); }
  void AssertRunnerEventImpl(RunnerEvent expected_event);
  void AssertServerTeardownImpl(fidl_serversuite::TeardownReason expected_reason);

 private:
  fidl_serversuite::Test test_;
  fidl_serversuite::AnyTarget::Tag target_kind_;
  fidl::SyncClient<fidl_serversuite::Runner> runner_;
  std::optional<uint64_t> runner_version_;
  channel_util::Channel client_end_;
  bool asserted_teardown_ = false;
  RunnerEventPayload runner_event_payload_;
};

}  // namespace server_suite

#endif  // SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_HARNESS_HARNESS_H_
