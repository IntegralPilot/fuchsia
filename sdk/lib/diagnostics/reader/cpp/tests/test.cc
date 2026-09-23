// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/default.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/cpp/component_context.h>

#include <memory>

#include <rapidjson/pointer.h>
#include <re2/re2.h>

#include "lib/fpromise/promise.h"
#include "lib/sys/cpp/service_directory.h"
#include "rapidjson/document.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace component {
namespace {

using namespace component_testing;

std::string regex_replace(const std::string& input, re2::RE2& reg, const std::string& rewrite) {
  std::string output = input;
  re2::RE2::GlobalReplace(&output, reg, rewrite);
  return output;
}

const char CM1_EXPECTED_DATA[] = R"JSON({
    "data_source": "Inspect",
    "metadata": {
        "component_url": "REALM_BUILDER_URL_PREFIX/test_app",
        "name": "root",
        "timestamp": TIMESTAMP
    },
    "moniker": "REALM_BUILDER_MONIKER_PREFIX/test_app",
    "payload": {
        "root": {
            "option_a": {
                "value": 10
            },
            "option_b": {
                "value": 20
            },
            "version": "v1"
        }
    },
    "version": 1
})JSON";
const char CM2_EXPECTED_DATA[] = R"JSON({
    "data_source": "Inspect",
    "metadata": {
        "component_url": "REALM_BUILDER_URL_PREFIX/test_app_2",
        "name": "root",
        "timestamp": TIMESTAMP
    },
    "moniker": "REALM_BUILDER_MONIKER_PREFIX/test_app_2",
    "payload": {
        "root": {
            "option_a": {
                "value": 10
            },
            "option_b": {
                "value": 20
            },
            "version": "v1"
        }
    },
    "version": 1
})JSON";

class ArchiveReaderTest : public gtest::RealLoopFixture {
 protected:
  ArchiveReaderTest() : executor_(dispatcher()) {}

  void SetUp() override {
    component_context_ = sys::ComponentContext::Create();

    auto builder = RealmBuilder::Create();

    builder.AddChild("test_app", "#meta/archive_reader_test_app.cm",
                     ChildOptions{.startup_mode = StartupMode::EAGER});
    builder.AddChild("test_app_2", "#meta/archive_reader_test_app.cm",
                     ChildOptions{.startup_mode = StartupMode::EAGER});

    builder.AddRoute(Route{.capabilities = {Protocol{"fuchsia.logger.LogSink"}},
                           .source = ParentRef(),
                           .targets = {ChildRef{"test_app"}, ChildRef{"test_app_2"}}});

    realm_ = std::make_unique<RealmRoot>(builder.Build(dispatcher()));
  }

  void TearDown() override { realm_.reset(); }

  async::Executor& executor() { return executor_; }

  std::shared_ptr<sys::ServiceDirectory> svc() { return component_context_->svc(); }

  std::string cm1_selector() {
    return "realm_builder\\:" + realm_->component().GetChildName() + "/test_app:root";
  }

  std::string cm2_selector() {
    return "realm_builder\\:" + realm_->component().GetChildName() + "/test_app_2:root";
  }

  std::string cm1_moniker() {
    return "realm_builder:" + realm_->component().GetChildName() + "/test_app";
  }

  std::string cm2_moniker() {
    return "realm_builder:" + realm_->component().GetChildName() + "/test_app_2";
  }

  void CheckLog(std::optional<diagnostics::reader::LogsData> maybeLog,
                fuchsia_diagnostics_types::Severity severity, const std::string& message,
                const std::vector<std::string>& tags) {
    ASSERT_TRUE(maybeLog.has_value());
    auto& log = maybeLog.value();
    EXPECT_EQ(log.moniker(), "realm_builder:" + realm_->component().GetChildName() + "/test_app");
    EXPECT_EQ(log.version(), 1u);
    EXPECT_EQ(log.message(), message);
    auto metadata = log.metadata();
    std::string url_ending = "/test_app";
    EXPECT_TRUE(
        std::equal(url_ending.rbegin(), url_ending.rend(), metadata.component_url.rbegin()));
    EXPECT_NE(metadata.timestamp, 0u);
    EXPECT_EQ(metadata.severity, severity);
    EXPECT_EQ(metadata.tags, tags);
    EXPECT_TRUE(metadata.pid.has_value());
    EXPECT_TRUE(metadata.tid.has_value());
  }

 private:
  std::unique_ptr<RealmRoot> realm_;
  async::Executor executor_;
  std::unique_ptr<sys::ComponentContext> component_context_;
};

using InspectResult = fpromise::result<std::vector<diagnostics::reader::InspectData>, std::string>;
using LogsResult = fpromise::result<std::optional<diagnostics::reader::LogsData>, std::string>;

TEST_F(ArchiveReaderTest, ReadHierarchyAPI1) {
  diagnostics::reader::ArchiveReader reader(executor().dispatcher(),
                                            {cm1_selector(), cm2_selector()});

  InspectResult result;
  executor().schedule_task(reader.SnapshotInspectUntilPresent({cm1_moniker(), cm2_moniker()})
                               .then([&](InspectResult& r) { result = std::move(r); }));
  RunLoopUntil([&] { return !!result; });

  ASSERT_TRUE(result.is_ok()) << "Error: " << result.error();

  auto value = result.take_value();
  std::sort(value.begin(), value.end(), [](auto& a, auto& b) { return a.moniker() < b.moniker(); });

  EXPECT_EQ(cm1_moniker(), value[0].moniker());
  EXPECT_EQ(cm2_moniker(), value[1].moniker());

  EXPECT_STREQ("v1", value[0].content()["root"]["version"].GetString());
  EXPECT_STREQ("v1", value[1].content()["root"]["version"].GetString());
}

TEST_F(ArchiveReaderTest, ReadHierarchyAPI2) {
  diagnostics::reader::ArchiveReader reader(executor().dispatcher());

  {
    InspectResult result;
    executor().schedule_task(reader.SetSelectors({cm1_selector()})
                                 .SnapshotInspectUntilPresent({cm1_moniker()})
                                 .then([&](InspectResult& r) { result = std::move(r); }));
    RunLoopUntil([&] { return !!result; });

    ASSERT_TRUE(result.is_ok()) << "Error: " << result.error();

    auto value = result.take_value();

    EXPECT_EQ(1ul, value.size());
    EXPECT_EQ(cm1_moniker(), value[0].moniker());
    EXPECT_STREQ("v1", value[0].content()["root"]["version"].GetString());
  }

  {
    InspectResult result;

    // reset selectors to only cm2
    executor().schedule_task(reader.SetSelectors({cm2_selector()})
                                 .SnapshotInspectUntilPresent({cm2_moniker()})
                                 .then([&](InspectResult& r) { result = std::move(r); }));
    RunLoopUntil([&] { return !!result; });

    ASSERT_TRUE(result.is_ok()) << "Error: " << result.error();

    auto value = result.take_value();

    EXPECT_EQ(1ul, value.size());
    EXPECT_EQ(cm2_moniker(), value[0].moniker());
    EXPECT_STREQ("v1", value[0].content()["root"]["version"].GetString());
  }

  {
    InspectResult result;

    // last set only cm2 selector, this adds cm1
    executor().schedule_task(reader.AddSelectors({cm1_selector()})
                                 .SnapshotInspectUntilPresent({cm1_moniker(), cm2_moniker()})
                                 .then([&](InspectResult& r) { result = std::move(r); }));
    RunLoopUntil([&] { return !!result; });

    ASSERT_TRUE(result.is_ok()) << "Error: " << result.error();

    auto value = result.take_value();
    std::sort(value.begin(), value.end(),
              [](auto& a, auto& b) { return a.moniker() < b.moniker(); });

    EXPECT_EQ(2ul, value.size());

    EXPECT_EQ(cm1_moniker(), value[0].moniker());
    EXPECT_EQ(cm2_moniker(), value[1].moniker());

    EXPECT_STREQ("v1", value[0].content()["root"]["version"].GetString());
    EXPECT_STREQ("v1", value[1].content()["root"]["version"].GetString());
  }
}

TEST_F(ArchiveReaderTest, ReadHierarchyWithAlternativeDispatcher) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  async::Executor local_executor(loop.dispatcher());
  diagnostics::reader::ArchiveReader reader(loop.dispatcher(), {cm1_selector(), cm2_selector()});

  InspectResult result;
  local_executor.schedule_task(reader.SnapshotInspectUntilPresent({cm1_moniker(), cm2_moniker()})
                                   .then([&](InspectResult& r) { result = std::move(r); }));

  // Use the alternate loop.
  while (true) {
    loop.Run(zx::deadline_after(zx::msec(10)));
    if (!!result) {
      break;
    }
  }

  ASSERT_TRUE(result.is_ok()) << "Error: " << result.error();

  auto value = result.take_value();
  std::sort(value.begin(), value.end(), [](auto& a, auto& b) { return a.moniker() < b.moniker(); });

  EXPECT_EQ(cm1_moniker(), value[0].moniker());
  EXPECT_EQ(cm2_moniker(), value[1].moniker());

  EXPECT_STREQ("v1", value[0].content()["root"]["version"].GetString());
  EXPECT_STREQ("v1", value[1].content()["root"]["version"].GetString());
}

TEST_F(ArchiveReaderTest, Sort) {
  diagnostics::reader::ArchiveReader reader(dispatcher(), {cm1_selector(), cm2_selector()});

  InspectResult result;
  executor().schedule_task(reader.SnapshotInspectUntilPresent({cm1_moniker(), cm2_moniker()})
                               .then([&](InspectResult& r) { result = std::move(r); }));
  RunLoopUntil([&] { return !!result; });

  ASSERT_TRUE(result.is_ok()) << "Error: " << result.error();

  auto value = result.take_value();
  ASSERT_EQ(2lu, value.size());

  std::sort(value.begin(), value.end(), [](auto& a, auto& b) { return a.moniker() < b.moniker(); });
  value[0].Sort();
  value[1].Sort();

  re2::RE2 timestamp_reg("\"timestamp\": \\d+");
  re2::RE2 url_reg("\"component_url\": \"realm-builder://\\d+/");
  re2::RE2 moniker_reg("\"moniker\": \"realm_builder.+/");

  auto cm1_expected_json = value[0].PrettyJson();
  cm1_expected_json = regex_replace(cm1_expected_json, timestamp_reg, "\"timestamp\": TIMESTAMP");
  cm1_expected_json =
      regex_replace(cm1_expected_json, url_reg, "\"component_url\": \"REALM_BUILDER_URL_PREFIX/");
  cm1_expected_json =
      regex_replace(cm1_expected_json, moniker_reg, "\"moniker\": \"REALM_BUILDER_MONIKER_PREFIX/");

  auto cm2_expected_json = value[1].PrettyJson();
  cm2_expected_json = regex_replace(cm2_expected_json, timestamp_reg, "\"timestamp\": TIMESTAMP");
  cm2_expected_json =
      regex_replace(cm2_expected_json, url_reg, "\"component_url\": \"REALM_BUILDER_URL_PREFIX/");
  cm2_expected_json =
      regex_replace(cm2_expected_json, moniker_reg, "\"moniker\": \"REALM_BUILDER_MONIKER_PREFIX/");

  EXPECT_EQ(CM1_EXPECTED_DATA, cm1_expected_json);
  EXPECT_EQ(CM2_EXPECTED_DATA, cm2_expected_json);
}

TEST_F(ArchiveReaderTest, ReadLogs) {
  diagnostics::reader::ArchiveReader reader(dispatcher(), {cm1_selector()});

  auto subscription = reader.GetLogs(fuchsia_diagnostics::StreamMode::kSnapshotThenSubscribe);

  auto next_log = [&]() -> std::optional<diagnostics::reader::LogsData> {
    LogsResult result;
    executor().schedule_task(
        subscription.Next().then([&](LogsResult& r) { result = std::move(r); }));
    RunLoopUntil([&] { return !!result; });
    return result.take_value();
  };

  CheckLog(next_log(), fuchsia_diagnostics_types::Severity::kInfo, "I'm an info log",
           {"test_program", "hello"});
  CheckLog(next_log(), fuchsia_diagnostics_types::Severity::kWarn, "I'm a warn log",
           {"test_program"});
  CheckLog(next_log(), fuchsia_diagnostics_types::Severity::kError, "I'm an error log",
           {"test_program"});
}

}  // namespace
}  // namespace component
