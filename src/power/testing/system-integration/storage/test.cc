// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/natural_ostream.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/fdio/directory.h>

#include <gtest/gtest.h>

#include "src/power/testing/system-integration/util/test_util.h"

class PowerSystemIntegration : public system_integration_utils::TestLoopBase, public testing::Test {
 public:
  void SetUp() override {
    Initialize();
    fence_ = PrepareDriver("mmc-ffe07000", "fuchsia-boot:///aml-sdmmc#meta/aml-sdmmc.cm",
                           /*expect_new_koid=*/true, /*use_df_elements=*/true);

    // Helps logs settle a bit, not necessary.
    RunLoopWithTimeout(zx::msec(500));
  }
  void TearDown() override {
    fence_.reset();

    // Helps ensure driver framework state has had a chance to reset.
    RunLoopWithTimeout(zx::sec(1));
  }

 private:
  // Hold on to fence for the test duration.
  zx::eventpair fence_;
};

TEST_F(PowerSystemIntegration, StorageSuspendResumeTest) {
  // To enable changing SAG's power levels, first trigger the "boot complete" logic. This is done by
  // setting both exec state level and app activity level to active.
  test_sagcontrol::SystemActivityGovernorState state = GetBootCompleteState();
  ASSERT_EQ(ChangeSagState(state), ZX_OK);
  ASSERT_TRUE(SetBootComplete());

  // There are two archive accessors. One is the test one that is hermetic to the test realm.
  // This is where the broker/SAG specific entries can be found.
  //
  // The other is the real one for the system. This is where the driver entries can be found.
  auto test_archives_result = component::Connect<fuchsia_diagnostics::ArchiveAccessor>();
  ASSERT_EQ(ZX_OK, test_archives_result.status_value());
  diagnostics::reader::ArchiveReader test_reader(dispatcher(), {},
                                                 std::move(test_archives_result.value()));

  auto real_archives_result = component::Connect<fuchsia_diagnostics::ArchiveAccessor>(
      "/svc/fuchsia.diagnostics.RealArchiveAccessor");
  ASSERT_EQ(ZX_OK, real_archives_result.status_value());
  diagnostics::reader::ArchiveReader real_reader(dispatcher(), {},
                                                 std::move(real_archives_result.value()));

  const std::string sag_moniker = "system-activity-governor/system-activity-governor";
  const std::vector<std::string> sag_exec_state_level = {"root", "power_elements",
                                                         "execution_state", "power_level"};

  const std::string pb_moniker = "power-broker";
  const auto aml_sdmmc_element_id = GetPowerElementId(test_reader, pb_moniker, "mmc-ffe07000");
  ASSERT_TRUE(aml_sdmmc_element_id.is_ok());
  const std::vector<std::string> aml_sdmmc_required_level = {
      "root",     "broker",
      "topology", "fuchsia.inspect.Graph",
      "topology", aml_sdmmc_element_id.value(),
      "meta",     "required_level"};
  const std::vector<std::string> aml_sdmmc_current_level = {
      "root",     "broker",
      "topology", "fuchsia.inspect.Graph",
      "topology", aml_sdmmc_element_id.value(),
      "meta",     "current_level"};
  const auto core_sdmmc_element_id = GetPowerElementId(test_reader, pb_moniker, "aml-sd-emmc");
  ASSERT_TRUE(core_sdmmc_element_id.is_ok());
  const std::vector<std::string> core_sdmmc_required_level = {
      "root",     "broker",
      "topology", "fuchsia.inspect.Graph",
      "topology", core_sdmmc_element_id.value(),
      "meta",     "required_level"};
  const std::vector<std::string> core_sdmmc_current_level = {
      "root",     "broker",
      "topology", "fuchsia.inspect.Graph",
      "topology", core_sdmmc_element_id.value(),
      "meta",     "current_level"};

  // Driver monikers are unstable, so wildcard the moniker and use a tree name
  const std::string aml_sdmmc_inspect_tree_name = "aml-sd-emmc";
  const std::string aml_sdmmc_moniker = "bootstrap/*-drivers*mmc-ffe07000*";
  // TODO(b/344044167): Fix inspect node names in aml-sdmmc driver.
  const std::vector<std::string> aml_sdmmc_suspended = {"root", "aml-sdmmc-port-mmc@ffe07000",
                                                        "power_suspended"};

  // Driver monikers are unstable, so wildcard the moniker and use a tree name
  const std::string core_sdmmc_moniker = "bootstrap/*-drivers*mmc-ffe07000*";
  const std::string sdmmc_root_device_inspect_tree_name = "sdmmc";
  const std::vector<std::string> core_sdmmc_suspended = {"root", "sdmmc_core", "power_suspended"};

  // Verify boot complete state using inspect data:
  // - SAG: exec state level active
  // - Power Broker: aml-sdmmc's power element on.
  // - aml-sdmmc, core sdmmc: not suspended
  MatchInspectData(test_reader, sag_moniker, std::nullopt, sag_exec_state_level,
                   uint64_t{2});  // kActive
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_sdmmc_required_level,
                   uint64_t{1});  // On
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_sdmmc_current_level,
                   uint64_t{1});  // On
  MatchInspectData(test_reader, pb_moniker, std::nullopt, core_sdmmc_required_level,
                   uint64_t{1});  // On
  MatchInspectData(test_reader, pb_moniker, std::nullopt, core_sdmmc_current_level,
                   uint64_t{1});  // On
  MatchInspectData(real_reader, aml_sdmmc_moniker, aml_sdmmc_inspect_tree_name, aml_sdmmc_suspended,
                   false);
  MatchInspectData(real_reader, core_sdmmc_moniker, sdmmc_root_device_inspect_tree_name,
                   core_sdmmc_suspended, false);

  // Emulate system suspend.
  state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kInactive);
  state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kInactive);
  ASSERT_EQ(ChangeSagState(state), ZX_OK);
  ASSERT_EQ(AwaitSystemSuspend(), ZX_OK);

  // Verify suspend state using inspect data:
  // - SAG: exec state level inactive
  // - Power Broker: aml-sdmmc's power element off.
  // - aml-sdmmc, core sdmmc: suspended
  MatchInspectData(test_reader, sag_moniker, std::nullopt, sag_exec_state_level,
                   uint64_t{0});  // kInactive
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_sdmmc_required_level,
                   uint64_t{0});  // Off
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_sdmmc_current_level,
                   uint64_t{0});  // Off
  MatchInspectData(test_reader, pb_moniker, std::nullopt, core_sdmmc_required_level,
                   uint64_t{0});  // Off
  MatchInspectData(test_reader, pb_moniker, std::nullopt, core_sdmmc_current_level,
                   uint64_t{0});  // Off
  MatchInspectData(real_reader, aml_sdmmc_moniker, aml_sdmmc_inspect_tree_name, aml_sdmmc_suspended,
                   true);
  MatchInspectData(real_reader, core_sdmmc_moniker, sdmmc_root_device_inspect_tree_name,
                   core_sdmmc_suspended, true);

  // Emulate system resume.
  ASSERT_EQ(StartSystemResume(), ZX_OK);
  state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kActive);
  state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kActive);
  ASSERT_EQ(ChangeSagState(state), ZX_OK);

  // Verify resume state using inspect data:
  // - SAG: exec state level active
  // - Power Broker: aml-sdmmc's power element on.
  // - aml-sdmmc, core sdmmc: not suspended
  MatchInspectData(test_reader, sag_moniker, std::nullopt, sag_exec_state_level,
                   uint64_t{2});  // kActive
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_sdmmc_required_level,
                   uint64_t{1});  // On
  MatchInspectData(test_reader, pb_moniker, std::nullopt, aml_sdmmc_current_level,
                   uint64_t{1});  // On
  MatchInspectData(test_reader, pb_moniker, std::nullopt, core_sdmmc_required_level,
                   uint64_t{1});  // On
  MatchInspectData(test_reader, pb_moniker, std::nullopt, core_sdmmc_current_level,
                   uint64_t{1});  // On
  MatchInspectData(real_reader, aml_sdmmc_moniker, aml_sdmmc_inspect_tree_name, aml_sdmmc_suspended,
                   false);
  MatchInspectData(real_reader, core_sdmmc_moniker, sdmmc_root_device_inspect_tree_name,
                   core_sdmmc_suspended, false);
}
