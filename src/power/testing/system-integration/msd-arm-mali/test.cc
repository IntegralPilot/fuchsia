// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.registrar/cpp/fidl.h>
#include <fidl/fuchsia.gpu.magma/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/natural_ostream.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/driver/power/cpp/power-support.h>
#include <lib/fdio/directory.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma_client/test_util/magma_map_cpu.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/channel.h>
#include <poll.h>

#include <filesystem>
#include <thread>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-arm-mali/include/magma_arm_mali_types.h"
#include "src/graphics/drivers/msd-arm-mali/include/magma_arm_mali_vendor_id.h"
#include "src/graphics/drivers/msd-arm-mali/include/magma_vendor_queries.h"
#include "src/graphics/drivers/msd-arm-mali/tests/integration/mali_utils.h"
#include "src/power/testing/system-integration/util/test_util.h"

namespace {
inline const std::string kProductionDriverUrl =
    "fuchsia-pkg://fuchsia.com/msd-arm-mali#meta/msd_arm.cm";
inline const std::string kTestDriverUrl =
    "fuchsia-pkg://fuchsia.com/mali_power_test_pkg#meta/msd_arm_test.cm";

namespace InspectSelectors {
inline const std::string kSagMoniker = "system-activity-governor/system-activity-governor";
inline const std::vector<std::string> kSagExecStateLevel = {"root", "power_elements",
                                                            "execution_state", "power_level"};

// Driver monikers are unstable, so wildcard the moniker and use a tree name
inline const std::string kMsdArmMaliInspectTreeName = "mali-test";
inline const std::string kMsdArmMaliMoniker = "bootstrap/*-drivers*";
inline const std::vector<std::string> kMsdArmMaliIsSystemSuspending = {
    "root", "msd-arm-mali", "device", "is_system_suspending"};
inline const std::vector<std::string> kMsdArmMaliPoweredOn = {"root", "msd-arm-mali", "device",
                                                              "powered_on"};
inline const std::vector<std::string> kMsdArmMaliPowerOnAfterSuspend = {
    "root", "msd-arm-mali", "device", "power_on_after_suspend"};
}  // namespace InspectSelectors

class TestConnection : public magma::TestDeviceBase {
 public:
  TestConnection() : magma::TestDeviceBase(MAGMA_VENDOR_ID_MALI) {
    magma_device_create_connection(device(), &connection_);
    DASSERT(connection_);

    magma_connection_create_context(connection_, &context_id_);
    helper_.emplace(connection_, context_id_);
  }

  ~TestConnection() {
    magma_connection_release_context(connection_, context_id_);

    if (connection_)
      magma_connection_release(connection_);
  }

  void SubmitCommandBuffer(mali_utils::AtomHelper::How how, uint8_t atom_number,
                           uint8_t atom_dependency, bool protected_mode) {
    helper_->SubmitCommandBuffer(how, atom_number, atom_dependency, protected_mode);
  }

 private:
  magma_connection_t connection_;
  uint32_t context_id_;
  std::optional<mali_utils::AtomHelper> helper_;
};

class PowerSystemIntegration : public system_integration_utils::TestLoopBase, public testing::Test {
 public:
  void SetUp() override {
    Initialize();

    auto registrar = component::Connect<fuchsia_driver_registrar::DriverRegistrar>();
    ASSERT_EQ(ZX_OK, registrar.status_value());
    auto register_res = fidl::Call(*registrar)->Register({kTestDriverUrl});
    ASSERT_TRUE(register_res.is_ok()) << register_res.error_value();

    auto manager = component::Connect<fuchsia_driver_development::Manager>();
    ASSERT_EQ(ZX_OK, manager.status_value());
    (void)fidl::Call(*manager)->DisableDriver({{
        .driver_url = kProductionDriverUrl,
    }});
    (void)fidl::Call(*manager)->RebindCompositesWithDriver({{
        .driver_url = kProductionDriverUrl,
    }});
  }

  void TearDown() override {
    auto manager = component::Connect<fuchsia_driver_development::Manager>();
    if (manager.is_ok()) {
      (void)fidl::Call(*manager)->DisableDriver({{
          .driver_url = kTestDriverUrl,
      }});
      (void)fidl::Call(*manager)->EnableDriver({{
          .driver_url = kProductionDriverUrl,
      }});
      (void)fidl::Call(*manager)->RebindCompositesWithDriver({{
          .driver_url = kTestDriverUrl,
      }});
    }
    fence_.reset();
    // Add a delay for the fence reset to finish restarting the target driver back to normal.
    RunLoopWithTimeout(zx::sec(1));
  }

 protected:
  zx::eventpair fence_;
};

// Integration test verifying Mali & aml-gpu power suspend/resume mechanics.
// Uses PrepareDriverWithPowerTokenOverrides to inject power token overrides
// for the "arm-mali-0" device.
TEST_F(PowerSystemIntegration, SuspendResume) {
  // Bind driver with power token overrides for target child node "arm-mali-0".
  fence_ = PrepareDriverWithPowerTokenOverrides("gpu-ffe40000", "/aml-gpu-package#meta/aml-gpu.cm",
                                                /*expect_new_koid=*/true, {"arm-mali-0"});

  // Duration to sleep must be << 1 second, or else the command submission may timeout.
  const auto kPollDuration = zx::msec(50);
  // To enable changing SAG's power levels, first trigger the "boot complete" logic. This is done by
  // setting both exec state level and app activity level to active.
  test_sagcontrol::SystemActivityGovernorState state = GetBootCompleteState();
  ASSERT_EQ(ChangeSagState(state, kPollDuration), ZX_OK);
  ASSERT_TRUE(SetBootComplete());

  // There are two archive accessors. One is the test one that is hermetic to the test realm.
  // This is where the broker/SAG specific entries can be found.
  //
  // The other is the real one for the system. This is where the driver entries can be found.
  //
  auto test_archives_result = component::Connect<fuchsia_diagnostics::ArchiveAccessor>();
  ASSERT_EQ(ZX_OK, test_archives_result.status_value());
  diagnostics::reader::ArchiveReader test_reader(dispatcher(), {},
                                                 std::move(test_archives_result.value()));

  auto real_archives_result = component::Connect<fuchsia_diagnostics::ArchiveAccessor>(
      "/svc/fuchsia.diagnostics.RealArchiveAccessor");
  ASSERT_EQ(ZX_OK, real_archives_result.status_value());
  diagnostics::reader::ArchiveReader real_reader(dispatcher(), {},
                                                 std::move(real_archives_result.value()));

  const std::string pb_moniker = "power-broker";
  const auto aml_gpu_element_id = GetPowerElementId(test_reader, pb_moniker, "gpu-ffe40000");
  ASSERT_TRUE(aml_gpu_element_id.is_ok());
  const std::vector<std::string> aml_gpu_required_level = {"root",     "broker",
                                                           "topology", "fuchsia.inspect.Graph",
                                                           "topology", aml_gpu_element_id.value(),
                                                           "meta",     "required_level"};
  const std::vector<std::string> aml_gpu_current_level = {"root",     "broker",
                                                          "topology", "fuchsia.inspect.Graph",
                                                          "topology", aml_gpu_element_id.value(),
                                                          "meta",     "current_level"};
  const auto mali_element_id = GetPowerElementId(test_reader, pb_moniker, "arm-mali-0");
  ASSERT_TRUE(mali_element_id.is_ok());
  const std::vector<std::string> mali_required_level = {"root",     "broker",
                                                        "topology", "fuchsia.inspect.Graph",
                                                        "topology", mali_element_id.value(),
                                                        "meta",     "required_level"};
  const std::vector<std::string> mali_current_level = {"root",     "broker",
                                                       "topology", "fuchsia.inspect.Graph",
                                                       "topology", mali_element_id.value(),
                                                       "meta",     "current_level"};

  const auto exec_state_id = GetPowerElementId(test_reader, pb_moniker, "execution_state");
  ASSERT_TRUE(exec_state_id.is_ok());
  const std::vector<std::string> exec_state_req = {"root",     "broker",
                                                   "topology", "fuchsia.inspect.Graph",
                                                   "topology", exec_state_id.value(),
                                                   "meta",     "required_level"};
  const auto cpu_id = GetPowerElementId(test_reader, pb_moniker, "cpu");
  ASSERT_TRUE(cpu_id.is_ok());
  const std::vector<std::string> cpu_req = {
      "root",     "broker",       "topology", "fuchsia.inspect.Graph",
      "topology", cpu_id.value(), "meta",     "required_level"};

  std::cout << "Verify boot complete state using inspect data:\n";
  // - SAG: exec state level active
  // - Power Broker: aml-gpu and mali on
  // - msd_arm_mali - not powered on.
  // - msd_arm_mali - system is not suspending.
  // - msd_arm_mali - no power on after suspend.
  MatchInspectData(test_reader, InspectSelectors::kSagMoniker, std::nullopt,
                   InspectSelectors::kSagExecStateLevel, uint64_t{2});  // kActive
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_gpu_required_level, uint64_t{1});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_gpu_current_level, uint64_t{1});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, mali_required_level, uint64_t{1});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, mali_current_level, uint64_t{1});
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPoweredOn, false);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliIsSystemSuspending, false);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPowerOnAfterSuspend, false);

  std::unique_ptr<TestConnection> test;
  test.reset(new TestConnection());
  std::atomic_bool finished_test{false};
  std::thread command_loop([&] {
    uint32_t i = 0;
    while (!finished_test) {
      {
        SCOPED_TRACE(std::to_string(i++));
        test->SubmitCommandBuffer(mali_utils::AtomHelper::NORMAL, 1, 0, false);
      }
    }
  });

  std::cout << "Work has been queued and system is running:\n";
  // - msd_arm_mali - powered on.
  // - msd_arm_mali - system is not suspending.
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPoweredOn, true);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliIsSystemSuspending, false);

  // Emulate system suspend.
  state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kInactive);
  state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kInactive);
  ASSERT_EQ(ChangeSagState(state, kPollDuration), ZX_OK);
  ASSERT_EQ(AwaitSystemSuspend(), ZX_OK);

  MatchInspectData(test_reader, pb_moniker, std::nullopt, exec_state_req, uint64_t{0});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, cpu_req, uint64_t{0});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, mali_required_level, uint64_t{0});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, mali_current_level, uint64_t{0});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_gpu_required_level, uint64_t{0});
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_gpu_current_level, uint64_t{0});

  std::cout << "Work is still queuing but system is suspending:\n";
  // - msd_arm_mali - not powered on.
  // - msd_arm_mali - system is suspending.
  // - msd_arm_mali - power on after suspend.
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPoweredOn, false);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliIsSystemSuspending, true);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPowerOnAfterSuspend, true);

  // Emulate system resume.
  ASSERT_EQ(StartSystemResume(), ZX_OK);
  state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kActive);
  state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kActive);
  ASSERT_EQ(ChangeSagState(state, kPollDuration), ZX_OK);

  std::cout << "Work has been queued before resume which can now be completed:\n";
  // - msd_arm_mali - powered on.
  // - msd_arm_mali - system is not suspending.
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPoweredOn, true);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliIsSystemSuspending, false);

  finished_test = true;
  command_loop.join();

  std::cout << "Now that work is done, the GPU can power down.\n";
  // - msd_arm_mali - not powered on.
  // - msd_arm_mali - system is not suspending.
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliPoweredOn, false);
  MatchInspectData(real_reader, InspectSelectors::kMsdArmMaliMoniker,
                   InspectSelectors::kMsdArmMaliInspectTreeName,
                   InspectSelectors::kMsdArmMaliIsSystemSuspending, false);
}
}  // namespace
