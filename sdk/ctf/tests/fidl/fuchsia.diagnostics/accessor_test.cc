// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/component/cpp/fidl.h>
#include <fuchsia/diagnostics/cpp/fidl.h>
#include <fuchsia/sys2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/result.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/sys/cpp/component_context.h>

#include <rapidjson/document.h>
#include <rapidjson/prettywriter.h>
#include <rapidjson/stringbuffer.h>
#include <re2/re2.h>
#include <zxtest/zxtest.h>

namespace {

class AccessorTest : public zxtest::Test {};
using namespace component_testing;

const char EXPECTED[] = R"JSON({
    "data_source": "Inspect",
    "metadata": {
        "component_url": "COMPONENT_URL",
        "name": "root",
        "timestamp": TIMESTAMP
    },
    "moniker": "MONIKER",
    "payload": {
        "root": {
            "arrays": {
                "doubles": [
                    0.0,
                    0.0,
                    3.5,
                    0.0
                ],
                "ints": [
                    -1,
                    0
                ],
                "uints": [
                    0,
                    2,
                    0
                ]
            },
            "buffers": {
                "bytes": "b64:AQID",
                "string": "foo"
            },
            "exponential_histograms": {
                "double": {
                    "counts": [
                        1.0
                    ],
                    "floor": 1.5,
                    "indexes": [
                        2
                    ],
                    "initial_step": 2.0,
                    "size": 5,
                    "step_multiplier": 3.5
                },
                "int": {
                    "counts": [
                        1
                    ],
                    "floor": -10,
                    "indexes": [
                        2
                    ],
                    "initial_step": 2,
                    "size": 5,
                    "step_multiplier": 3
                },
                "uint": {
                    "counts": [
                        1
                    ],
                    "floor": 1,
                    "indexes": [
                        2
                    ],
                    "initial_step": 2,
                    "size": 5,
                    "step_multiplier": 3
                }
            },
            "linear_histgorams": {
                "double": {
                    "counts": [
                        1.0
                    ],
                    "floor": 1.5,
                    "indexes": [
                        2
                    ],
                    "size": 5,
                    "step": 2.5
                },
                "int": {
                    "counts": [
                        1
                    ],
                    "floor": -10,
                    "indexes": [
                        3
                    ],
                    "size": 5,
                    "step": 2
                },
                "uint": {
                    "counts": [
                        1
                    ],
                    "floor": 1,
                    "indexes": [
                        2
                    ],
                    "size": 5,
                    "step": 2
                }
            },
            "numeric": {
                "bool": true,
                "double": 1.5,
                "int": -1,
                "uint": 1
            }
        }
    },
    "version": 1
})JSON";

struct Sorter {
  bool operator()(const rapidjson::Value::Member& a, const rapidjson::Value::Member& b) const {
    return strcmp(a.name.GetString(), b.name.GetString()) < 0;
  }
};

template <typename T>
void SortJsonValue(T value) {
  if (value->IsObject()) {
    std::sort(value->MemberBegin(), value->MemberEnd(), Sorter());
    for (auto element = value->MemberBegin(); element != value->MemberEnd(); ++element) {
      SortJsonValue(&element->value);
    }
  } else if (value->IsArray()) {
    auto array = value->GetArray();
    for (auto element = value->Begin(); element != value->End(); ++element) {
      SortJsonValue(element);
    }
  }
}

std::string SortJsonFile(std::string input) {
  rapidjson::Document document;
  document.Parse(input.c_str());
  SortJsonValue(&document);
  rapidjson::StringBuffer buffer;
  rapidjson::PrettyWriter<rapidjson::StringBuffer> writer(buffer);
  document.Accept(writer);
  return buffer.GetString();
}

// Tests that reading inspect data returns expected data from the archive accessor.
TEST_F(AccessorTest, StreamDiagnosticsInspect) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  ASSERT_EQ(loop.StartThread(), ZX_OK);

  static constexpr char kInspectPublisher[] = "inspect-publisher";
  static constexpr auto kInspectPublisherUrl = "#meta/inspect-publisher.cm";

  auto context = sys::ComponentContext::Create();

  auto realm = RealmBuilder::Create(context->svc())
                   .AddChild(kInspectPublisher, kInspectPublisherUrl,
                             ChildOptions{.startup_mode = StartupMode::EAGER})
                   .AddRoute(Route{
                       .capabilities = {Protocol{"fuchsia.logger.LogSink"}},
                       .source = ParentRef(),
                       .targets = {ChildRef{kInspectPublisher}},
                   })
                   .Build(loop.dispatcher());

  auto _binder = realm.component().ConnectSync<fuchsia::component::Binder>();

  auto moniker =
      "test_suite/realm_builder\\:" + realm.component().GetChildName() + "/inspect-publisher";
  auto selector = moniker + ":root";
  diagnostics::reader::ArchiveReader reader(loop.dispatcher(), {selector});

  fpromise::result<std::vector<diagnostics::reader::InspectData>, std::string> actual_result;
  fpromise::single_threaded_executor executor;

  // TODO(b/302150818): once the removal of escaping in the moniker response lands, we can switch
  // back to just using SnapshotInspectUntilPresent.
  bool keepTrying = true;
  while (keepTrying) {
    fpromise::result<std::vector<diagnostics::reader::InspectData>, std::string> result;
    executor.schedule_task(reader.GetInspectSnapshot().then(
        [&](fpromise::result<std::vector<diagnostics::reader::InspectData>, std::string>& r) {
          result = std::move(r);
        }));
    executor.run();
    ASSERT_TRUE(result.is_ok());
    for (auto& value : result.value()) {
      auto monikerSuffix = realm.component().GetChildName() + "/inspect-publisher";
      if (value.moniker().find(monikerSuffix) != std::string::npos) {
        keepTrying = false;
        actual_result = std::move(result);
        break;
      }
    }
  }

  EXPECT_TRUE(actual_result.is_ok());

  auto& data = actual_result.value()[0];
  data.Sort();
  std::string actual = data.PrettyJson();
  re2::RE2::GlobalReplace(&actual, re2::RE2("\"component_url\": \".+\""),
                          "\"component_url\": \"COMPONENT_URL\"");
  re2::RE2::GlobalReplace(&actual, re2::RE2("\"moniker\": \".+\""), "\"moniker\": \"MONIKER\"");
  re2::RE2::GlobalReplace(&actual, re2::RE2("        \"errors\": null,\n"), "");

  std::string timestamp;
  EXPECT_TRUE(re2::RE2::PartialMatch(actual, re2::RE2("\"timestamp\": (\\d+)"), &timestamp));

  std::string expected = EXPECTED;

  // Replace non-deterministic expected values.
  re2::RE2::GlobalReplace(&expected, re2::RE2("CHILD_NAME"), realm.component().GetChildName());
  re2::RE2::GlobalReplace(&expected, re2::RE2("TIMESTAMP"), timestamp);

  std::string actual_sorted = SortJsonFile(actual);

  EXPECT_TRUE(expected == actual_sorted, "Histogram format didn't match buckets or params");
}

}  // namespace
