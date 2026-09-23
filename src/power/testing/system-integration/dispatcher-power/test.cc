// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.driver.registrar/cpp/fidl.h>
#include <fidl/test.power.dispatcher/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

#include "src/power/testing/system-integration/util/test_util.h"

namespace {

class TestControllerServer : public fidl::Server<test_power_dispatcher::TestController> {
 public:
  struct Trackers {
    uint32_t recurring_task_run_count{0};
    uint32_t wake_vector_triggered_count{0};
    uint32_t non_wake_triggered_count{0};
    uint32_t resume_called_count{0};
    bool resume_has_lease{false};
  };

  explicit TestControllerServer(Trackers* trackers) : trackers_(trackers) {}

  void ReportRecurringTaskRun(ReportRecurringTaskRunCompleter::Sync& completer) override {
    trackers_->recurring_task_run_count++;
  }

  void ReportWakeVectorTriggered(ReportWakeVectorTriggeredCompleter::Sync& completer) override {
    trackers_->wake_vector_triggered_count++;
  }

  void ReportNonWakeWaitTriggered(ReportNonWakeWaitTriggeredCompleter::Sync& completer) override {
    trackers_->non_wake_triggered_count++;
  }

  void ReportSystemResume(ReportSystemResumeRequest& request,
                          ReportSystemResumeCompleter::Sync& completer) override {
    trackers_->resume_called_count++;
    trackers_->resume_has_lease = request.has_lease();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<test_power_dispatcher::TestController> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  Trackers* trackers_;
};

class TestControllerReceiver : public fidl::Server<fuchsia_component_sandbox::Receiver> {
 public:
  explicit TestControllerReceiver(
      async_dispatcher_t* dispatcher, TestControllerServer* server,
      fidl::ServerBindingGroup<test_power_dispatcher::TestController>* bindings)
      : dispatcher_(dispatcher), server_(server), bindings_(bindings) {}

  void Receive(ReceiveRequest& request, ReceiveCompleter::Sync& completer) override {
    bindings_->AddBinding(
        dispatcher_,
        fidl::ServerEnd<test_power_dispatcher::TestController>(std::move(request.channel())),
        server_, fidl::kIgnoreBindingClosure);
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_sandbox::Receiver> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  TestControllerServer* server_;
  fidl::ServerBindingGroup<test_power_dispatcher::TestController>* bindings_;
};

}  // namespace

class DispatcherPowerSystemIntegrationTest : public system_integration_utils::TestLoopBase,
                                             public testing::Test {
 protected:
  void SetUp() override {
    Initialize();

    auto registrar = component::Connect<fuchsia_driver_registrar::DriverRegistrar>();
    ASSERT_TRUE(registrar.is_ok());
    auto register_res =
        fidl::Call(*registrar)
            ->Register(
                {"fuchsia-pkg://fuchsia.com/dispatcher-power-test-pkg#meta/" + driver_url_suffix_});
    ASSERT_TRUE(register_res.is_ok());

    auto manager = component::Connect<fuchsia_driver_development::Manager>();
    ASSERT_TRUE(manager.is_ok());
    auto added = fidl::Call(*manager)->AddTestNode({{{
        .name = node_name_,
        .properties =
            {
                {
                    fuchsia_driver_framework::NodeProperty{
                        fuchsia_driver_framework::NodePropertyKey::WithStringValue(
                            "fuchsia.test.TEST_CHILD"),
                        fuchsia_driver_framework::NodePropertyValue::WithStringValue(
                            test_child_name_)},
                },
            },
    }}});
    EXPECT_EQ(true, added.is_ok());

    RunLoopWithTimeout(zx::sec(2));

    auto [receiver_client, receiver_server] =
        fidl::Endpoints<fuchsia_component_sandbox::Receiver>::Create();

    test_controller_server_ = std::make_unique<TestControllerServer>(&trackers_);
    receiver_server_ = std::make_unique<TestControllerReceiver>(
        dispatcher(), test_controller_server_.get(), &test_bindings_);

    receiver_binding_.emplace(dispatcher(), std::move(receiver_server), receiver_server_.get(),
                              fidl::kIgnoreBindingClosure);

    std::vector<system_integration_utils::CustomDictionaryEntry> custom_entries;
    custom_entries.push_back({"test.power.dispatcher.TestController", std::move(receiver_client)});

    fence_ = PrepareDriver(node_name_, driver_url_suffix_,
                           /*expect_new_koid=*/true, /*use_df_elements=*/true,
                           std::move(custom_entries));
  }

  void TearDown() override {
    // Trigger the test node removal
    FX_LOGS(INFO) << "Triggering RemoveTestNode in TearDown...";
    auto manager = component::Connect<fuchsia_driver_development::Manager>();
    if (manager.is_ok()) {
      auto remove_res = fidl::Call(*manager)->RemoveTestNode({{
          .name = node_name_,
      }});
      EXPECT_TRUE(remove_res.is_ok());

      auto disable_res = fidl::Call(*manager)->DisableDriver({{
          .driver_url =
              "fuchsia-pkg://fuchsia.com/dispatcher-power-test-pkg#meta/" + driver_url_suffix_,
      }});
      EXPECT_TRUE(disable_res.is_ok());
    }

    // Wait for the unbind and shutdown sequence to complete cleanly by looping until the node is
    // gone.
    FX_LOGS(INFO) << "Waiting for unbind to complete...";
    while (!GetNodeInfo(node_name_).empty()) {
      RunLoopWithTimeout(zx::msec(50));
    }

    // Release the dictionary fence. Since the node has been removed, the resuscitation will be
    // skipped.
    FX_LOGS(INFO) << "Releasing dictionary fence...";
    fence_.reset();
    receiver_binding_.reset();
    receiver_server_.reset();
    test_controller_server_.reset();
    test_bindings_.RemoveAll();
  }

  static void AcquireLease(const fidl::ClientEnd<fuchsia_power_broker::Topology>& broker,
                           zx::event power_token, const std::string& lease_name,
                           zx::eventpair& out_lease_token) {
    zx::eventpair lease_token_peer;
    ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &out_lease_token, &lease_token_peer));

    fuchsia_power_broker::LeaseSchema schema;
    schema.lease_token(std::move(lease_token_peer));
    schema.lease_name(lease_name);

    std::vector<fuchsia_power_broker::LeaseDependency> deps;
    deps.push_back(fuchsia_power_broker::LeaseDependency{{
        .requires_token = std::move(power_token),
        .requires_level = 1,
    }});
    schema.dependencies(std::move(deps));

    auto lease_res = fidl::Call(broker)->Lease(std::move(schema));
    ASSERT_TRUE(lease_res.is_ok());
  }

  std::string node_name_ = "dispatcher-power-test-node";
  std::string test_child_name_ = "dispatcher-power-test-driver";
  std::string driver_url_suffix_ = "dispatcher-power-test-driver.cm";

  zx::eventpair fence_;
  zx::eventpair lease_token_resume_;

  TestControllerServer::Trackers trackers_;
  std::unique_ptr<TestControllerServer> test_controller_server_;
  std::unique_ptr<TestControllerReceiver> receiver_server_;
  std::optional<fidl::ServerBinding<fuchsia_component_sandbox::Receiver>> receiver_binding_;
  fidl::ServerBindingGroup<test_power_dispatcher::TestController> test_bindings_;

  void RunTestBody() {
    // To enable changing SAG's power levels, first trigger the "boot complete" logic.
    test_sagcontrol::SystemActivityGovernorState state = GetBootCompleteState();
    ASSERT_EQ(ChangeSagState(state), ZX_OK);
    ASSERT_TRUE(SetBootComplete());

    // 1. Retrieve the test driver's captured dependency token.
    zx::event power_token;
    while (!power_token.is_valid()) {
      RunLoopWithTimeout(zx::msec(50));
      power_token = this->GetCapturedToken(node_name_);
    }

    // Duplicate the token for wake vector triggering later.
    zx::event wake_vector_trigger;
    ASSERT_EQ(ZX_OK, power_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &wake_vector_trigger));

    // 2. Directly acquire the active lease on the test driver element from the test.
    auto broker = component::Connect<fuchsia_power_broker::Topology>();
    ASSERT_EQ(ZX_OK, broker.status_value());

    zx::eventpair lease_token;
    AcquireLease(*broker, std::move(power_token), "test-lease", lease_token);

    // 3. Get the test driver host's KOID at startup.
    uint64_t test_driver_host_koid = 0;
    auto node_info_startup = GetNodeInfo(node_name_);
    for (const auto& node : node_info_startup) {
      if (node.driver_host_koid().has_value()) {
        test_driver_host_koid = node.driver_host_koid().value();
        break;
      }
    }
    ASSERT_NE(test_driver_host_koid, 0u);
    FX_LOGS(INFO) << "Test driver host koid at startup: " << test_driver_host_koid;

    // 4. Wait until we see the recurring task has run at least ten times (proving it runs on
    // startup).
    FX_LOGS(INFO) << "Waiting for dispatcher power test driver recurring task to start running...";
    while (trackers_.recurring_task_run_count < 10) {
      RunLoopWithTimeout(zx::msec(50));
    }
    FX_LOGS(INFO) << "Recurring task ran " << trackers_.recurring_task_run_count << " times.";

    // 5. Drop the active lease so the test driver can transition to inactive.
    FX_LOGS(INFO) << "Releasing active lease...";
    lease_token.reset();
    // 6. Emulate system suspend.
    FX_LOGS(INFO) << "Emulating system suspend...";
    state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kInactive);
    state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kInactive);
    ASSERT_EQ(ChangeSagState(state), ZX_OK);
    ASSERT_EQ(AwaitSystemSuspend(), ZX_OK);

    // Get the run count at the moment of suspend.
    uint32_t count_at_suspend = trackers_.recurring_task_run_count;
    FX_LOGS(INFO) << "Recurring task run count at suspend: " << count_at_suspend;

    // 7. Wait 2 seconds and verify that no new recurring task runs occur.
    FX_LOGS(INFO) << "Waiting 2 seconds to verify task is suspended...";
    RunLoopWithTimeout(zx::sec(2));

    // 7a. Trigger a non-wake wait (USER_SIGNAL_1) while suspended.
    FX_LOGS(INFO) << "Triggering non-wake wait via power token...";
    ASSERT_EQ(ZX_OK, wake_vector_trigger.signal(0, ZX_USER_SIGNAL_1));

    // 7b. Wait 2 seconds and verify that the non-wake wait did NOT run and the task is still
    // suspended.
    RunLoopWithTimeout(zx::sec(2));
    uint32_t count_after_non_wake = trackers_.recurring_task_run_count;
    FX_LOGS(INFO) << "Recurring task run count after non-wake wait: " << count_after_non_wake;
    EXPECT_EQ(count_at_suspend, count_after_non_wake);
    EXPECT_EQ(0u, trackers_.non_wake_triggered_count);

    // 7c. Trigger the wake vector while suspended.
    FX_LOGS(INFO) << "Triggering wake vector via power token...";
    ASSERT_EQ(ZX_OK, wake_vector_trigger.signal(0, ZX_USER_SIGNAL_0));

    // 8. Emulate system resume in response to the wake vector.
    FX_LOGS(INFO) << "Emulating system resume...";
    ASSERT_EQ(StartSystemResume(), ZX_OK);
    state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kActive);
    state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kActive);
    ASSERT_EQ(ChangeSagState(state), ZX_OK);

    // 10. Wait 2 seconds and verify that:
    // - The recurring task runs again.
    // - The wake vector callback executed.
    // - The non-wake wait callback executed.
    // - SystemResume was called with a valid lease token.
    FX_LOGS(INFO) << "Waiting 2 seconds to verify task has resumed...";
    RunLoopWithTimeout(zx::sec(2));

    uint32_t count_after_resume = trackers_.recurring_task_run_count;
    FX_LOGS(INFO) << "Recurring task run count after resume: " << count_after_resume;
    EXPECT_GT(count_after_resume, count_at_suspend);

    EXPECT_EQ(1u, trackers_.wake_vector_triggered_count);
    EXPECT_EQ(1u, trackers_.non_wake_triggered_count);
    EXPECT_EQ(2u, trackers_.resume_called_count);
    EXPECT_TRUE(trackers_.resume_has_lease);
  }
};

class DispatcherPowerSystemIntegrationTestCpp : public DispatcherPowerSystemIntegrationTest {
 public:
  DispatcherPowerSystemIntegrationTestCpp() {
    node_name_ = "dispatcher-power-test-node";
    test_child_name_ = "dispatcher-power-test-driver";
    driver_url_suffix_ = "dispatcher-power-test-driver.cm";
  }
};

TEST_F(DispatcherPowerSystemIntegrationTestCpp, TestDispatcherPowerManagement) { RunTestBody(); }
