// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/stop_signals.h"

#include <fuchsia/hardware/power/statecontrol/cpp/fidl.h>
#include <fuchsia/process/lifecycle/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fit/function.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/unit_test_fixture.h"

namespace forensics::feedback {
namespace {

using WaitForLifecycleStopTest = UnitTestFixture;
using ::fuchsia::hardware::power::statecontrol::ShutdownAction;
using ::fuchsia::hardware::power::statecontrol::ShutdownWatcher_OnShutdown_Result;

TEST_F(WaitForLifecycleStopTest, BadChannel) {
  async::Executor executor(dispatcher());

  std::optional<Error> error;
  fidl::InterfaceRequest<fuchsia::process::lifecycle::Lifecycle> request;
  executor.schedule_task(
      WaitForLifecycleStop(dispatcher(), std::move(request)).or_else([&error](const Error& e) {
        error = e;
        return fpromise::error();
      }));

  RunLoopUntilIdle();
  EXPECT_EQ(error, Error::kBadValue);
}

TEST_F(WaitForLifecycleStopTest, ClientDisconnects) {
  async::Executor executor(dispatcher());

  std::optional<Error> error;
  fuchsia::process::lifecycle::LifecyclePtr ptr;
  executor.schedule_task(WaitForLifecycleStop(dispatcher(), ptr.NewRequest(dispatcher()))
                             .or_else([&error](const Error& e) {
                               error = e;
                               return fpromise::error();
                             }));

  ptr.Unbind();
  RunLoopUntilIdle();
  EXPECT_EQ(error, Error::kConnectionError);
}

TEST_F(WaitForLifecycleStopTest, ServerDisconnectsOnCallbackExecution) {
  async::Executor executor(dispatcher());

  std::optional<LifecycleStopSignal> signal;
  fuchsia::process::lifecycle::LifecyclePtr ptr;
  executor.schedule_task(WaitForLifecycleStop(dispatcher(), ptr.NewRequest(dispatcher()))
                             .and_then([&signal](LifecycleStopSignal& s) {
                               signal = std::move(s);
                               return fpromise::ok();
                             }));

  ptr->Stop();
  RunLoopUntilIdle();
  EXPECT_TRUE(ptr.is_bound());
  ASSERT_NE(signal, std::nullopt);

  signal->Respond();
  RunLoopUntilIdle();
  EXPECT_FALSE(ptr.is_bound());
}

TEST_F(WaitForLifecycleStopTest, ServerDisconnectsOnCallbackDeletion) {
  async::Executor executor(dispatcher());

  std::optional<LifecycleStopSignal> signal;
  fuchsia::process::lifecycle::LifecyclePtr ptr;
  executor.schedule_task(WaitForLifecycleStop(dispatcher(), ptr.NewRequest(dispatcher()))
                             .and_then([&signal](LifecycleStopSignal& s) {
                               signal = std::move(s);
                               return fpromise::ok();
                             }));

  ptr->Stop();
  RunLoopUntilIdle();
  EXPECT_TRUE(ptr.is_bound());
  ASSERT_NE(signal, std::nullopt);

  signal = std::nullopt;
  RunLoopUntilIdle();
  EXPECT_FALSE(ptr.is_bound());
}

using WaitForRebootReasonTest = UnitTestFixture;

TEST_F(WaitForRebootReasonTest, BadChannel) {
  async::Executor executor(dispatcher());

  std::optional<Error> error;
  fidl::InterfaceRequest<fuchsia::hardware::power::statecontrol::ShutdownWatcher> request;
  executor.schedule_task(
      WaitForShutdownInfo(dispatcher(), std::move(request)).or_else([&error](const Error& e) {
        error = e;
        return fpromise::error();
      }));

  RunLoopUntilIdle();
  EXPECT_EQ(error, Error::kBadValue);
}

TEST_F(WaitForRebootReasonTest, ClientDisconnects) {
  async::Executor executor(dispatcher());

  std::optional<Error> error;
  fuchsia::hardware::power::statecontrol::ShutdownWatcherPtr ptr;
  executor.schedule_task(WaitForShutdownInfo(dispatcher(), ptr.NewRequest(dispatcher()))
                             .or_else([&error](const Error& e) {
                               error = e;
                               return fpromise::error();
                             }));

  ptr.Unbind();
  RunLoopUntilIdle();
  EXPECT_EQ(error, Error::kConnectionError);
}

TEST_F(WaitForRebootReasonTest, ServerDisconnectsOnCallbackExecution) {
  async::Executor executor(dispatcher());

  std::optional<GracefulShutdownInfoSignal> signal;
  fuchsia::hardware::power::statecontrol::ShutdownWatcherPtr ptr;
  executor.schedule_task(WaitForShutdownInfo(dispatcher(), ptr.NewRequest(dispatcher()))
                             .and_then([&signal](GracefulShutdownInfoSignal& s) {
                               signal = std::move(s);
                               return fpromise::ok();
                             }));

  bool called{false};

  fuchsia::hardware::power::statecontrol::ShutdownOptions options;
  std::vector<fuchsia::hardware::power::statecontrol::ShutdownReason> reasons = {
      fuchsia::hardware::power::statecontrol::ShutdownReason::USER_REQUEST};
  options.set_reasons(reasons);
  options.set_action(ShutdownAction::REBOOT);
  ptr->OnShutdown(std::move(options),
                  [&called](const ShutdownWatcher_OnShutdown_Result& result) { called = true; });

  RunLoopUntilIdle();
  EXPECT_TRUE(ptr.is_bound());
  ASSERT_NE(signal, std::nullopt);
  EXPECT_THAT(signal->Reasons(), testing::ElementsAre(GracefulShutdownReason::kUserRequest));

  signal->Respond();
  RunLoopUntilIdle();
  EXPECT_TRUE(called);
  EXPECT_FALSE(ptr.is_bound());
}

TEST_F(WaitForRebootReasonTest, ServerDisconnectsOnCallbackDeletion) {
  async::Executor executor(dispatcher());

  std::optional<GracefulShutdownInfoSignal> signal;
  fuchsia::hardware::power::statecontrol::ShutdownWatcherPtr ptr;
  executor.schedule_task(WaitForShutdownInfo(dispatcher(), ptr.NewRequest(dispatcher()))
                             .and_then([&signal](GracefulShutdownInfoSignal& s) {
                               signal = std::move(s);
                               return fpromise::ok();
                             }));

  bool called{false};

  fuchsia::hardware::power::statecontrol::ShutdownOptions options;
  std::vector<fuchsia::hardware::power::statecontrol::ShutdownReason> reasons = {
      fuchsia::hardware::power::statecontrol::ShutdownReason::USER_REQUEST};
  options.set_reasons(reasons);
  options.set_action(ShutdownAction::REBOOT);
  ptr->OnShutdown(std::move(options),
                  [&called](const ShutdownWatcher_OnShutdown_Result& result) { called = true; });

  RunLoopUntilIdle();
  ASSERT_NE(signal, std::nullopt);
  EXPECT_THAT(signal->Reasons(), testing::ElementsAre(GracefulShutdownReason::kUserRequest));

  signal = std::nullopt;
  RunLoopUntilIdle();
  EXPECT_TRUE(called);
  EXPECT_FALSE(ptr.is_bound());
}

TEST_F(WaitForRebootReasonTest, NoCompletionOnNoAction) {
  async::Executor executor(dispatcher());

  std::optional<GracefulShutdownInfoSignal> signal;
  fuchsia::hardware::power::statecontrol::ShutdownWatcherPtr ptr;
  executor.schedule_task(WaitForShutdownInfo(dispatcher(), ptr.NewRequest(dispatcher()))
                             .and_then([&signal](GracefulShutdownInfoSignal& s) {
                               signal = std::move(s);
                               return fpromise::ok();
                             }));

  bool called{false};

  fuchsia::hardware::power::statecontrol::ShutdownOptions options;
  std::vector<fuchsia::hardware::power::statecontrol::ShutdownReason> reasons = {
      fuchsia::hardware::power::statecontrol::ShutdownReason::USER_REQUEST};
  options.set_reasons(reasons);
  ptr->OnShutdown(std::move(options),
                  [&called](const ShutdownWatcher_OnShutdown_Result& result) { called = true; });

  RunLoopUntilIdle();
  EXPECT_EQ(signal, std::nullopt);
  EXPECT_TRUE(called);
}

struct TestParam {
  std::string test_name;
  ShutdownAction action;
  GracefulShutdownAction expected_action;
};

class WaitForRebootReasonParameterizedTest : public UnitTestFixture,
                                             public testing::WithParamInterface<TestParam> {};

INSTANTIATE_TEST_SUITE_P(WithVariousShutdownActions, WaitForRebootReasonParameterizedTest,
                         ::testing::ValuesIn(std::vector<TestParam>({
                             {
                                 "Poweroff",
                                 ShutdownAction::POWEROFF,
                                 GracefulShutdownAction::kPoweroff,
                             },
                             {
                                 "Reboot",
                                 ShutdownAction::REBOOT,
                                 GracefulShutdownAction::kReboot,
                             },
                             {
                                 "RebootToBootloader",
                                 ShutdownAction::REBOOT_TO_BOOTLOADER,
                                 GracefulShutdownAction::kRebootToBootloader,
                             },
                             {
                                 "RebootToRecovery",
                                 ShutdownAction::REBOOT_TO_RECOVERY,
                                 GracefulShutdownAction::kRebootToRecovery,
                             },
                         })),
                         [](const testing::TestParamInfo<TestParam>& info) {
                           return info.param.test_name;
                         });

TEST_P(WaitForRebootReasonParameterizedTest, CompletionWithExpectedAction) {
  const TestParam& param = GetParam();
  async::Executor executor(dispatcher());

  std::optional<GracefulShutdownInfoSignal> signal;
  fuchsia::hardware::power::statecontrol::ShutdownWatcherPtr ptr;
  executor.schedule_task(WaitForShutdownInfo(dispatcher(), ptr.NewRequest(dispatcher()))
                             .and_then([&signal](GracefulShutdownInfoSignal& s) {
                               signal = std::move(s);
                               return fpromise::ok();
                             }));

  bool called{false};

  fuchsia::hardware::power::statecontrol::ShutdownOptions options;
  std::vector<fuchsia::hardware::power::statecontrol::ShutdownReason> reasons = {
      fuchsia::hardware::power::statecontrol::ShutdownReason::USER_REQUEST};
  options.set_reasons(reasons);
  options.set_action(param.action);
  ptr->OnShutdown(std::move(options),
                  [&called](const ShutdownWatcher_OnShutdown_Result& result) { called = true; });

  RunLoopUntilIdle();
  ASSERT_TRUE(signal.has_value());
  EXPECT_EQ(signal->Action(), param.expected_action);
  EXPECT_THAT(signal->Reasons(), testing::ElementsAre(GracefulShutdownReason::kUserRequest));
}

}  // namespace
}  // namespace forensics::feedback
