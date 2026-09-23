// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component/cpp/markers.h>
#include <fidl/fuchsia.component/cpp/wire.h>
#include <fidl/fuchsia.inspect/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fuchsia/diagnostics/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/fpromise/promise.h>
#include <lib/inspect/component/cpp/service.h>
#include <lib/inspect/component/cpp/testing.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

using inspect::Inspector;
using inspect::InspectSettings;
using inspect::Node;
using inspect::testing::TreeClient;
using inspect::testing::TreeNameIteratorClient;

namespace {

class InspectServiceTest : public gtest::RealLoopFixture,
                           public testing::WithParamInterface<uint64_t> {
 public:
  InspectServiceTest()
      : inspector_(Inspector(InspectSettings{.maximum_size = 268435456})),
        executor_(dispatcher()) {}

  Inspector inspector_;

 protected:
  inspect::Node& root() { return inspector_.GetRoot(); }

  TreeClient ConnectFrozenThenLive() {
    return ConnectWithSettings(
        inspect::TreeHandlerSettings{.snapshot_behavior = inspect::TreeServerSendPreference::Frozen(
                                         inspect::TreeServerSendPreference::Type::Live)});
  }

  TreeClient ConnectFrozenThenDeepCopy() {
    return ConnectWithSettings(
        inspect::TreeHandlerSettings{.snapshot_behavior = inspect::TreeServerSendPreference::Frozen(
                                         inspect::TreeServerSendPreference::Type::DeepCopy)});
  }

  TreeClient ConnectPrivate() {
    return ConnectWithSettings(inspect::TreeHandlerSettings{
        .snapshot_behavior = inspect::TreeServerSendPreference::DeepCopy()});
  }

  TreeClient ConnectLive() {
    return ConnectWithSettings(inspect::TreeHandlerSettings{
        .snapshot_behavior = inspect::TreeServerSendPreference::Live()});
  }

  TreeClient ConnectWithSettings(inspect::TreeHandlerSettings settings) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_inspect::Tree>();
    inspect::TreeServer::StartSelfManagedServer(inspector_, settings, dispatcher(),
                                                std::move(endpoints->server));

    return TreeClient{std::move(endpoints->client), dispatcher()};
  }

  TreeClient ConnectVmo() {
    auto endpoints = fidl::CreateEndpoints<fuchsia_inspect::Tree>();
    inspect::TreeServer::StartSelfManagedServer(inspector_.DuplicateVmo(), {}, dispatcher(),
                                                std::move(endpoints->server));

    return TreeClient{std::move(endpoints->client), dispatcher()};
  }

  async::Executor executor_;
};

// The failure tests below are not perfect. They don't make any assertions
// about the the type fallback behavior that is specified. This is because
// triggering the failure of the primary behavior also causes the failure
// of DeepCopy fallback behavior, meaning that all the tests bottom out in
// Live duplicates (Live duplicate is the only behavior that never fails).
// Still, the tests demonstrate that even on failure, data is served via
// fallback.

TEST_F(InspectServiceTest, SingleTreeGetContentFailFrozenCopyAndDoLive) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectFrozenThenLive();

  fpromise::result<zx::vmo> content;
  inspector_.AtomicUpdate([&](Node& n) {
    client->GetContent().Then(
        [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
          ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                        result.error().FormatDescription().c_str());
          content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
        });

    RunLoopUntil([&] { return !!content; });
  });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(std::move(vmo));
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());
}

TEST_F(InspectServiceTest, SingleTreeGetContentFailFrozenCopyAndDoDeepCopy) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectFrozenThenDeepCopy();

  fpromise::result<zx::vmo> content;
  inspector_.AtomicUpdate([&](Node& n) {
    client->GetContent().Then(
        [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
          ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                        result.error().FormatDescription().c_str());
          content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
        });

    RunLoopUntil([&] { return !!content; });
  });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(std::move(vmo));
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());
}

TEST_F(InspectServiceTest, SingleTreeGetContentFailDeepCopy) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectPrivate();

  fpromise::result<zx::vmo> content;
  inspector_.AtomicUpdate([&](Node& n) {
    client->GetContent().Then(
        [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
          ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                        result.error().FormatDescription().c_str());
          content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
        });

    RunLoopUntil([&] { return !!content; });
  });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(std::move(vmo));
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());
}

TEST_F(InspectServiceTest, SingleTreeGetContent) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectFrozenThenLive();

  fpromise::result<zx::vmo> content;
  client->GetContent().Then(
      [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
        ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                      result.error().FormatDescription().c_str());
        content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
      });

  RunLoopUntil([&] { return !!content; });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(std::move(vmo));
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());
}

TEST_F(InspectServiceTest, SingleTreeGetContentDeepCopy) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectPrivate();

  fpromise::result<zx::vmo> content;
  client->GetContent().Then(
      [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
        ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                      result.error().FormatDescription().c_str());
        content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
      });

  RunLoopUntil([&] { return !!content; });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(vmo);
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());

  auto should_not_see = root().CreateInt("val2", 2);
  auto hierarchy_2 = inspect::ReadFromVmo(vmo);
  ASSERT_TRUE(hierarchy_2.is_ok());

  const auto* val_prop_2 =
      hierarchy_2.value().node().get_property<inspect::IntPropertyValue>("val2");
  ASSERT_EQ(nullptr, val_prop_2);
}

TEST_F(InspectServiceTest, SingleTreeGetContentLive) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectLive();

  fpromise::result<zx::vmo> content;
  client->GetContent().Then(
      [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
        ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                      result.error().FormatDescription().c_str());
        content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
      });

  RunLoopUntil([&] { return !!content; });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(vmo);
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());

  auto should_see = root().CreateInt("val2", 2);
  auto hierarchy_2 = inspect::ReadFromVmo(vmo);
  ASSERT_TRUE(hierarchy_2.is_ok());

  const auto* val_prop_2 =
      hierarchy_2.value().node().get_property<inspect::IntPropertyValue>("val2");
  ASSERT_NE(nullptr, val_prop_2);
  ASSERT_EQ(2, val_prop_2->value());
}

TEST_P(InspectServiceTest, ListChildNames) {
  inspect::ValueList values;
  std::vector<std::string> expected_names;
  const auto max = GetParam();
  for (auto i = 0ul; i < max; i++) {
    root().CreateLazyNode(
        "a", []() { return fpromise::make_result_promise<Inspector>(fpromise::error()); }, &values);
    expected_names.push_back(std::string("a-") + std::to_string(i));
  }

  auto client = ConnectFrozenThenLive();
  auto endpoints = fidl::CreateEndpoints<fuchsia_inspect::TreeNameIterator>();

  ASSERT_TRUE(client->ListChildNames(std::move(endpoints->server)).ok());

  bool done = false;
  std::vector<std::string> names_result;
  TreeNameIteratorClient iter(std::move(endpoints->client), dispatcher());
  executor_.schedule_task(inspect::testing::ReadAllChildNames(iter).and_then(
      [&](std::vector<std::string>& promised_names) {
        names_result = std::move(promised_names);
        done = true;
      }));

  RunLoopUntil([&] { return done; });
  ASSERT_EQ(names_result.size(), max);
  std::sort(std::begin(names_result), std::end(names_result));
  std::sort(std::begin(expected_names), std::end(expected_names));
  for (size_t i = 0; i < names_result.size(); i++) {
    ASSERT_EQ(expected_names[i], names_result[i]);
  }
}

INSTANTIATE_TEST_SUITE_P(ListChildren, InspectServiceTest,
                         testing::Values(0, 20, 200, ZX_CHANNEL_MAX_MSG_BYTES));

TEST_F(InspectServiceTest, OpenChild) {
  inspect::ValueList values;
  root().CreateLazyNode(
      "a",
      []() {
        Inspector insp;
        insp.GetRoot().CreateInt("val", 1, &insp);
        return fpromise::make_ok_promise(std::move(insp));
      },
      &values);
  root().CreateLazyNode(
      "b", []() { return fpromise::make_result_promise<Inspector>(fpromise::error()); }, &values);

  auto client = ConnectFrozenThenLive();
  auto iter_endpoints = fidl::CreateEndpoints<fuchsia_inspect::TreeNameIterator>();

  ASSERT_TRUE(client->ListChildNames(std::move(iter_endpoints->server)).ok());

  bool done = false;
  std::vector<std::string> names_result;
  TreeNameIteratorClient iter(std::move(iter_endpoints->client), dispatcher());
  executor_.schedule_task(inspect::testing::ReadAllChildNames(iter).and_then(
      [&](std::vector<std::string>& promised_names) {
        names_result = std::move(promised_names);
        done = true;
      }));

  RunLoopUntil([&] { return done; });
  ASSERT_EQ(names_result.size(), 2ul);

  {
    auto child_endpoints_one = fidl::CreateEndpoints<fuchsia_inspect::Tree>();
    ASSERT_TRUE(client
                    ->OpenChild(fidl::StringView::FromExternal(names_result[0]),
                                std::move(child_endpoints_one->server))
                    .ok());

    auto child_tree_client = TreeClient{std::move(child_endpoints_one->client), dispatcher()};
    fpromise::result<zx::vmo> content;
    child_tree_client->GetContent().Then(
        [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
          ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                        result.error().FormatDescription().c_str());
          content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
        });

    RunLoopUntil([&] { return !!content; });

    auto vmo = content.take_value();
    auto hierarchy = inspect::ReadFromVmo(std::move(vmo));
    ASSERT_TRUE(hierarchy.is_ok());

    const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
    ASSERT_NE(nullptr, val_prop);
    EXPECT_EQ(1, val_prop->value());
  }

  {
    auto child_endpoints_one = fidl::CreateEndpoints<fuchsia_inspect::Tree>();
    ASSERT_TRUE(client
                    ->OpenChild(fidl::StringView::FromExternal(names_result[1]),
                                std::move(child_endpoints_one->server))
                    .ok());

    auto child_tree_client = TreeClient{std::move(child_endpoints_one->client), dispatcher()};
    fpromise::result<zx::vmo> content;
    child_tree_client->GetContent().Then(
        [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
          ASSERT_FALSE(result.ok());
        });
  }
}

TEST_F(InspectServiceTest, ReadSingleLevelIntoHierarchy) {
  inspect::ValueList values;
  root().CreateLazyNode(
      "a",
      []() {
        Inspector insp;
        insp.GetRoot().CreateInt("val", 1, &insp);
        return fpromise::make_ok_promise(std::move(insp));
      },
      &values);
  root().CreateLazyNode(
      "b",
      []() {
        Inspector insp;
        insp.GetRoot().CreateInt("val", 3, &insp);
        return fpromise::make_ok_promise(std::move(insp));
      },
      &values);

  auto client = ConnectFrozenThenLive();
  inspect::Hierarchy hierarchy;

  auto done = false;
  executor_.schedule_task(
      inspect::testing::ReadFromTree(client, dispatcher()).and_then([&](inspect::Hierarchy& h) {
        hierarchy = std::move(h);
        done = true;
      }));

  RunLoopUntil([&] { return done; });

  ASSERT_EQ(2ul, hierarchy.children().size());

  hierarchy.Sort();
  auto* a = hierarchy.children().at(0).node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_EQ(1, a->value());
  auto* b = hierarchy.children().at(1).node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_EQ(3, b->value());
}

TEST_F(InspectServiceTest, ReadMultiLevelIntoHierarchy) {
  inspect::ValueList values;
  root().CreateLazyNode(
      "a",
      []() {
        Inspector insp;
        insp.GetRoot().CreateLazyNode(
            "interior-a",
            []() {
              Inspector interior;
              interior.GetRoot().CreateInt("val", 1, &interior);
              return fpromise::make_ok_promise(std::move(interior));
            },
            &insp);

        return fpromise::make_ok_promise(std::move(insp));
      },
      &values);

  auto client = ConnectFrozenThenLive();
  inspect::Hierarchy hierarchy;

  auto done = false;
  executor_.schedule_task(
      inspect::testing::ReadFromTree(client, dispatcher()).and_then([&](inspect::Hierarchy& h) {
        hierarchy = std::move(h);
        done = true;
      }));

  RunLoopUntil([&] { return done; });

  // failing because children aren't parsed yet
  ASSERT_EQ(1ul, hierarchy.children().size());
  ASSERT_EQ(1ul, hierarchy.children().at(0).children().size());
  auto* a =
      hierarchy.children().at(0).children().at(0).node().get_property<inspect::IntPropertyValue>(
          "val");
  ASSERT_EQ(1, a->value());
}

TEST_F(InspectServiceTest, SingleVmoGetContent) {
  auto val = root().CreateInt("val", 1);
  auto client = ConnectVmo();

  fpromise::result<zx::vmo> content;
  client->GetContent().Then(
      [&](fidl::WireUnownedResult<fuchsia_inspect::Tree::GetContent>& result) {
        ZX_ASSERT_MSG(result.ok(), "Tree::GetContent failed: %s",
                      result.error().FormatDescription().c_str());
        content = fpromise::ok(std::move(result.Unwrap()->content.buffer().vmo));
      });

  RunLoopUntil([&] { return !!content; });

  auto vmo = content.take_value();
  auto hierarchy = inspect::ReadFromVmo(std::move(vmo));
  ASSERT_TRUE(hierarchy.is_ok());

  const auto* val_prop = hierarchy.value().node().get_property<inspect::IntPropertyValue>("val");
  ASSERT_NE(nullptr, val_prop);
  EXPECT_EQ(1, val_prop->value());
}

TEST_F(InspectServiceTest, ListChildNamesFromVmoIsEmpty) {
  inspect::ValueList values;
  std::vector<std::string> expected_names;
  root().RecordLazyNode("a", []() { return fpromise::make_ok_promise(inspect::Inspector{}); });

  auto client = ConnectVmo();
  auto endpoints = fidl::CreateEndpoints<fuchsia_inspect::TreeNameIterator>();

  ASSERT_TRUE(client->ListChildNames(std::move(endpoints->server)).ok());

  bool done = false;
  std::vector<std::string> names_result;
  TreeNameIteratorClient iter(std::move(endpoints->client), dispatcher());
  executor_.schedule_task(inspect::testing::ReadAllChildNames(iter).and_then(
      [&](std::vector<std::string>& promised_names) {
        names_result = std::move(promised_names);
        done = true;
      }));

  RunLoopUntil([&] { return done; });
  ASSERT_TRUE(names_result.empty());
}

TEST_F(InspectServiceTest, ReadFromComponentInspector) {
  auto svc = component::OpenServiceRoot();
  auto client_end =
      component::ConnectAt<fuchsia_component::Binder>(*svc, "InspectorPublisherBinder");
  ASSERT_TRUE(client_end.is_ok());

  fidl::WireSyncClient(std::move(*client_end));

  diagnostics::reader::ArchiveReader reader(dispatcher(), {});

  auto result = RunPromise(reader.SnapshotInspectUntilPresent({"inspector_publisher"}));

  auto data = result.take_value();
  uint64_t app_index;
  bool found = false;
  for (uint64_t i = 0; i < data.size(); i++) {
    if (data.at(i).moniker() == "inspector_publisher") {
      app_index = i;
      found = true;
      break;
    }
  }

  ASSERT_TRUE(found);

  auto& app_data = data.at(app_index);

  ASSERT_EQ(app_data.metadata().name, "ComponentInspector");
  ASSERT_EQ(app_data.metadata().filename, std::nullopt);

  ASSERT_EQ(1, app_data.GetByPath({"root", "val1"}).GetInt());
  ASSERT_EQ(2, app_data.GetByPath({"root", "val2"}).GetInt());
  ASSERT_EQ(3, app_data.GetByPath({"root", "val3"}).GetInt());
  ASSERT_EQ(4, app_data.GetByPath({"root", "val4"}).GetInt());
  ASSERT_EQ(0, app_data.GetByPath({"root", "child", "val"}).GetInt());
  ASSERT_EQ(
      std::string("OK"),
      std::string(app_data.GetByPath({"root", "fuchsia.inspect.Health", "status"}).GetString()));
}

TEST_F(InspectServiceTest, ReadFromPublishedVmo) {
  auto svc = component::OpenServiceRoot();
  auto client_end = component::ConnectAt<fuchsia_component::Binder>(*svc, "VmoPublisherBinder");
  ASSERT_TRUE(client_end.is_ok());

  fidl::WireSyncClient(std::move(*client_end));

  diagnostics::reader::ArchiveReader reader(dispatcher(), {});

  auto result = RunPromise(reader.SnapshotInspectUntilPresent({"vmo_publisher"}));

  auto data = result.take_value();
  uint64_t app_index;
  bool found = false;
  for (uint64_t i = 0; i < data.size(); i++) {
    if (data.at(i).moniker() == "vmo_publisher") {
      app_index = i;
      found = true;
      break;
    }
  }

  ASSERT_TRUE(found);

  auto& vmo_data = data.at(app_index);

  ASSERT_EQ(vmo_data.metadata().name, "VmoServer");
  ASSERT_EQ(vmo_data.metadata().filename, std::nullopt);

  ASSERT_EQ(std::string("only in VMO"), vmo_data.GetByPath({"root", "value1"}).GetString());
  ASSERT_EQ(10, vmo_data.GetByPath({"root", "value2"}).GetInt());
}
}  // namespace
