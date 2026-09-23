// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/fidl.h>

#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/bin/driver_manager/tests/driver_manager_test_base.h"

class FakeDictionaryUtil : public driver_manager::DictionaryUtil {
 public:
  FakeDictionaryUtil(async_dispatcher_t* dispatcher)
      : driver_manager::DictionaryUtil(
            fidl::Endpoints<fuchsia_component_sandbox::CapabilityStore>::Create().client,
            dispatcher) {}

  void DictionaryDirConnectorOpen(
      fuchsia_component_sandbox::CapabilityId dictionary, std::string_view key,
      fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> callback) override {
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    callback(zx::ok(std::move(client)));
  }

  void CopyExportDictionary(
      fuchsia_component_sandbox::CapabilityId dictionary,
      fit::callback<void(zx::result<fuchsia_component_sandbox::DictionaryRef>)> callback) override {
    callback(zx::ok(fuchsia_component_sandbox::DictionaryRef(zx::eventpair{})));
  }

  void CreateDictionaryWith(
      std::unordered_map<std::string, fidl::ClientEnd<fuchsia_component_sandbox::DirReceiver>>
          receivers,
      fit::callback<void(zx::result<fuchsia_component_sandbox::CapabilityId>)> callback) override {
    receivers_ = std::move(receivers);
    callback(zx::ok(1234));
  }

  std::unordered_map<std::string, fidl::ClientEnd<fuchsia_component_sandbox::DirReceiver>>
      receivers_;
};

class CompositeNodeSpecTestNodeManager : public TestNodeManagerBase {
 public:
  CompositeNodeSpecTestNodeManager(async_dispatcher_t* dispatcher) : dictionary_util_(dispatcher) {}

  driver_manager::DictionaryUtil& dictionary_util() override { return dictionary_util_; }

  FakeDictionaryUtil dictionary_util_;
};

class CompositeNodeSpecTest : public DriverManagerTestBase {
 public:
  void SetUp() override {
    node_manager_ = std::make_unique<CompositeNodeSpecTestNodeManager>(dispatcher());
    DriverManagerTestBase::SetUp();
    arena_ = std::make_unique<fidl::Arena<512>>();
  }

  driver_manager::NodeManager* GetNodeManager() override { return node_manager_.get(); }

  driver_manager::CompositeNodeSpec CreateCompositeNodeSpec(std::string name, size_t size) {
    std::vector<fuchsia_driver_framework::ParentSpec2> parents(size);
    return driver_manager::CompositeNodeSpec(
        driver_manager::CompositeNodeSpecCreateInfo{
            .name = std::move(name),
            .parents = std::move(parents),
        },
        dispatcher(), node_manager_.get());
  }

  zx::result<std::optional<driver_manager::NodeWkPtr>> MatchAndBindParentSpec(
      driver_manager::CompositeNodeSpec& spec, std::weak_ptr<driver_manager::Node> parent_node,
      std::vector<std::string> parent_names, uint32_t node_index, uint32_t primary_index = 0) {
    fuchsia_driver_framework::CompositeParent matched_parent({
        .composite = fuchsia_driver_framework::CompositeInfo{{
            .spec = fuchsia_driver_framework::CompositeNodeSpec{{
                .name = spec.name(),
                .parents2 = std::vector<fuchsia_driver_framework::ParentSpec2>(parent_names.size()),
            }},
            .matched_driver = fuchsia_driver_framework::CompositeDriverMatch{{
                .composite_driver = fuchsia_driver_framework::CompositeDriverInfo{{
                    .composite_name = "test-composite",
                    .driver_info = fuchsia_driver_framework::DriverInfo{{
                        .url = "fuchsia-boot:///#meta/composite-driver.cm",
                        .colocate = true,
                    }},
                }},
                .parent_names = parent_names,
                .primary_parent_index = primary_index,
            }},
        }},
        .index = node_index,
    });

    auto parent = parent_node.lock();
    ZX_ASSERT(parent);
    auto resource = parent->GetSelfResource();
    ZX_ASSERT(resource.has_value());
    return spec.BindParent(fidl::ToWire(*arena_, matched_parent), resource.value());
  }

  zx::result<std::optional<driver_manager::NodeWkPtr>> MatchAndBindParentSpec(
      driver_manager::CompositeNodeSpec& spec, std::shared_ptr<driver_manager::Resource> resource,
      std::vector<std::string> parent_names, uint32_t node_index, uint32_t primary_index = 0) {
    fuchsia_driver_framework::CompositeParent matched_parent({
        .composite = fuchsia_driver_framework::CompositeInfo{{
            .spec = fuchsia_driver_framework::CompositeNodeSpec{{
                .name = spec.name(),
                .parents2 = std::vector<fuchsia_driver_framework::ParentSpec2>(parent_names.size()),
            }},
            .matched_driver = fuchsia_driver_framework::CompositeDriverMatch{{
                .composite_driver = fuchsia_driver_framework::CompositeDriverInfo{{
                    .composite_name = "test-composite",
                    .driver_info = fuchsia_driver_framework::DriverInfo{{
                        .url = "fuchsia-boot:///#meta/composite-driver.cm",
                        .colocate = true,
                    }},
                }},
                .parent_names = parent_names,
                .primary_parent_index = primary_index,
            }},
        }},
        .index = node_index,
    });

    return spec.BindParent(fidl::ToWire(*arena_, matched_parent), resource);
  }

  void VerifyCompositeNode(std::weak_ptr<driver_manager::Node> composite_node,
                           std::vector<std::string> expected_parents, uint32_t primary_index) {
    auto composite_node_ptr = composite_node.lock();
    ASSERT_TRUE(composite_node_ptr);
    ASSERT_EQ(expected_parents.size(), composite_node_ptr->parents().size());
    for (size_t i = 0; i < expected_parents.size(); i++) {
      ASSERT_EQ(expected_parents[i], composite_node_ptr->parents()[i].lock()->name());
    }
    ASSERT_EQ(expected_parents[primary_index], composite_node_ptr->GetPrimaryParent()->name());
  }

  std::unique_ptr<CompositeNodeSpecTestNodeManager> node_manager_;

 private:
  std::unique_ptr<fidl::Arena<512>> arena_;
};

TEST_F(CompositeNodeSpecTest, SpecBind) {
  auto spec = CreateCompositeNodeSpec("spec", 2);

  // Bind the first node.
  std::shared_ptr parent_1 = CreateNode("spec_parent_1");
  auto result = MatchAndBindParentSpec(spec, parent_1, {"node-0", "node-1"}, 0);
  ASSERT_TRUE(result.is_ok());
  ASSERT_FALSE(result.value());

  // Bind the second node.
  std::shared_ptr parent_2 = CreateNode("spec_parent_2");
  result = MatchAndBindParentSpec(spec, parent_2, {"node-0", "node-1"}, 1);
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(result.value());

  // Verify the parents and primary node.
  auto composite_node = result.value().value();
  VerifyCompositeNode(composite_node, {"spec_parent_1", "spec_parent_2"}, 0);
}

TEST_F(CompositeNodeSpecTest, SpecBindWithResources) {
  auto spec = CreateCompositeNodeSpec("spec", 2);

  // Create a parent node.
  std::shared_ptr parent = CreateNode("parent");

  // Create a resource from the parent.
  auto provided_resource = CreateResource(parent, "resource");

  // Bind the first parent node.
  auto result = MatchAndBindParentSpec(spec, parent, {"node-0", "node-1"}, 0);
  ASSERT_TRUE(result.is_ok());
  ASSERT_FALSE(result.value());

  // Bind the provided resource (index 1).
  result = MatchAndBindParentSpec(spec, provided_resource, {"node-0", "node-1"}, 1);
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(result.value());

  // Verify the composite node.
  auto composite_node = result.value().value();
  VerifyCompositeNode(composite_node, {"parent"}, 0);
}

TEST_F(CompositeNodeSpecTest, RemoveWithCompositeNode) {
  auto spec = CreateCompositeNodeSpec("spec", 2);

  // Bind the first node.
  std::shared_ptr parent_1 = CreateNode("spec_parent_1");
  auto result = MatchAndBindParentSpec(spec, parent_1, {"node-0", "node-1"}, 0);
  ASSERT_TRUE(result.is_ok());
  ASSERT_FALSE(result.value());

  // Bind the second node.
  std::shared_ptr parent_2 = CreateNode("spec_parent_2");
  result = MatchAndBindParentSpec(spec, parent_2, {"node-0", "node-1"}, 1);
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(result.value());

  // Verify the parents and primary node.
  auto composite_node = spec.completed_composite_node();
  ASSERT_TRUE(composite_node.has_value());
  auto composite_node_ptr = composite_node->lock();
  VerifyCompositeNode(composite_node.value(), {"spec_parent_1", "spec_parent_2"}, 0);

  // Invoke remove.
  spec.Remove([](zx::result<> result) {});
  ASSERT_EQ(driver_manager::ShutdownIntent::kRebindComposite,
            composite_node_ptr->shutdown_intent());
  ASSERT_FALSE(spec.completed_composite_node());
}

TEST_F(CompositeNodeSpecTest, RemoveWithNoCompositeNode) {
  auto spec = CreateCompositeNodeSpec("spec", 2);

  // Bind the second node.
  std::shared_ptr parent_2 = CreateNode("spec_parent_2");
  auto result = MatchAndBindParentSpec(spec, parent_2, {"node-0", "node-1"}, 1);
  ASSERT_TRUE(result.is_ok());
  ASSERT_FALSE(result.value());

  ASSERT_FALSE(spec.completed_composite_node());

  // Invoke remove.
  spec.Remove([](zx::result<> result) {});
  ASSERT_FALSE(spec.completed_composite_node());
}

TEST_F(CompositeNodeSpecTest, SpecBindWithResourcesAndDictionary) {
  auto spec = CreateCompositeNodeSpec("spec", 2);

  // Create a parent node.
  std::shared_ptr parent = CreateNode("parent");
  parent->set_collection(driver_manager::Collection::kBoot);
  parent->set_dictionary_ref(1234);

  // Create a resource from the parent.
  driver_manager::NodeOffer node_offer_1{
      .source_name = "parent",
      .source_collection = driver_manager::Collection::kBoot,
      .transport = driver_manager::OfferTransport::Dictionary,
      .service_name = "service_1",
      .source_instance_filter = {"default"},
      .renamed_instances = {fuchsia_component_decl::NameMapping{{
          .source_name = "default",
          .target_name = "default",
      }}},
  };

  auto provided_resource = std::make_shared<driver_manager::Resource>(
      node_manager_->GetNextResourceId(), parent, "resource",
      std::vector<fuchsia_driver_framework::NodeProperty2>{},
      std::vector<driver_manager::NodeOffer>{node_offer_1}, std::nullopt, dispatcher());
  parent->add_provided_resource_for_testing(provided_resource);

  // Bind the first parent node (index 0).
  auto result = MatchAndBindParentSpec(spec, parent, {"node-0", "node-1"}, 0, 1);
  ASSERT_TRUE(result.is_ok());
  ASSERT_FALSE(result.value());

  // Bind the provided resource (index 1).
  result = MatchAndBindParentSpec(spec, provided_resource, {"node-0", "node-1"}, 1, 1);
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(result.value());

  // Verify the composite node.
  auto composite_node = result.value().value();
  auto composite_node_ptr = composite_node.lock();
  ASSERT_TRUE(composite_node_ptr);
  VerifyCompositeNode(composite_node, {"parent"}, 0);

  bool callback_called = false;
  composite_node_ptr->PrepareDictionary([&](zx::result<> result) {
    ASSERT_TRUE(result.is_ok());
    callback_called = true;
  });

  RunLoopUntilIdle();
  ASSERT_TRUE(callback_called);

  // Verify that CreateDictionaryWith was called with correct receivers
  auto& fake_util = node_manager_->dictionary_util_;

  ASSERT_EQ(fake_util.receivers_.size(), 1u);
  ASSERT_TRUE(fake_util.receivers_.count("service_1"));
}

TEST_F(CompositeNodeSpecTest, SpecBindParentNameProperty) {
  std::vector<fuchsia_driver_framework::ParentSpec2> parents;
  parents.push_back(fuchsia_driver_framework::ParentSpec2{{
      .bind_rules = {},
      .properties =
          {
              fuchsia_driver_framework::NodeProperty2{{
                  .key = "fuchsia.NAME",
                  .value = fuchsia_driver_framework::NodePropertyValue::WithStringValue("node-0"),
              }},
          },
  }});
  parents.push_back(fuchsia_driver_framework::ParentSpec2{{
      .bind_rules = {},
      .properties =
          {
              fuchsia_driver_framework::NodeProperty2{{
                  .key = "fuchsia.NAME",
                  .value =
                      fuchsia_driver_framework::NodePropertyValue::WithStringValue("wrong-node-1"),
              }},
          },
  }});
  driver_manager::CompositeNodeSpec spec(
      driver_manager::CompositeNodeSpecCreateInfo{
          .name = "spec",
          .parents = std::move(parents),
      },
      dispatcher(), node_manager_.get());

  std::shared_ptr parent_1 = CreateNode("spec_parent_1");
  auto result = MatchAndBindParentSpec(spec, parent_1, {"node-0", "node-1"}, 0);
  ASSERT_TRUE(result.is_ok());

  std::shared_ptr parent_2 = CreateNode("spec_parent_2");
  result = MatchAndBindParentSpec(spec, parent_2, {"node-0", "node-1"}, 1);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, result.status_value());
}
