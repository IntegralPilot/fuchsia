// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/diagnostics/reader/cpp/logs.h>

#include <rapidjson/document.h>
#include <zxtest/zxtest.h>

namespace {

using diagnostics::reader::LogsData;

TEST(LogsDataTest, ErrorExtraction) {
  std::vector<diagnostics::reader::LogsData> data;
  rapidjson::Document doc;
  doc.Parse(R"({
      "moniker": "moniker",
      "version": 1,
      "data_source": "Logs",
      "payload": {
       "root": {
           "message":{
               "value":"test"
           }
       }
      },
      "metadata": {
        "component_url": "url",
        "errors": [
          {"dropped_logs": {"count": 2}},
          {"rolled_out_logs": {"count": 4}},
          {"parse_record": "bad format"},
          {"other": {"message": "something unexpected happened"}}
        ],
        "file": "test file.cc",
        "line": 420,
        "pid": 1001,
        "severity": "INFO",
        "tags": ["You're", "IT!"],
        "tid": 200,
        "timestamp": 0
      }
    })");
  auto log = LogsData(std::move(doc));
  EXPECT_EQ(log.metadata().errors.size(), 4);

  auto& error = std::get<LogsData::DroppedLogsError>(log.metadata().errors[0]);
  EXPECT_EQ(error.count, 2);

  auto& error2 = std::get<LogsData::RolledOutLogsError>(log.metadata().errors[1]);
  EXPECT_EQ(error2.count, 4);

  auto& error3 = std::get<LogsData::FailedToParseRecordError>(log.metadata().errors[2]);
  EXPECT_EQ(error3.message, "bad format");

  auto& error4 = std::get<LogsData::OtherError>(log.metadata().errors[3]);
  EXPECT_EQ(error4.message, "something unexpected happened");
}

TEST(LogsDataTest, StructuredKeys) {
  std::vector<diagnostics::reader::LogsData> data;
  rapidjson::Document doc;
  doc.Parse(R"({
      "moniker": "moniker",
      "version": 1,
      "data_source": "Logs",
      "payload": {
       "root": {
           "message":{
               "value":"test"
           },
           "keys": {
            "an_int": -3,
            "a_uint": 3,
            "a_double": 3.2,
            "a_string": "hello",
            "a_boolean": true,
            "a_uint_array": [1,2,3],
            "an_int_array": [-1,2,3],
            "a_double_array": [5.25],
            "a_string_array": ["foo", "bar"]
           }
       }
      },
      "metadata": {
        "component_url": "url",
        "errors": [],
        "file": "test file.cc",
        "line": 420,
        "pid": 1001,
        "severity": "INFO",
        "tags": ["You're", "IT!"],
        "tid": 200,
        "timestamp": 0
      }
    })");
  auto log = LogsData(std::move(doc));
  EXPECT_EQ(log.keys().size(), 9);

  const auto& keys = log.keys();
  EXPECT_EQ(-3, keys[0].Get<inspect::IntPropertyValue>().value());
  EXPECT_EQ(3, keys[1].Get<inspect::IntPropertyValue>().value());
  EXPECT_EQ(3.2, keys[2].Get<inspect::DoublePropertyValue>().value());
  EXPECT_EQ("hello", keys[3].Get<inspect::StringPropertyValue>().value());
  EXPECT_EQ(true, keys[4].Get<inspect::BoolPropertyValue>().value());
  EXPECT_EQ(std::vector<int64_t>({1, 2, 3}), keys[5].Get<inspect::IntArrayValue>().value());
  EXPECT_EQ(std::vector<int64_t>({-1, 2, 3}), keys[6].Get<inspect::IntArrayValue>().value());
  EXPECT_EQ(std::vector<double>({5.25}), keys[7].Get<inspect::DoubleArrayValue>().value());
  EXPECT_EQ(std::vector<std::string>({"foo", "bar"}),
            keys[8].Get<inspect::StringArrayValue>().value());
}

}  // namespace
