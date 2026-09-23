// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/tests/driver_manager_test_base.h"

#include "src/devices/bin/driver_manager/resource.h"

void DriverManagerTestBase::SetUp() {
  TestLoopFixture::SetUp();
  devfs_ = std::make_unique<driver_manager::Devfs>(root_devnode_, dispatcher());
  root_ = CreateNode("root");
  root_->AddToDevfsForTesting(root_devnode_.value());
}

void DriverManagerTestBase::TearDown() {
  root_.reset();
  devfs_.reset();
  TestLoopFixture::TearDown();
}

std::shared_ptr<driver_manager::Node> DriverManagerTestBase::CreateNode(
    std::string_view name, std::map<std::string, zx::event> node_token_overrides) {
  auto node = std::make_shared<driver_manager::Node>(name, std::weak_ptr<driver_manager::Node>{},
                                                     GetNodeManager(), dispatcher(),
                                                     std::move(node_token_overrides));
  node->InitializeSelfResource();
  node->AddToDevfsForTesting(root_devnode_.value());
  node->devfs_device().publish();
  return node;
}

std::shared_ptr<driver_manager::Node> DriverManagerTestBase::CreateNode(
    std::string_view name, std::weak_ptr<driver_manager::Node> parent,
    std::map<std::string, zx::event> node_token_overrides) {
  auto node = std::make_shared<driver_manager::Node>(name, std::move(parent), GetNodeManager(),
                                                     dispatcher(), std::move(node_token_overrides));
  node->InitializeSelfResource();
  node->AddToDevfsForTesting(root_devnode_.value());
  node->devfs_device().publish();
  node->AddToParents();
  return node;
}

std::shared_ptr<driver_manager::Node> DriverManagerTestBase::CreateCompositeNode(
    std::string_view name, std::vector<std::weak_ptr<driver_manager::Node>> parents,
    const std::vector<fuchsia_driver_framework::NodePropertyEntry2>& parent_properties,
    uint32_t primary_index, std::map<std::string, zx::event> node_token_overrides) {
  std::vector<std::string> parent_names;
  parent_names.reserve(parents.size());
  std::vector<std::weak_ptr<driver_manager::Resource>> parent_resources;
  parent_resources.reserve(parents.size());
  for (auto& parent_wk : parents) {
    auto parent = parent_wk.lock();
    ZX_ASSERT(parent);
    parent_names.push_back(parent->name());
    auto self_resource = parent->GetSelfResource();
    ZX_ASSERT(self_resource.has_value());
    parent_resources.push_back(self_resource.value());
  }
  return driver_manager::Node::CreateCompositeNode(
             name, std::move(parent_resources), std::move(parent_names), parent_properties,
             GetNodeManager(), dispatcher(), "", primary_index, std::move(node_token_overrides))
      .value();
}

std::shared_ptr<driver_manager::Resource> DriverManagerTestBase::CreateResource(
    std::weak_ptr<driver_manager::Node> owner, std::string_view name) {
  return std::make_shared<driver_manager::Resource>(
      GetNodeManager()->GetNextResourceId(), std::move(owner), std::string(name),
      std::vector<fuchsia_driver_framework::NodeProperty2>{},
      std::vector<driver_manager::NodeOffer>{}, std::nullopt, dispatcher());
}
