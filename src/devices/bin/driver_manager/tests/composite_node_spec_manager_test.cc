// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/composite/composite_node_spec_manager.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fit/defer.h>
#include <lib/inspect/cpp/reader.h>

#include <memory>
#include <utility>

#include <gtest/gtest.h>

#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"
#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace {

fdf::CompositeParent MakeCompositeNodeSpecInfo(std::string spec_name, uint32_t index,
                                               std::vector<std::string> specs) {
  return fdf::CompositeParent{{
      .composite = fdf::CompositeInfo{{
          .spec = fdf::CompositeNodeSpec{{
              .name = spec_name,
              .parents2 = std::vector<fdf::ParentSpec2>(specs.size()),
          }},
          .matched_driver = fdf::CompositeDriverMatch{{
              .composite_driver = fdf::CompositeDriverInfo{{
                  .composite_name = "test_composite",
                  .driver_info = fdf::DriverInfo{},
              }},
              .parent_names = specs,
          }},
      }},
      .index = index,
  }};
}

}  // namespace

class FakeNodeManager : public driver_manager::NodeManager {
 public:
  void Bind(driver_manager::Node& node,
            std::shared_ptr<driver_manager::BindResultTracker> result_tracker) override {}
  void Bind(driver_manager::Resource& resource,
            std::shared_ptr<driver_manager::BindResultTracker> result_tracker) override {}

  zx::result<driver_manager::DriverHost*> CreateDriverHost(
      bool use_next_vdso, std::string_view driver_host_name_for_colocation) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  driver_manager::DriverHost* GetDriverHost(
      std::string_view driver_name_name_for_colocation) override {
    return nullptr;
  }
};

class FakeCompositeNodeSpec : public driver_manager::CompositeNodeSpec {
 public:
  explicit FakeCompositeNodeSpec(driver_manager::CompositeNodeSpecCreateInfo create_info,
                                 async_dispatcher_t* dispatcher,
                                 driver_manager::NodeManager* manager)
      : driver_manager::CompositeNodeSpec(create_info, dispatcher, manager),
        parent_resources_(create_info.parents.size(), std::nullopt) {}

  zx::result<std::optional<driver_manager::NodeWkPtr>> BindParent(
      fuchsia_driver_framework::wire::CompositeParent composite_parent,
      const driver_manager::ResourceWkPtr& resource) override {
    ZX_ASSERT(composite_parent.has_index());
    auto node_index = composite_parent.index();
    if (node_index >= parent_resources_.size()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }

    const std::optional<std::weak_ptr<driver_manager::Resource>>& current_at_index =
        parent_resources_[node_index];
    if (current_at_index.has_value()) {
      const std::weak_ptr<driver_manager::Resource>& existing_resource = current_at_index.value();
      if (!existing_resource.expired()) {
        return zx::error(ZX_ERR_ALREADY_BOUND);
      }
    }

    parent_resources_[composite_parent.index()] = resource;
    return zx::ok(std::weak_ptr<driver_manager::Node>());
  }

  fuchsia_driver_development::wire::CompositeNodeInfo GetCompositeInfo(
      fidl::AnyArena& arena) const override {
    return fuchsia_driver_development::wire::CompositeNodeInfo::Builder(arena).Build();
  }

  void Remove(driver_manager::RemoveCompositeNodeCallback callback) override {
    remove_invoked_ = true;
    callback(zx::ok());
  }

  const std::vector<std::optional<driver_manager::ResourceWkPtr>>& GetParentResources()
      const override {
    return parent_resources_;
  }

  std::vector<std::optional<driver_manager::NodeWkPtr>> GetParentNodes() const override {
    std::vector<std::optional<driver_manager::NodeWkPtr>> parent_nodes;
    parent_nodes.reserve(parent_resources_.size());
    for (const auto& parent_resource : parent_resources_) {
      if (parent_resource == std::nullopt) {
        parent_nodes.push_back(std::nullopt);
      } else if (auto resource = parent_resource->lock()) {
        parent_nodes.push_back(resource->owner());
      } else {
        parent_nodes.push_back(std::nullopt);
      }
    }
    return parent_nodes;
  }

  bool remove_invoked() const { return remove_invoked_; }

 private:
  bool remove_invoked_ = false;
  std::vector<std::optional<std::weak_ptr<driver_manager::Resource>>> parent_resources_;
};

class FakeDeviceManagerBridge : public driver_manager::CompositeManagerBridge {
 public:
  // CompositeManagerBridge:
  void BindNodesForCompositeNodeSpec() override {}
  void AddSpecToDriverIndex(fdf::wire::CompositeNodeSpec spec,
                            driver_manager::AddToIndexCallback callback) override {
    if (add_spec_status_ == ZX_OK) {
      callback(zx::ok());
    } else {
      callback(zx::error(add_spec_status_));
    }
  }

  void RequestRebindFromDriverIndex(std::string spec, std::optional<std::string> driver_url_suffix,
                                    fit::callback<void(zx::result<>)> callback) override {
    callback(zx::ok());
  }

  void set_add_spec_status(zx_status_t status) { add_spec_status_ = status; }

 private:
  zx_status_t add_spec_status_ = ZX_OK;
};

class CompositeNodeSpecManagerTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    composite_node_spec_manager_ =
        std::make_unique<driver_manager::CompositeNodeSpecManager>(&bridge_);
  }

  static fdf::ParentSpec2 MakeParentSpec(std::vector<fdf::BindRule2> bind_rules,
                                         std::vector<fdf::NodeProperty2> properties) {
    return fdf::ParentSpec2{{
        .bind_rules = std::move(bind_rules),
        .properties = std::move(properties),
    }};
  }

  fit::result<fuchsia_driver_framework::CompositeNodeSpecError> AddSpec(
      fidl::AnyArena& arena, std::string name, std::vector<fdf::ParentSpec2> parents) {
    auto spec = std::make_unique<FakeCompositeNodeSpec>(
        driver_manager::CompositeNodeSpecCreateInfo{
            .name = name,
            .parents = parents,
        },
        dispatcher(), &node_manager_);
    auto spec_ptr = spec.get();
    std::optional<fit::result<fuchsia_driver_framework::CompositeNodeSpecError>> add_spec_result;

    composite_node_spec_manager_->AddSpec(
        fidl::ToWire(arena, fdf::CompositeNodeSpec{{
                                .name = name,
                                .parents2 = parents,
                            }}),
        std::move(spec), [&add_spec_result](fit::result<fdf::CompositeNodeSpecError> result) {
          add_spec_result = result;
        });
    if (add_spec_result->is_ok()) {
      specs_[name] = spec_ptr;
    }
    return add_spec_result.value();
  }

  std::shared_ptr<driver_manager::Node> CreateNode(const char* name) {
    auto node = std::make_shared<driver_manager::Node>(name, std::weak_ptr<driver_manager::Node>{},
                                                       &node_manager_, loop_.dispatcher());
    node->InitializeSelfResource({}, {}, {});
    return node;
  }

  void VerifyRemoveInvokedForSpec(bool expected, const std::string& name) {
    ZX_ASSERT(specs_[name]);
    ASSERT_EQ(expected, specs_[name]->remove_invoked());
  }

 protected:
  FakeNodeManager node_manager_;

  std::unique_ptr<driver_manager::CompositeNodeSpecManager> composite_node_spec_manager_;

  std::unordered_map<std::string, FakeCompositeNodeSpec*> specs_;
  FakeDeviceManagerBridge bridge_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

TEST_F(CompositeNodeSpecManagerTest, TestAddMatchCompositeNodeSpec) {
  fidl::Arena allocator;

  std::vector<fdf::ParentSpec2> parents{
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("b", 1u)}),
  };

  auto spec_name = "test_name";
  fdf::CompositeParent match = MakeCompositeNodeSpecInfo(spec_name, 0, {"node-0", "node-1"});

  ASSERT_TRUE(AddSpec(allocator, spec_name, std::move(parents)).is_ok());
  ASSERT_EQ(2u, composite_node_spec_manager_->specs().at(spec_name)->GetParentResources().size());
  ASSERT_FALSE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[0]);
  ASSERT_FALSE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[1]);

  //  Bind parent spec 2.
  zx::result result = composite_node_spec_manager_->BindParentSpec(
      allocator,
      fidl::ToWire(allocator,
                   std::vector{MakeCompositeNodeSpecInfo(spec_name, 1, {"node-0", "node-1"})}),
      std::weak_ptr<driver_manager::Resource>());
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(1u, result.value().completed_node_and_drivers.size());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[1]);

  //  Bind parent spec 1.
  ASSERT_TRUE(
      composite_node_spec_manager_
          ->BindParentSpec(allocator,
                           fidl::ToWire(allocator, std::vector{MakeCompositeNodeSpecInfo(
                                                       spec_name, 0, {"node-0", "node-1"})}),
                           std::weak_ptr<driver_manager::Resource>())
          .is_ok());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[0]);
}

TEST_F(CompositeNodeSpecManagerTest, TestBindSameNodeTwice) {
  fidl::Arena allocator;

  std::vector<fdf::ParentSpec2> parents{
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("c", 100u)}),
  };

  auto spec_name = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name, std::move(parents)).is_ok());
  ASSERT_EQ(2u, composite_node_spec_manager_->specs().at(spec_name)->GetParentResources().size());

  ASSERT_FALSE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[0]);
  ASSERT_FALSE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[1]);

  //  Bind parent spec 1.
  std::shared_ptr<driver_manager::Node> node = CreateNode("node");
  ASSERT_TRUE(
      composite_node_spec_manager_
          ->BindParentSpec(allocator,
                           fidl::ToWire(allocator, std::vector{MakeCompositeNodeSpecInfo(
                                                       spec_name, 0, {"node-0", "node-1"})}),
                           node->GetSelfResource().value())
          .is_ok());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[0]);

  // Bind the same node.
  ASSERT_EQ(ZX_ERR_NOT_FOUND,
            composite_node_spec_manager_
                ->BindParentSpec(allocator,
                                 fidl::ToWire(allocator, std::vector{MakeCompositeNodeSpecInfo(
                                                             spec_name, 0, {"node-0", "node-1"})}),
                                 node->GetSelfResource().value())
                .status_value());
}

TEST_F(CompositeNodeSpecManagerTest, FailedDriverIndexCall) {
  fidl::Arena allocator;

  std::vector<fdf::ParentSpec2> parents{
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("c", 100u)}),
  };

  bridge_.set_add_spec_status(ZX_ERR_INTERNAL);

  auto spec_name = "test_name";

  auto result = AddSpec(allocator, spec_name, std::move(parents));
  ASSERT_FALSE(result.is_ok());
  EXPECT_EQ(fuchsia_driver_framework::CompositeNodeSpecError::kDriverIndexFailure,
            result.error_value());
}

TEST_F(CompositeNodeSpecManagerTest, TestMultibindDisabled) {
  fidl::Arena allocator;

  auto shared_bind_rules = std::vector{
      fdf::MakeAcceptBindRule("f", 10u),
  };
  auto shared_props = std::vector{
      fdf::MakeProperty2("c", 10u),
  };

  // Add the first composite node spec.
  std::vector<fdf::ParentSpec2> parent_specs_1{
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("d", 1u)}),
      MakeParentSpec(shared_bind_rules, shared_props),
  };

  auto spec_name_1 = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name_1, parent_specs_1).is_ok());
  ASSERT_EQ(2u, composite_node_spec_manager_->specs().at(spec_name_1)->GetParentResources().size());

  // Add a second composite node spec with a node that's the same as one in the first composite node
  // spec.
  std::vector<fdf::ParentSpec2> parent_specs_2{
      MakeParentSpec(shared_bind_rules, shared_props),
  };
  auto spec_name_2 = "test_name2";
  ASSERT_TRUE(AddSpec(allocator, spec_name_2, parent_specs_2).is_ok());
  ASSERT_EQ(1u, composite_node_spec_manager_->specs().at(spec_name_2)->GetParentResources().size());

  // Bind the node that's in both specs. The node should only bind to one
  // composite node spec.
  auto matched_node = std::vector{
      MakeCompositeNodeSpecInfo(spec_name_1, 1, {"node-0", "node-1"}),
      MakeCompositeNodeSpecInfo(spec_name_2, 0, {"node-0"}),
  };

  std::shared_ptr<driver_manager::Node> node_1 = CreateNode("node_1");
  std::shared_ptr<driver_manager::Node> node_2 = CreateNode("node_2");
  zx::result result = composite_node_spec_manager_->BindParentSpec(
      allocator, fidl::ToWire(allocator, matched_node), node_1->GetSelfResource().value(), false);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(1u, result.value().completed_node_and_drivers.size());

  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name_1)->GetParentResources()[1]);
  ASSERT_FALSE(composite_node_spec_manager_->specs().at(spec_name_2)->GetParentResources()[0]);

  // Bind the node again. Both composite node specs should now have the bound node.
  ASSERT_TRUE(composite_node_spec_manager_
                  ->BindParentSpec(allocator, fidl::ToWire(allocator, matched_node),
                                   node_2->GetSelfResource().value(), false)
                  .is_ok());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name_1)->GetParentResources()[1]);
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name_2)->GetParentResources()[0]);
}

TEST_F(CompositeNodeSpecManagerTest, TestMultibindEnabled) {
  fidl::Arena allocator;

  auto shared_bind_rules = std::vector{
      fdf::MakeAcceptBindRule("g", 10u),
  };
  auto shared_props = std::vector{
      fdf::MakeProperty2("c", 10u),
  };

  // Add the first composite node spec.
  std::vector<fdf::ParentSpec2> parent_specs_1{
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("d", 1u)}),
      MakeParentSpec(shared_bind_rules, shared_props),
  };

  auto spec_name_1 = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name_1, parent_specs_1).is_ok());
  ASSERT_EQ(2u, composite_node_spec_manager_->specs().at(spec_name_1)->GetParentResources().size());

  // Add a second composite node spec with a node that's the same as one in the first composite node
  // spec.
  std::vector<fdf::ParentSpec2> parent_specs_2{
      MakeParentSpec(shared_bind_rules, shared_props),
  };
  auto spec_name_2 = "test_name2";
  ASSERT_TRUE(AddSpec(allocator, spec_name_2, parent_specs_2).is_ok());
  ASSERT_EQ(1u, composite_node_spec_manager_->specs().at(spec_name_2)->GetParentResources().size());

  // Bind the node that's in both specs. The node should bind to both.
  auto matched_node = std::vector{
      MakeCompositeNodeSpecInfo(spec_name_1, 1, {"node-0", "node-1"}),
      MakeCompositeNodeSpecInfo(spec_name_2, 0, {"node-0"}),
  };

  zx::result result =
      composite_node_spec_manager_->BindParentSpec(allocator, fidl::ToWire(allocator, matched_node),
                                                   std::weak_ptr<driver_manager::Resource>(), true);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(2u, result.value().completed_node_and_drivers.size());

  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name_1)->GetParentResources()[1]);
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name_2)->GetParentResources()[0]);
}

TEST_F(CompositeNodeSpecManagerTest, TestBindWithNoCompositeMatch) {
  fidl::Arena allocator;
  std::vector<fdf::ParentSpec2> parent_specs{
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("no_match", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("no_match_2", 1u)}),
  };

  auto spec_name = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name, parent_specs).is_ok());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name));

  //  Bind parent spec 1 with no composite driver.
  auto matched_node = std::vector{
      fuchsia_driver_framework::CompositeParent{{
          .composite = fuchsia_driver_framework::CompositeInfo{{
              .spec = fuchsia_driver_framework::CompositeNodeSpec{{
                  .name = spec_name,
                  .parents2 = std::vector<fuchsia_driver_framework::ParentSpec2>(2),
              }},
          }},
          .index = 0,
      }},
  };

  ASSERT_EQ(ZX_ERR_NOT_FOUND, composite_node_spec_manager_
                                  ->BindParentSpec(allocator, fidl::ToWire(allocator, matched_node),
                                                   std::weak_ptr<driver_manager::Resource>())
                                  .status_value());

  // Add a composite match into the matched node info.
  // Reattempt binding the parent spec 1. With a matched composite driver, it should
  // now bind successfully.
  auto matched_node_with_composite = std::vector{
      MakeCompositeNodeSpecInfo(spec_name, 0, {"node-0", "node-1"}),
  };
  ASSERT_TRUE(composite_node_spec_manager_
                  ->BindParentSpec(allocator, fidl::ToWire(allocator, matched_node_with_composite),
                                   std::weak_ptr<driver_manager::Resource>())
                  .is_ok());
  ASSERT_EQ(2u, composite_node_spec_manager_->specs().at(spec_name)->GetParentResources().size());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[0]);
}

TEST_F(CompositeNodeSpecManagerTest, TestAddDuplicate) {
  fidl::Arena allocator;
  std::vector<fdf::ParentSpec2> parent_specs{
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("a", 1u)}),
  };

  auto spec_name = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name, parent_specs).is_ok());
  ASSERT_EQ(fuchsia_driver_framework::CompositeNodeSpecError::kAlreadyExists,
            AddSpec(allocator, spec_name, std::move(parent_specs)).error_value());
}

TEST_F(CompositeNodeSpecManagerTest, TestDuplicateSpecsWithMatch) {
  fidl::Arena allocator;
  std::vector<fdf::ParentSpec2> parent_specs{
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("e", 10u)}),
  };

  auto spec_name = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name, parent_specs).is_ok());
  ASSERT_EQ(2u, composite_node_spec_manager_->specs().at(spec_name)->GetParentResources().size());
  ASSERT_EQ(fuchsia_driver_framework::CompositeNodeSpecError::kAlreadyExists,
            AddSpec(allocator, spec_name, std::move(parent_specs)).error_value());
}

TEST_F(CompositeNodeSpecManagerTest, TestRebindRequestWithNoMatch) {
  fidl::Arena allocator;
  std::vector<fdf::ParentSpec2> parent_specs{
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("e", 10u)}),
  };

  std::string spec_name = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name, parent_specs).is_ok());

  bool is_callback_success = false;
  composite_node_spec_manager_->Rebind(spec_name, std::nullopt,
                                       [&is_callback_success](zx::result<> result) {
                                         if (result.is_ok()) {
                                           is_callback_success = true;
                                         }
                                       });
  ASSERT_TRUE(is_callback_success);
  VerifyRemoveInvokedForSpec(true, spec_name);
}

TEST_F(CompositeNodeSpecManagerTest, TestRebindRequestWithMatch) {
  fidl::Arena allocator;
  std::vector<fdf::ParentSpec2> parent_specs{
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("e", 10u)}),
  };

  std::string spec_name = "test_name";
  ASSERT_TRUE(AddSpec(allocator, spec_name, parent_specs).is_ok());

  auto matched_parent_1 = std::vector{
      MakeCompositeNodeSpecInfo(spec_name, 0, {"node-0", "node-1"}),
  };
  zx::result result = composite_node_spec_manager_->BindParentSpec(
      allocator, fidl::ToWire(allocator, matched_parent_1),
      std::weak_ptr<driver_manager::Resource>());
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(1u, result.value().completed_node_and_drivers.size());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[0]);

  auto matched_parent_2 = std::vector{
      MakeCompositeNodeSpecInfo(spec_name, 1, {"node-0", "node-1"}),
  };
  ASSERT_EQ(1u, composite_node_spec_manager_
                    ->BindParentSpec(allocator, fidl::ToWire(allocator, matched_parent_2),
                                     std::weak_ptr<driver_manager::Resource>())
                    .value()
                    .completed_node_and_drivers.size());
  ASSERT_TRUE(composite_node_spec_manager_->specs().at(spec_name)->GetParentResources()[1]);

  bool is_callback_success = false;
  composite_node_spec_manager_->Rebind(spec_name, std::nullopt,
                                       [&is_callback_success](zx::result<> result) {
                                         if (result.is_ok()) {
                                           is_callback_success = true;
                                         }
                                       });
  ASSERT_TRUE(is_callback_success);
  VerifyRemoveInvokedForSpec(true, spec_name);
}

TEST_F(CompositeNodeSpecManagerTest, TestRecordInspect) {
  fidl::Arena allocator;
  std::vector<fdf::ParentSpec2> parents{
      MakeParentSpec({fdf::MakeAcceptBindRule("a", 10u)}, {fdf::MakeProperty2("a", 1u)}),
      MakeParentSpec({fdf::MakeAcceptBindRule("b", 1u)}, {fdf::MakeProperty2("b", 1u)}),
  };

  std::string spec_name = "test_spec";
  ASSERT_TRUE(AddSpec(allocator, spec_name, std::move(parents)).is_ok());

  inspect::Inspector inspector;
  composite_node_spec_manager_->RecordInspect(inspector);

  auto hierarchy_result = inspect::ReadFromVmo(inspector.DuplicateVmo());
  ASSERT_TRUE(hierarchy_result.is_ok());
  auto hierarchy = hierarchy_result.take_value();
  ASSERT_EQ(1ul, hierarchy.children().size());
  EXPECT_EQ("composite_node_specs", hierarchy.children()[0].node().name());
  ASSERT_EQ(1ul, hierarchy.children()[0].children().size());
  EXPECT_EQ(spec_name, hierarchy.children()[0].children()[0].node().name());
  EXPECT_EQ(1ul, hierarchy.children()[0].children()[0].children().size());
  EXPECT_EQ("parents", hierarchy.children()[0].children()[0].children()[0].node().name());
  EXPECT_EQ(2ul, hierarchy.children()[0].children()[0].children()[0].children().size());
}
