// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <string>
#include <vector>

#include <gtest/gtest.h>

#include "src/developer/forensics/exceptions/constants.h"
#include "src/developer/forensics/exceptions/handler/wake_lease.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace forensics::exceptions {
namespace {

using ::fidl::ClientEnd;
using ::fidl::Endpoints;
using ::fidl::ServerEnd;
using ::fidl::SyncClient;
using ::forensics::exceptions::handler::WakeLease;
using ::fuchsia_power_broker::ElementControl;
using ::fuchsia_power_broker::ElementInfoProvider;
using ::fuchsia_power_broker::ElementInfoProviderGetStatusEndpointsResponse;
using ::fuchsia_power_broker::ElementInfoProviderService;
using ::fuchsia_power_broker::ElementRunner;
using ::fuchsia_power_broker::ElementRunnerSetLevelRequest;
using ::fuchsia_power_broker::ElementSchema;
using ::fuchsia_power_broker::ElementStatusEndpoint;
using ::fuchsia_power_broker::LeaseControl;
using ::fuchsia_power_broker::Lessor;
using ::fuchsia_power_broker::LessorLeaseResponse;
using ::fuchsia_power_broker::LevelDependency;
using ::fuchsia_power_broker::Status;
using ::fuchsia_power_broker::StatusWatchPowerLevelResponse;
using ::fuchsia_power_broker::Topology;
using ::fuchsia_power_system::ActivityGovernor;
using ::fuchsia_power_system::ApplicationActivityLevel;
using ::fuchsia_power_system::BootControl;
using ::fuchsia_power_system::ExecutionStateLevel;
using ::fuchsia_power_system::LeaseToken;
using ::fuchsia_power_system::PowerElements;

constexpr char kApplicationActivity[] = "application_activity";
constexpr char kExecutionState[] = "execution_state";
constexpr char kBootCompleteIndicator[] = "boot-complete-indicator";

constexpr uint8_t ToUint(const ApplicationActivityLevel value) {
  return static_cast<uint8_t>(value);
}
constexpr uint8_t ToUint(const ExecutionStateLevel value) { return static_cast<uint8_t>(value); }

// Evaluates |rexpr|. On error, this function check-fails and prints the error. On success, sets the
// result of |rexpr| to |lhs|. This function exists to check invariants.
#define ASSIGN_OR_CHECK(lhs, rexpr) \
  ASSIGN_OR_CHECK_IMPL(RESULT_MACROS_CONCAT_NAME(result, __LINE__), lhs, rexpr)

// Same as |ASSIGN_OR_CHECK|, but doesn't attempt to set the result value of |expr| to a variable.
// Useful for checking a FIDL call that doesn't return a value but can still return an error.
#define CHECK_RESULT(expr) CHECK_RESULT_IMPL(RESULT_MACROS_CONCAT_NAME(result, __LINE__), expr)

#define ASSIGN_OR_CHECK_IMPL(result, lhs, rexpr) \
  CHECK_RESULT_IMPL(result, rexpr);              \
  lhs = std::move(result).value()

#define CHECK_RESULT_IMPL(result, rexpr) \
  auto(result) = (rexpr);                \
  FX_CHECK((result).is_ok()) << "Error: " << (result).error_value()

#define RESULT_MACROS_CONCAT_NAME(x, y) RESULT_MACROS_CONCAT_IMPL(x, y)
#define RESULT_MACROS_CONCAT_IMPL(x, y) x##y

template <typename Protocol>
SyncClient<Protocol> Connect() {
  zx::result client_end = component::Connect<Protocol>();
  FX_CHECK(client_end.is_ok());
  return SyncClient(std::move(client_end).value());
}

bool SetBootComplete() {
  zx::result boot_control_client = component::Connect<BootControl>();
  if (!boot_control_client.is_ok()) {
    FX_LOGS(ERROR) << "Synchronous error when connecting to the fuchsia.power.system/BootControl"
                   << " protocol: " << boot_control_client.status_string();
    return false;
  }
  auto status = fidl::WireCall(boot_control_client.value())->SetBootComplete();
  return status.ok();
}

// Returns nullptr if there's an error connecting to required protocols.
std::unique_ptr<WakeLease> CreateWakeLease(async_dispatcher_t* dispatcher) {
  zx::result sag_client_end = component::Connect<ActivityGovernor>();
  if (!sag_client_end.is_ok()) {
    FX_LOGS(ERROR)
        << "Synchronous error when connecting to the fuchsia.power.system/ActivityGovernor"
        << " protocol: " << sag_client_end.status_string();
    return nullptr;
  }

  return std::make_unique<WakeLease>(dispatcher, /*lease_name=*/"exceptions-element-001",
                                     std::move(sag_client_end).value());
}

ElementSchema BuildAssertiveApplicationActivitySchema(
    zx::event requires_token, ServerEnd<ElementControl> element_control_server_end,
    ServerEnd<Lessor> lessor_server_end, ClientEnd<ElementRunner> element_runner_client_end,
    const std::string& element_name) {
  LevelDependency dependency{{
      .dependent_level = kPowerLevelActive,
      .requires_token = std::move(requires_token),
      .requires_level_by_preference =
          std::vector<uint8_t>(1, ToUint(ApplicationActivityLevel::kActive)),
  }};

  ElementSchema schema{{
      .element_name = element_name,
      .initial_current_level = kPowerLevelActive,
      .valid_levels = std::vector<uint8_t>({kPowerLevelInactive, kPowerLevelActive}),
      .lessor_channel = std::move(lessor_server_end),
      .element_control = std::move(element_control_server_end),
      .element_runner = std::move(element_runner_client_end),
  }};

  std::optional<std::vector<LevelDependency>>& dependencies = schema.dependencies();
  dependencies.emplace().push_back(std::move(dependency));

  return schema;
}

struct ElementWithLease {
  ClientEnd<ElementControl> element_control;
  ClientEnd<LeaseControl> lease_control;
  ServerEnd<ElementRunner> element_runner;
};

// Adds an element with an assertive dependency on ApplicationActivity and takes a lease on that
// element.
ElementWithLease RaiseApplicationActivity() {
  ASSIGN_OR_CHECK(PowerElements power_elements, Connect<ActivityGovernor>()->GetPowerElements());
  FX_CHECK(power_elements.application_activity().has_value());
  FX_CHECK(power_elements.application_activity()->assertive_dependency_token().has_value());

  zx::event aa_token =
      std::move(power_elements.application_activity()->assertive_dependency_token()).value();

  Endpoints<ElementControl> element_control_endpoints = Endpoints<ElementControl>::Create();
  Endpoints<Lessor> lessor_endpoints = Endpoints<Lessor>::Create();
  SyncClient<Lessor> lessor_client(std::move(lessor_endpoints.client));
  Endpoints<ElementRunner> element_runner = Endpoints<ElementRunner>::Create();

  ElementSchema schema = BuildAssertiveApplicationActivitySchema(
      std::move(aa_token), std::move(element_control_endpoints.server),
      std::move(lessor_endpoints.server), std::move(element_runner.client),
      /*element_name=*/kBootCompleteIndicator);

  CHECK_RESULT(Connect<Topology>()->AddElement(std::move(schema)));

  ASSIGN_OR_CHECK(LessorLeaseResponse aa_lease, lessor_client->Lease(kPowerLevelActive));

  return ElementWithLease{
      .element_control = std::move(element_control_endpoints.client),
      .lease_control = std::move(aa_lease.lease_control()),
      .element_runner = std::move(element_runner.server),
  };
}

std::vector<ElementStatusEndpoint> GetStatusEndpoints() {
  ASSIGN_OR_CHECK(ElementInfoProviderService::ServiceClient element_info_service,
                  component::OpenService<ElementInfoProviderService>("system_activity_governor"));

  ASSIGN_OR_CHECK(ClientEnd<ElementInfoProvider> element_info,
                  element_info_service.connect_status_provider());

  SyncClient<ElementInfoProvider> element_info_client(std::move(element_info));
  ASSIGN_OR_CHECK(ElementInfoProviderGetStatusEndpointsResponse status_endpoints,
                  element_info_client->GetStatusEndpoints());

  return std::move(status_endpoints.endpoints());
}

uint8_t GetCurrentLevel(const std::string& element) {
  std::vector<ElementStatusEndpoint> status_endpoints = GetStatusEndpoints();
  auto status = std::find_if(status_endpoints.begin(), status_endpoints.end(),
                             [&element](const ElementStatusEndpoint& endpoint) {
                               return endpoint.identifier() == element;
                             });
  FX_CHECK(status != status_endpoints.end());

  SyncClient<Status> status_client(std::move(*status->status()));
  ASSIGN_OR_CHECK(StatusWatchPowerLevelResponse result, status_client->WatchPowerLevel());

  return result.current_level();
}

// Uses |status_client| to retrieve the current power level. This function will block until the
// power level changes if the power level has not changed since the last time |status_client| was
// used to retrieve the power level.
uint8_t GetCurrentLevel(const SyncClient<Status>& status_client) {
  ASSIGN_OR_CHECK(StatusWatchPowerLevelResponse result, status_client->WatchPowerLevel());
  return result.current_level();
}

using WakeLeaseIntegrationTest = gtest::RealLoopFixture;

class FakeElementRunner : public fidl::Server<ElementRunner> {
 public:
  FakeElementRunner() = default;

  void handle_unknown_method(fidl::UnknownMethodMetadata<ElementRunner> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL() << "Unknown method called";
  }

  void SetLevel(ElementRunnerSetLevelRequest& request,
                SetLevelCompleter::Sync& completer) override {
    // Return to acknowledge the new level.
    completer.Reply();
  }
};

TEST_F(WakeLeaseIntegrationTest, AcquiresLease) {
  // Take an assertive dependency on ApplicationActivity to indicate boot complete, allowing SAG to
  // suspend if it deems appropriate. After we've acquired our wake lease using the WakeLease class,
  // we'll drop the lease on ApplicationActivity and check that ExecutionState was held at the
  // kSuspending level (the level that WakeLease has an assertive dependency on).
  ElementWithLease aa_element = RaiseApplicationActivity();
  ASSERT_TRUE(SetBootComplete());
  ASSERT_EQ(GetCurrentLevel(kApplicationActivity), ToUint(ApplicationActivityLevel::kActive));

  std::vector<ElementStatusEndpoint> status_endpoints = GetStatusEndpoints();
  auto es_status_endpoint = std::find_if(status_endpoints.begin(), status_endpoints.end(),
                                         [](const ElementStatusEndpoint& endpoint) {
                                           return endpoint.identifier() == kExecutionState;
                                         });
  ASSERT_NE(es_status_endpoint, status_endpoints.end());

  SyncClient<Status> es_status_client(std::move(*es_status_endpoint->status()));

  // Subsequent calls to GetCurrentLevel using |es_status_client| will block until the level has
  // changed, so we'll reuse |es_status_client| when we want to wait until the power level has
  // changed.
  ASSERT_EQ(GetCurrentLevel(es_status_client), ToUint(ExecutionStateLevel::kActive));

  std::optional<LeaseToken> lease;
  std::unique_ptr<WakeLease> wake_lease = CreateWakeLease(dispatcher());

  fpromise::promise<void, Error> lease_promise =
      wake_lease->Acquire(kWakeLeaseAcquisitionTimeout)
          .or_else([](const Error& error) {
            FX_LOGS(FATAL) << "Wake lease not acquired: " << ToString(error);
            return fpromise::make_result_promise<LeaseToken, Error>(
                fpromise::error(Error::kBadValue));
          })
          .and_then(
              [&lease](LeaseToken& acquired_lease) mutable { lease = std::move(acquired_lease); });

  async::Executor executor(dispatcher());
  FakeElementRunner element_runner;
  fidl::BindServer(executor.dispatcher(), std::move(aa_element.element_runner), &element_runner);

  executor.schedule_task(std::move(lease_promise));
  RunLoopWithTimeoutOrUntil([&lease]() { return lease.has_value(); },
                            kWakeLeaseAcquisitionTimeout + zx::sec(1));

  // |aa_element| is still valid, so ApplicationActivity should still be holding ExecutionState at
  // kActive.
  ASSERT_TRUE(lease.has_value());
  EXPECT_TRUE(lease->is_valid());
  ASSERT_EQ(GetCurrentLevel(kExecutionState), ToUint(ExecutionStateLevel::kActive));

  // Drop the AA lease but leave |lease| intact.
  //
  // Power Broker won't consider a lease dropped until the power element's power level is set to the
  // necessary level (here, 0) via the CurrentLevel protocol. For testing simplicity, we'll just
  // remove the element from the topology by resetting |element_control|.
  aa_element.lease_control.reset();
  ASSERT_FALSE(aa_element.lease_control.is_valid());
  aa_element.element_control.reset();
  ASSERT_FALSE(aa_element.element_control.is_valid());

  RunLoopUntilIdle();
  ASSERT_EQ(GetCurrentLevel(es_status_client), ToUint(ExecutionStateLevel::kSuspending));

  // Drop |lease| so nothing is holding the system awake anymore.
  lease.reset();
  RunLoopUntilIdle();
  EXPECT_EQ(GetCurrentLevel(es_status_client), ToUint(ExecutionStateLevel::kInactive));
}

}  // namespace
}  // namespace forensics::exceptions
