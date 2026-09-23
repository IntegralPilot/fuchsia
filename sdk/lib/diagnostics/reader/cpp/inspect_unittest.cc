// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/diagnostics/reader/cpp/archive_reader.h>

#include <optional>

#include <rapidjson/document.h>
#include <zxtest/zxtest.h>

#include "lib/inspect/cpp/hierarchy.h"

namespace {

using diagnostics::reader::InspectData;

TEST(InspectDataTest, ComponentNameExtraction) {
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"moniker": "root/hub/my_component"})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &datum = data[0];
    EXPECT_EQ("root/hub/my_component", datum.moniker());
  }
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"moniker": "abcd"})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &datum = data[0];
    EXPECT_EQ("abcd", datum.moniker());
  }
  {
    // Can't find path, empty return.
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"not_moniker": "abcd"})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &datum = data[0];
    EXPECT_EQ("", datum.moniker());
  }
}

TEST(InspectDataTest, ContentExtraction) {
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"payload": {"value": "hello", "count": 10}})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &datum = data[0];
    EXPECT_EQ(rapidjson::Value("hello"), datum.GetByPath({"value"}));
    EXPECT_EQ(rapidjson::Value(10), datum.GetByPath({"count"}));
    EXPECT_EQ(rapidjson::Value(), datum.GetByPath({"value", "1234"}));
  }
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"payload": {"name/with/slashes": "hello"}})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &datum = data[0];
    EXPECT_EQ(rapidjson::Value("hello"), datum.GetByPath({"name/with/slashes"}));
  }
  {
    // Content is missing, return nullptr.
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"moniker": "root/hub/my_component"})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &datum = data[0];
    EXPECT_EQ(rapidjson::Value(), datum.GetByPath({"value"}));
  }
}

TEST(InspectDataTest, ArrayValueCtor) {
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"([
      {"payload": {"value": "hello", "count": 10}},
      {"payload": {"value": "world", "count": 40}}
    ])");

    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    InspectData &first = data[0];
    InspectData &second = data[1];

    EXPECT_EQ(rapidjson::Value("hello"), first.GetByPath({"value"}));
    EXPECT_EQ(rapidjson::Value(10), first.GetByPath({"count"}));
    EXPECT_EQ(rapidjson::Value(), first.GetByPath({"value", "1234"}));

    EXPECT_EQ(rapidjson::Value("world"), second.GetByPath({"value"}));
    EXPECT_EQ(rapidjson::Value(40), second.GetByPath({"count"}));
    EXPECT_EQ(rapidjson::Value(), second.GetByPath({"value", "1234"}));
  }
}

TEST(InspectDataTest, EmptyArray) {
  std::vector<diagnostics::reader::InspectData> data;
  rapidjson::Document doc;
  doc.Parse(R"({"payload": { "root": { "test_array": []}}})");
  diagnostics::reader::EmplaceInspect(std::move(doc), &data);
  EXPECT_EQ(true, data[0].payload().has_value());
  EXPECT_EQ("root", data[0].payload().value()->name());

  auto *values =
      data[0].payload().value()->node().get_property<inspect::IntArrayValue>("test_array");
  ASSERT_NOT_NULL(values);
  ASSERT_EQ(0ul, values->value().size());
}

TEST(InspectDataTest, ParseJSON) {
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    EXPECT_EQ("", data[0].moniker());
    EXPECT_EQ(0, data[0].version());
    EXPECT_EQ(std::nullopt, data[0].metadata().filename);
    EXPECT_EQ(std::nullopt, data[0].metadata().name);
    EXPECT_EQ(std::nullopt, data[0].metadata().component_url);
    EXPECT_EQ(0, data[0].metadata().timestamp);
    EXPECT_EQ(std::nullopt, data[0].metadata().errors);
    EXPECT_EQ(std::nullopt, data[0].payload());
  }
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"metadata":
      {
        "filename": "fuchsia.inspect.Tree",
        "timestamp": 39085389926
      },
      "moniker": "bootstrap/archivist",
      "version": 1})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    EXPECT_EQ("bootstrap/archivist", data[0].moniker());
    EXPECT_EQ(1, data[0].version());
    EXPECT_EQ("fuchsia.inspect.Tree", data[0].metadata().filename);
    EXPECT_EQ(std::nullopt, data[0].metadata().name);
    EXPECT_EQ(std::nullopt, data[0].metadata().component_url);
    EXPECT_EQ(39085389926, data[0].metadata().timestamp);
    EXPECT_EQ(std::nullopt, data[0].metadata().errors);
    EXPECT_EQ(std::nullopt, data[0].payload());
  }
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"metadata":
      {
        "name": "MyInspectTree",
        "timestamp": 39085389926
      },
      "moniker": "bootstrap/archivist",
      "version": 1})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    EXPECT_EQ("bootstrap/archivist", data[0].moniker());
    EXPECT_EQ(1, data[0].version());
    EXPECT_EQ(std::nullopt, data[0].metadata().filename);
    EXPECT_EQ("MyInspectTree", data[0].metadata().name);
    EXPECT_EQ(std::nullopt, data[0].metadata().component_url);
    EXPECT_EQ(39085389926, data[0].metadata().timestamp);
    EXPECT_EQ(std::nullopt, data[0].metadata().errors);
    EXPECT_EQ(std::nullopt, data[0].payload());
  }
  {
    std::vector<diagnostics::reader::InspectData> data;
    rapidjson::Document doc;
    doc.Parse(R"({"metadata":
      {
        "filename": "fuchsia.inspect.Tree",
        "component_url": "fuchsia-pkg://fuchsia.com/archivist#meta/archivist.cm",
        "timestamp": 39085389926,
        "errors": [{ "message": "e1"},{ "message": "e2"},{ "message": "e3"}]
      },
      "payload": {
        "root": {
          "all_archive_accessor_node": {
            "archive_accessor_connections_opened": 2,
            "archive_accessor_connections_closed": 0,
            "batch_iterator_connection1": {
              "batch_iterator_get_next_requests": 1,
              "batch_iterator_get_next_responses": 0
            }
          },
          "event_stats": {
            "test_array": [1, 2, 3, 4, 5, 6, 7, 8 ],
            "sources": {
              "v2": {},
              "v1": {}
            }
          },
          "fuchsia.inspect.Health": {
            "start_timestamp_nanos": 3358357188,
            "status": "OK"
          },
          "test": 5000,
          "test1": true
        }
      },
      "moniker": "bootstrap/archivist",
      "version": 1})");
    diagnostics::reader::EmplaceInspect(std::move(doc), &data);
    EXPECT_EQ("bootstrap/archivist", data[0].moniker());
    EXPECT_EQ(1, data[0].version());
    EXPECT_EQ("fuchsia.inspect.Tree", data[0].metadata().filename);
    EXPECT_EQ(std::nullopt, data[0].metadata().name);
    EXPECT_EQ("fuchsia-pkg://fuchsia.com/archivist#meta/archivist.cm",
              data[0].metadata().component_url);
    EXPECT_EQ(39085389926, data[0].metadata().timestamp);
    EXPECT_EQ(3, data[0].metadata().errors->size());
    EXPECT_EQ("e1", data[0].metadata().errors->at(0).message);
    EXPECT_EQ("e2", data[0].metadata().errors->at(1).message);
    EXPECT_EQ("e3", data[0].metadata().errors->at(2).message);
    EXPECT_EQ(true, data[0].payload().has_value());
    EXPECT_EQ("root", data[0].payload().value()->name());
    auto root = data[0].payload().value();
    auto &rootNode = data[0].payload().value()->node();
    const auto accessor = root->GetByPath({"all_archive_accessor_node"});
    ASSERT_NOT_NULL(accessor);
    ASSERT_NOT_NULL(accessor->node().get_property<inspect::IntPropertyValue>(
        "archive_accessor_connections_opened"));
    EXPECT_EQ(2, accessor->node()
                     .get_property<inspect::IntPropertyValue>("archive_accessor_connections_opened")
                     ->value());
    ASSERT_NOT_NULL(accessor->node().get_property<inspect::IntPropertyValue>(
        "archive_accessor_connections_closed"));
    EXPECT_EQ(0, accessor->node()
                     .get_property<inspect::IntPropertyValue>("archive_accessor_connections_closed")
                     ->value());
    auto batchInteratorConnection = accessor->GetByPath({"batch_iterator_connection1"});
    ASSERT_NOT_NULL(batchInteratorConnection);
    ASSERT_NOT_NULL(batchInteratorConnection->node().get_property<inspect::IntPropertyValue>(
        "batch_iterator_get_next_requests"));
    EXPECT_EQ(1, batchInteratorConnection->node()
                     .get_property<inspect::IntPropertyValue>("batch_iterator_get_next_requests")
                     ->value());
    ASSERT_NOT_NULL(batchInteratorConnection->node().get_property<inspect::IntPropertyValue>(
        "batch_iterator_get_next_responses"));
    EXPECT_EQ(0, batchInteratorConnection->node()
                     .get_property<inspect::IntPropertyValue>("batch_iterator_get_next_responses")
                     ->value());
    const auto &health = root->GetByPath({"fuchsia.inspect.Health"});
    ASSERT_NOT_NULL(health);
    EXPECT_EQ(
        3358357188,
        health->node().get_property<inspect::IntPropertyValue>("start_timestamp_nanos")->value());
    EXPECT_EQ("OK", health->node().get_property<inspect::StringPropertyValue>("status")->value());
    const auto &event_stats = root->GetByPath({"event_stats"});
    ASSERT_NOT_NULL(event_stats);
    ASSERT_NOT_NULL(event_stats->node().get_property<inspect::IntArrayValue>("test_array"));
    EXPECT_EQ(1,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[0]);
    EXPECT_EQ(2,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[1]);
    EXPECT_EQ(3,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[2]);
    EXPECT_EQ(4,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[3]);
    EXPECT_EQ(5,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[4]);
    EXPECT_EQ(6,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[5]);
    EXPECT_EQ(7,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[6]);
    EXPECT_EQ(8,
              event_stats->node().get_property<inspect::IntArrayValue>("test_array")->value()[7]);

    const auto &v1 = event_stats->GetByPath({"sources", "v1"});
    ASSERT_NOT_NULL(v1);
    const auto &v2 = event_stats->GetByPath({"sources", "v2"});
    ASSERT_NOT_NULL(v2);
    EXPECT_EQ(2, rootNode.properties().size());
    ASSERT_NOT_NULL(rootNode.get_property<inspect::IntPropertyValue>("test"));
    EXPECT_EQ(5000, rootNode.get_property<inspect::IntPropertyValue>("test")->value());
    ASSERT_NOT_NULL(rootNode.get_property<inspect::BoolPropertyValue>("test1"));
    EXPECT_EQ(true, rootNode.get_property<inspect::BoolPropertyValue>("test1")->value());
  }
}

}  // namespace
