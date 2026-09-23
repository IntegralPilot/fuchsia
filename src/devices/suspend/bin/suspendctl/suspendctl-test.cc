// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "suspendctl.h"

#include <fidl/fuchsia.device/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.power.suspend/cpp/markers.h>
#include <fidl/fuchsia.hardware.power.suspend/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.power.suspend/cpp/wire_types.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <sstream>

#include <gtest/gtest.h>

namespace {

class FakeSuspendDevice : public fidl::testing::WireTestBase<suspendctl::fh_suspend::Suspender> {
 public:
  int SuspendCallCount() const { return suspend_call_count_; }
  void SetSuspendCallCount(int count) { suspend_call_count_ = count; }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    // ADD_FAILURE("unexpected call to %s", name.c_str());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  static constexpr zx::duration kDefaultSuspendDuration = zx::sec(5);
  static constexpr zx::duration kDefaultSuspendOverhead = zx::sec(1);
  static constexpr zx::duration kDefaultResumeLatency = zx::usec(100);

  void Suspend(SuspendRequestView request, SuspendCompleter::Sync& completer) override;
  void GetSuspendStates(GetSuspendStatesCompleter::Sync& completer) override;

  int suspend_call_count_ = 0;
};

void FakeSuspendDevice::Suspend(SuspendRequestView request, SuspendCompleter::Sync& completer) {
  suspend_call_count_++;

  if (!request->has_state_index() || request->state_index() != 0) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  fidl::Arena arena;
  auto result = suspendctl::fh_suspend::wire::SuspenderSuspendResponse::Builder(arena)
                    .suspend_duration(kDefaultSuspendDuration.to_nsecs())
                    .suspend_overhead(kDefaultSuspendOverhead.to_nsecs())
                    .Build();

  completer.ReplySuccess(result);
}

void FakeSuspendDevice::GetSuspendStates(GetSuspendStatesCompleter::Sync& completer) {
  fidl::Arena arena;
  auto state = suspendctl::fh_suspend::wire::SuspendState::Builder(arena)
                   .resume_latency(kDefaultResumeLatency.to_nsecs())
                   .Build();
  std::vector<suspendctl::fh_suspend::wire::SuspendState> states = {state};
  auto result = suspendctl::fh_suspend::wire::SuspenderGetSuspendStatesResponse::Builder(arena)
                    .suspend_states(states)
                    .Build();
  completer.ReplySuccess(result);
}

class SuspendctlTest : public testing::Test {
 public:
  void SetUp() override;

 protected:
  fidl::SyncClient<suspendctl::fh_suspend::Suspender> client_;

 private:
  FakeSuspendDevice suspend_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

void SuspendctlTest::SetUp() {
  auto suspend_endpoints = fidl::Endpoints<suspendctl::fh_suspend::Suspender>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(suspend_endpoints.server),
                   static_cast<fidl::WireServer<suspendctl::fh_suspend::Suspender>*>(&suspend_));

  client_ =
      fidl::SyncClient<suspendctl::fh_suspend::Suspender>(std::move(suspend_endpoints.client));
  ASSERT_EQ(loop_.StartThread("suspender-test-fidl-thread"), ZX_OK);
}

TEST_F(SuspendctlTest, TestCallSuspend) {
  std::stringstream discard_cout, discard_cerr;
  int result = suspendctl::DoSuspend(std::move(client_), discard_cout, discard_cerr);
  EXPECT_EQ(result, 0);
}

TEST(SuspendCtlStandaloneTest, TestParseArgs) {
  {
    const std::vector<std::string> kTestArgs = {"suspendctl", "world"};
    auto result = suspendctl::ParseArgs(kTestArgs);
    EXPECT_EQ(result, suspendctl::Action::Error);
  }
  {
    const std::vector<std::string> kTestArgs = {"suspendctl", "help"};
    auto result = suspendctl::ParseArgs(kTestArgs);
    EXPECT_EQ(result, suspendctl::Action::PrintHelp);
  }
  {
    const std::vector<std::string> kTestArgs = {"suspendctl", "suspend"};
    auto result = suspendctl::ParseArgs(kTestArgs);
    EXPECT_EQ(result, suspendctl::Action::Suspend);
  }
  {
    const std::vector<std::string> kTestArgs = {"suspendctl"};
    auto result = suspendctl::ParseArgs(kTestArgs);
    EXPECT_EQ(result, suspendctl::Action::Error);
  }
  {
    const std::vector<std::string> kTestArgs = {"suspendctl", "suspend", "extra"};
    auto result = suspendctl::ParseArgs(kTestArgs);
    EXPECT_EQ(result, suspendctl::Action::Suspend);
  }
}

}  // namespace
