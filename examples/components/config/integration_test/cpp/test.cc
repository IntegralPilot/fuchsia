// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/diagnostics/cpp/fidl.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/cpp/service_directory.h>

#include <optional>
#include <string>

#include <rapidjson/document.h>
#include <rapidjson/pointer.h>
#include <src/lib/fsl/vmo/strings.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

using ContentVector = std::vector<fuchsia::diagnostics::FormattedContent>;
using diagnostics::reader::InspectData;

constexpr char kChildUrl[] = "#meta/config_example.cm";

class IntegrationTest : public gtest::RealLoopFixture {
 protected:
  InspectData GetInspect(const std::string& name, const std::string& moniker) {
    std::stringstream selector;
    selector << "*/" << name << ":root";
    diagnostics::reader::ArchiveReader reader(dispatcher(), {selector.str()});
    fpromise::result<std::vector<InspectData>, std::string> result;
    async::Executor executor(dispatcher());
    executor.schedule_task(reader.SnapshotInspectUntilPresent({moniker}).then(
        [&](fpromise::result<std::vector<InspectData>, std::string>& rest) {
          result = std::move(rest);
        }));
    RunLoopUntil([&] { return result.is_ok() || result.is_error(); });

    EXPECT_EQ(result.is_error(), false) << "Error was " << result.error();
    EXPECT_EQ(result.value().size(), 1ul) << "Expected only one component";

    return std::move(result.value()[0]);
  }
};

TEST_F(IntegrationTest, ConfigCpp) {
  auto realm_builder = component_testing::RealmBuilder::Create();
  auto options =
      component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER};
  auto child_name = "config_example_replace_none";
  realm_builder.AddChild(child_name, kChildUrl, options);

  realm_builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{"fuchsia.logger.LogSink"}},
      .source = component_testing::ParentRef(),
      .targets = {component_testing::ChildRef{child_name}}});
  auto realm = realm_builder.Build();
  auto moniker = "realm_builder:" + realm.component().GetChildName() + "/" + child_name;

  auto data = GetInspect(child_name, moniker);

  // Verify that the published values match the static package definition in ../../cpp/BUILD.gn.
  EXPECT_EQ(rapidjson::Value("World"), data.GetByPath({"root", "config", "greeting"}));
  EXPECT_EQ(rapidjson::Value(100), data.GetByPath({"root", "config", "delay_ms"}));
}

TEST_F(IntegrationTest, ConfigCppReplaceSome) {
  auto realm_builder = component_testing::RealmBuilder::Create();
  auto options =
      component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER};
  auto child_name = "config_example_replace_some";
  realm_builder.AddChild(child_name, kChildUrl, options);

  // [START config_load]
  realm_builder.InitMutableConfigFromPackage(child_name);
  // [END config_load]

  // [START config_replace]
  realm_builder.SetConfigValue(child_name, "greeting", "Fuchsia");
  // [END config_replace]

  realm_builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{"fuchsia.logger.LogSink"}},
      .source = component_testing::ParentRef(),
      .targets = {component_testing::ChildRef{child_name}}});
  auto realm = realm_builder.Build();
  auto moniker = "realm_builder:" + realm.component().GetChildName() + "/" + child_name;

  auto data = GetInspect(child_name, moniker);

  EXPECT_EQ(rapidjson::Value("Fuchsia"), data.GetByPath({"root", "config", "greeting"}));
  EXPECT_EQ(rapidjson::Value(100), data.GetByPath({"root", "config", "delay_ms"}));
}

TEST_F(IntegrationTest, ConfigCppReplaceAllPackaged) {
  auto realm_builder = component_testing::RealmBuilder::Create();
  auto options =
      component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER};
  auto child_name = "config_example_replace_all_packaged";
  realm_builder.AddChild(child_name, kChildUrl, options);

  realm_builder.InitMutableConfigFromPackage(child_name);

  realm_builder.SetConfigValue(child_name, "greeting", "Fuchsia");
  realm_builder.SetConfigValue(child_name, "delay_ms",
                               component_testing::ConfigValue::Uint64(200u));

  realm_builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{"fuchsia.logger.LogSink"}},
      .source = component_testing::ParentRef(),
      .targets = {component_testing::ChildRef{child_name}}});
  auto realm = realm_builder.Build();
  auto moniker = "realm_builder:" + realm.component().GetChildName() + "/" + child_name;

  auto data = GetInspect(child_name, moniker);

  EXPECT_EQ(rapidjson::Value("Fuchsia"), data.GetByPath({"root", "config", "greeting"}));
  EXPECT_EQ(rapidjson::Value(200), data.GetByPath({"root", "config", "delay_ms"}));
}

// Same test as above except the config is initialized to empty.
TEST_F(IntegrationTest, ConfigCppSetAllWhenEmpty) {
  auto realm_builder = component_testing::RealmBuilder::Create();
  auto options =
      component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER};
  auto child_name = "config_example_set_all";
  realm_builder.AddChild(child_name, kChildUrl, options);

  // [START config_empty]
  realm_builder.InitMutableConfigToEmpty(child_name);
  // [END config_empty]

  realm_builder.SetConfigValue(child_name, "greeting", "Fuchsia");
  realm_builder.SetConfigValue(child_name, "delay_ms",
                               component_testing::ConfigValue::Uint64(200u));

  realm_builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{"fuchsia.logger.LogSink"}},
      .source = component_testing::ParentRef(),
      .targets = {component_testing::ChildRef{child_name}}});
  auto realm = realm_builder.Build();
  auto moniker = "realm_builder:" + realm.component().GetChildName() + "/" + child_name;

  auto data = GetInspect(child_name, moniker);

  EXPECT_EQ(rapidjson::Value("Fuchsia"), data.GetByPath({"root", "config", "greeting"}));
  EXPECT_EQ(rapidjson::Value(200), data.GetByPath({"root", "config", "delay_ms"}));
}
