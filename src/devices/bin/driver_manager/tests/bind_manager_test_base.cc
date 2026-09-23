// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/tests/bind_manager_test_base.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/cpp/bind.h>

#include "src/devices/bin/driver_manager/resource.h"

namespace fdi = fuchsia_driver_index;

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

void TestDriverIndex::MatchDriver(MatchDriverRequestView request,
                                  MatchDriverCompleter::Sync& completer) {
  std::optional<uint32_t> id;
  for (auto& property : request->args.properties()) {
    if (property.key.get() == bind_fuchsia::PLATFORM_DEV_INSTANCE_ID) {
      id = property.value.int_value();
    }
  }
  ASSERT_TRUE(id.has_value());
  match_request_count_++;
  completers_[id.value()].push(completer.ToAsync());
}

void TestDriverIndex::AddCompositeNodeSpec(AddCompositeNodeSpecRequestView request,
                                           AddCompositeNodeSpecCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void TestDriverIndex::RebindCompositeNodeSpec(RebindCompositeNodeSpecRequestView request,
                                              RebindCompositeNodeSpecCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void TestDriverIndex::MatchPendingNode(MatchPendingNodeRequestView request,
                                       MatchPendingNodeCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void TestDriverIndex::SetNotifier(
    fuchsia_driver_index::wire::DriverIndexSetNotifierRequest* request,
    SetNotifierCompleter::Sync& completer) {}

fidl::ClientEnd<fdi::DriverIndex> TestDriverIndex::Connect() {
  auto [client_end, server_end] = fidl::Endpoints<fdi::DriverIndex>::Create();
  fidl::BindServer(dispatcher_, std::move(server_end), this);
  return std::move(client_end);
}

void TestDriverIndex::ReplyWithMatch(uint32_t id, zx::result<fdi::MatchDriverResult> result) {
  ASSERT_FALSE(completers_[id].empty());
  auto completer = std::move(completers_[id].front());
  completers_[id].pop();
  match_request_count_--;

  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    return;
  }
  fidl::Arena arena;
  completer.ReplySuccess(fidl::ToWire(arena, result.value()));
}

void TestDriverIndex::VerifyRequestCount(uint32_t id, size_t expected_count) {
  ASSERT_EQ(expected_count, completers_[id].size());
}

void TestBindManagerBridge::AddSpecToDriverIndex(
    fuchsia_driver_framework::wire::CompositeNodeSpec spec,
    driver_manager::AddToIndexCallback callback) {
  callback(zx::ok());
}

void TestBindManagerBridge::AddCompositeNodeSpec(
    std::string composite, std::vector<std::string> parent_names,
    std::vector<fdf::ParentSpec2> parents,
    std::unique_ptr<driver_manager::CompositeNodeSpec> spec) {
  fidl::Arena arena;
  auto fidl_spec = fdf::CompositeNodeSpec{{.name = composite, .parents2 = std::move(parents)}};
  specs_.emplace(composite, CompositeNodeSpecData{
                                .spec = spec.get(),
                                .fidl_info = fdf::CompositeInfo{{
                                    .spec = fidl_spec,
                                    .matched_driver = fdf::CompositeDriverMatch{{
                                        .composite_driver = fdf::CompositeDriverInfo{{
                                            .driver_info = fdf::DriverInfo{{
                                                .url = "fuchsia-boot:///#meta/test.cm",
                                            }},
                                        }},
                                        .parent_names = std::move(parent_names),
                                        .primary_parent_index = 0,
                                    }},
                                }},
                            });

  composite_manager_.AddSpec(
      fidl::ToWire(arena, fidl_spec), std::move(spec),
      [](fit::result<fdf::CompositeNodeSpecError> result) { EXPECT_TRUE(result.is_ok()); });
}

void BindManagerTestBase::SetUp() {
  DriverManagerTestBase::SetUp();

  driver_index_ = std::make_unique<TestDriverIndex>(dispatcher());
  auto client = driver_index_->Connect();

  bridge_ = std::make_unique<TestBindManagerBridge>(
      fidl::WireClient<fdi::DriverIndex>(std::move(client), dispatcher()));

  bind_manager_ = std::make_unique<TestBindManager>(bridge_.get(), &node_manager_, dispatcher());
  node_manager_.set_bind_manager(bind_manager_.get());
  bridge_->set_bind_manager(bind_manager_.get());

  ASSERT_EQ(0u, bind_manager_->NumOrphanedResources());
  VerifyNoOngoingBind();
}

void BindManagerTestBase::TearDown() {
  nodes_.clear();
  DriverManagerTestBase::TearDown();
}

BindManagerTestBase::BindManagerData BindManagerTestBase::CurrentBindManagerData() const {
  return BindManagerTestBase::BindManagerData{
      .driver_index_request_count = driver_index_->NumOfMatchRequests(),
      .orphan_nodes_count = bind_manager_->NumOrphanedResources(),
      .pending_bind_count = bind_manager_->GetPendingRequests().size(),
      .pending_orphan_rebind_count = bind_manager_->GetPendingOrphanRebindCallbacks().size(),
  };
}

void BindManagerTestBase::VerifyBindManagerData(BindManagerTestBase::BindManagerData expected) {
  ASSERT_EQ(expected.driver_index_request_count, driver_index_->NumOfMatchRequests());
  ASSERT_EQ(expected.orphan_nodes_count, bind_manager_->NumOrphanedResources());
  ASSERT_EQ(expected.pending_bind_count, bind_manager_->GetPendingRequests().size());
  ASSERT_EQ(expected.pending_orphan_rebind_count,
            bind_manager_->GetPendingOrphanRebindCallbacks().size());
}

std::shared_ptr<driver_manager::Node> BindManagerTestBase::CreateNode(const std::string name,
                                                                      bool enable_multibind) {
  std::shared_ptr new_node = DriverManagerTestBase::CreateNode(name);
  new_node->set_can_multibind_composites(enable_multibind);
  return new_node;
}

void BindManagerTestBase::AddAndBindNode(
    std::string name, bool enable_multibind,
    std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  // This function should only be called for a new node.
  ASSERT_EQ(nodes_.find(name), nodes_.end());

  auto node = CreateNode(name, enable_multibind);
  auto instance_id = GetOrAddInstanceId(name);
  std::vector<fuchsia_driver_framework::NodeProperty2> node_properties = {
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, instance_id)};
  node->InitializeSelfResource(node_properties);
  nodes_.emplace(name, node);
  InvokeBind(name, std::move(tracker));
}

// This function should only be called when there's no ongoing bind.
// Adds a new node and invoke Bind(). Then complete the bind request with
// no matches. The ongoing bind flag should reset to false and the node
// should be added in the orphaned nodes.
void BindManagerTestBase::AddAndOrphanNode(
    std::string name, bool enable_multibind,
    std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  VerifyNoOngoingBind();

  size_t current_orphan_count = bind_manager_->NumOrphanedResources();

  // Invoke bind for a new node in the bind manager.
  AddAndBindNode(name, enable_multibind, std::move(tracker));
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
  ASSERT_EQ(current_orphan_count, bind_manager_->NumOrphanedResources());

  // Driver index completes the request with no matches for the node. The ongoing
  // bind flag should reset to false and the node should be added in the orphaned nodes.
  DriverIndexReplyWithNoMatch(name);
  VerifyNoOngoingBind();
  ASSERT_EQ(current_orphan_count + 1, bind_manager_->NumOrphanedResources());
}

void BindManagerTestBase::InvokeBind(std::string name,
                                     std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  ASSERT_NE(nodes_.find(name), nodes_.end());
  if (tracker == nullptr) {
    tracker = std::make_shared<driver_manager::BindResultTracker>(
        1, [](fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> info) {});
  }
  auto self_resource = nodes_[name]->GetSelfResource();
  ASSERT_TRUE(self_resource.has_value());
  bind_manager_->Bind(*self_resource.value(), "", tracker);
  RunLoopUntilIdle();
}

void BindManagerTestBase::InvokeBind_EXPECT_BIND_START(
    std::string name, std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  VerifyNoOngoingBind();
  InvokeBind(name, std::move(tracker));
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
}

void BindManagerTestBase::InvokeBind_EXPECT_QUEUED(
    std::string name, std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  auto expected_data = CurrentBindManagerData();
  expected_data.pending_bind_count += 1;
  InvokeBind(name, std::move(tracker));
  VerifyBindManagerData(expected_data);
}

void BindManagerTestBase::AddAndBindNode_EXPECT_BIND_START(
    std::string name, bool enable_multibind,
    std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  VerifyNoOngoingBind();
  // Bind process should begin and send a match request to the Driver Index.
  AddAndBindNode(name, enable_multibind, std::move(tracker));
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
}

void BindManagerTestBase::AddAndBindNode_EXPECT_QUEUED(
    std::string name, bool enable_multibind,
    std::shared_ptr<driver_manager::BindResultTracker> tracker) {
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
  auto expected_data = CurrentBindManagerData();
  expected_data.pending_bind_count += 1;

  // The bind request should be queued. There should be no new driver index MatchDriver
  // requests or orphaned nodes.
  AddAndBindNode(name, enable_multibind, std::move(tracker));
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
  VerifyBindManagerData(expected_data);
}

void BindManagerTestBase::AddCompositeNodeSpec(std::string composite,
                                               std::vector<std::string> parents) {
  std::vector<fdf::ParentSpec2> parent_specs;
  parent_specs.reserve(parents.size());
  for (auto& parent : parents) {
    auto instance_id = GetOrAddInstanceId(parent);
    parent_specs.push_back(fdf::ParentSpec2{
        {.bind_rules = {fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID,
                                                instance_id)},
         .properties = {fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, instance_id)}}});
  }

  auto spec = std::make_unique<driver_manager::CompositeNodeSpec>(
      driver_manager::CompositeNodeSpecCreateInfo{
          .name = composite,
          .parents = parent_specs,
      },
      dispatcher(), &node_manager_);

  bridge_->AddCompositeNodeSpec(composite, std::move(parents), std::move(parent_specs),
                                std::move(spec));
  RunLoopUntilIdle();
}

void BindManagerTestBase::AddCompositeNodeSpec_EXPECT_BIND_START(std::string composite,
                                                                 std::vector<std::string> parents) {
  VerifyNoOngoingBind();
  AddCompositeNodeSpec(composite, std::move(parents));
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
}

void BindManagerTestBase::AddCompositeNodeSpec_EXPECT_QUEUED(std::string composite,
                                                             std::vector<std::string> parents) {
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
  auto expected_data = CurrentBindManagerData();
  expected_data.pending_orphan_rebind_count += 1;
  AddCompositeNodeSpec(composite, std::move(parents));
  VerifyBindManagerData(expected_data);
}

void BindManagerTestBase::InvokeTryBindAllAvailable() {
  bind_manager_->TryBindAllAvailable();
  RunLoopUntilIdle();
}

void BindManagerTestBase::InvokeTryBindAllAvailable_EXPECT_BIND_START() {
  VerifyNoOngoingBind();
  InvokeTryBindAllAvailable();
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
}

void BindManagerTestBase::InvokeTryBindAllAvailable_EXPECT_QUEUED() {
  ASSERT_TRUE(bind_manager_->IsBindOngoing());

  auto expected_data = CurrentBindManagerData();
  expected_data.pending_orphan_rebind_count += 1;

  InvokeTryBindAllAvailable();
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
  VerifyBindManagerData(expected_data);
}

void BindManagerTestBase::DriverIndexReplyWithDriver(std::string node) {
  ASSERT_NE(instance_ids_.find(node), instance_ids_.end());
  auto driver_info = fdf::DriverInfo{{.url = "fuchsia-boot:///#meta/test.cm"}};
  driver_index_->ReplyWithMatch(instance_ids_[node],
                                zx::ok(fdi::MatchDriverResult::WithDriver(driver_info)));
  RunLoopUntilIdle();
}

void BindManagerTestBase::DriverIndexReplyWithComposite(
    std::string node, std::vector<std::pair<std::string, size_t>> matched_specs) {
  std::vector<fdf::CompositeParent> parents;
  parents.reserve(matched_specs.size());
  for (auto& [name, index] : matched_specs) {
    auto match_info = bridge_->specs().at(name).fidl_info;
    parents.push_back(fdf::CompositeParent{{
        .composite = match_info,
        .index = index,
    }});
  }

  driver_index_->ReplyWithMatch(
      instance_ids_[node],
      zx::ok(fdi::MatchDriverResult::WithCompositeParents(std::move(parents))));
  RunLoopUntilIdle();
}

void BindManagerTestBase::DriverIndexReplyWithNoMatch(std::string node) {
  ASSERT_NE(instance_ids_.find(node), instance_ids_.end());
  driver_index_->ReplyWithMatch(instance_ids_[node], zx::error(ZX_ERR_NOT_FOUND));
  RunLoopUntilIdle();
}

void BindManagerTestBase::VerifyNoOngoingBind() {
  ASSERT_EQ(false, bind_manager_->IsBindOngoing());
  ASSERT_TRUE(bind_manager_->GetPendingRequests().empty());
  ASSERT_TRUE(bind_manager_->GetPendingOrphanRebindCallbacks().empty());
}

void BindManagerTestBase::VerifyNoQueuedBind() {
  ASSERT_TRUE(bind_manager_->GetPendingRequests().empty());
  ASSERT_TRUE(bind_manager_->GetPendingOrphanRebindCallbacks().empty());
}

void BindManagerTestBase::VerifyOrphanedNodes(std::vector<std::string> expected_nodes) {
  ASSERT_EQ(expected_nodes.size(), bind_manager_->NumOrphanedResources());
  auto orphaned = bind_manager_->GetOrphanedResources();
  for (const auto& expected : expected_nodes) {
    bool found = false;
    for (auto& [id, resource_weak] : orphaned) {
      if (auto resource = resource_weak.lock()) {
        if (auto owner = resource->owner().lock()) {
          if (owner->MakeComponentMoniker() == expected) {
            found = true;
            break;
          }
        }
      }
    }
    ASSERT_TRUE(found) << "Could not find expected orphaned node: " << expected;
  }
}

void BindManagerTestBase::VerifyMultibindNodes(std::vector<std::string> expected_nodes) {
  auto multibind_nodes = bind_manager_->GetMultibindResources();
  ASSERT_EQ(expected_nodes.size(), multibind_nodes.size());
  for (const auto& expected : expected_nodes) {
    bool found = false;
    for (auto& [id, resource_weak] : multibind_nodes) {
      if (auto resource = resource_weak.lock()) {
        if (auto owner = resource->owner().lock()) {
          if (owner->MakeComponentMoniker() == expected) {
            found = true;
            break;
          }
        }
      }
    }
    ASSERT_TRUE(found) << "Could not find expected multibind node: " << expected;
  }
}

void BindManagerTestBase::VerifyBindOngoingWithRequests(
    std::vector<std::pair<std::string, size_t>> expected_requests) {
  ASSERT_TRUE(bind_manager_->IsBindOngoing());
  size_t expected_count = 0;
  for (auto& [name, count] : expected_requests) {
    driver_index_->VerifyRequestCount(GetOrAddInstanceId(name), count);
    expected_count += count;
  }
  ASSERT_EQ(expected_count, driver_index_->NumOfMatchRequests());
}

void BindManagerTestBase::VerifyPendingBindRequestCount(size_t expected) {
  ASSERT_EQ(expected, bind_manager_->GetPendingRequests().size());
}

void BindManagerTestBase::VerifyCompositeNodeExists(bool expected, std::string spec_name) {
  EXPECT_EQ(expected,
            bridge_->specs().at(spec_name).spec->completed_composite_node() != std::nullopt);
}

uint32_t BindManagerTestBase::GetOrAddInstanceId(std::string node_name) {
  if (instance_ids_.contains(node_name)) {
    return instance_ids_[node_name];
  }

  uint32_t instance_id = static_cast<uint32_t>(instance_ids_.size());
  instance_ids_[node_name] = instance_id;
  return instance_id;
}

TEST_F(BindManagerTestBase, TestAddNode) {
  AddAndOrphanNode("test-1");
  ASSERT_EQ(1u, nodes().size());
  ASSERT_EQ(1u, instance_ids().size());

  auto test_node_1 = nodes()["test-1"];
  ASSERT_TRUE(test_node_1);
  ASSERT_EQ(1u, test_node_1->properties_size());
  const auto test_node_1_properties = test_node_1->GetNodeProperties();
  ASSERT_TRUE(test_node_1_properties.has_value());
  ASSERT_EQ(1u, test_node_1_properties->size());
  const auto& test_node_1_property_1 = test_node_1_properties.value()[0];
  ASSERT_EQ(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, std::string(test_node_1_property_1.key()));
  ASSERT_EQ(static_cast<uint32_t>(0), test_node_1_property_1.value().int_value().value());

  AddAndBindNode("test-2");
  ASSERT_EQ(2u, nodes().size());
  ASSERT_EQ(2u, instance_ids().size());

  auto test_node_2 = nodes()["test-2"];
  ASSERT_TRUE(test_node_2);
  ASSERT_EQ(1u, test_node_2->properties_size());
  const auto& test_node_2_properties = test_node_2->GetNodeProperties();
  ASSERT_TRUE(test_node_2_properties.has_value());
  ASSERT_EQ(1u, test_node_2_properties->size());
  const auto& test_node_2_property_1 = test_node_2_properties.value()[0];
  ASSERT_EQ(bind_fuchsia::PLATFORM_DEV_INSTANCE_ID, std::string(test_node_2_property_1.key()));
  ASSERT_EQ(static_cast<uint32_t>(1), test_node_2_property_1.value().int_value().value());

  // Complete the outstanding request.
  DriverIndexReplyWithNoMatch("test-2");
}
