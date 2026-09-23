// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_DRIVER_MANAGER_TEST_BASE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_DRIVER_MANAGER_TEST_BASE_H_

#include "src/devices/bin/driver_manager/node.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace driver_manager {
class Resource;
}  // namespace driver_manager

class TestNodeManagerBase : public driver_manager::NodeManager {
 public:
  void Bind(driver_manager::Node& node,
            std::shared_ptr<driver_manager::BindResultTracker> result_tracker) override {}
  void Bind(driver_manager::Resource& resource,
            std::shared_ptr<driver_manager::BindResultTracker> result_tracker) override {}

  driver_manager::DriverHost* GetDriverHost(
      std::string_view driver_host_name_for_colocation) override {
    return nullptr;
  }
  zx::result<driver_manager::DriverHost*> CreateDriverHost(
      bool use_next_vdso, std::string_view driver_host_name_for_colocation) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  void CreatePowerElement(
      std::optional<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_client,
      std::string_view name, fuchsia_power_broker::DependencyToken element_token,
      std::vector<fuchsia_power_broker::DependencyToken> deps,
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> control,
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> runner,
      fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor,
      driver_manager::Collection for_collection,
      std::optional<fuchsia_power_broker::DependencyToken> cpu_token_override,
      std::optional<zx::eventpair> initial_lease_token, bool is_hermetic_power_test,
      fit::callback<void(zx::result<bool>)> cb) override {
    cb(zx::ok(false));
  }

  bool SuspendEnabled() override { return false; }

  driver_manager::MemoryAttributor& memory_attributor() override { return attributor_; }

  driver_manager::ResourceId GetNextResourceId() override { return next_resource_id_++; }

  void RebootSystem() override { reboot_system_called_ = true; }

  bool reboot_system_called_ = false;

 private:
  driver_manager::MemoryAttributor attributor_{async_get_default_dispatcher()};
  driver_manager::ResourceId next_resource_id_ = 0;
};

class DriverManagerTestBase : public gtest::TestLoopFixture {
 public:
  void SetUp() override;
  void TearDown() override;

  virtual driver_manager::NodeManager* GetNodeManager() = 0;

 protected:
  std::shared_ptr<driver_manager::Node> CreateNode(
      std::string_view name, std::map<std::string, zx::event> node_token_overrides = {});

  // Creates a DFv2 node and add it to the given parent.
  std::shared_ptr<driver_manager::Node> CreateNode(
      std::string_view name, std::weak_ptr<driver_manager::Node> parent,
      std::map<std::string, zx::event> node_token_overrides = {});

  std::shared_ptr<driver_manager::Resource> CreateResource(
      std::weak_ptr<driver_manager::Node> owner, std::string_view name);

  std::shared_ptr<driver_manager::Node> CreateCompositeNode(
      std::string_view name, std::vector<std::weak_ptr<driver_manager::Node>> parents,
      const std::vector<fuchsia_driver_framework::NodePropertyEntry2>& parent_properties,
      uint32_t primary_index = 0, std::map<std::string, zx::event> node_token_overrides = {});

  std::shared_ptr<driver_manager::Node> root() const { return root_; }

  driver_manager::Devfs* devfs() const { return devfs_.get(); }

  driver_manager::Devnode& root_devnode() { return root_devnode_.value(); }

 private:
  std::unique_ptr<driver_manager::Devfs> devfs_;
  std::shared_ptr<driver_manager::Node> root_;
  std::optional<driver_manager::Devnode> root_devnode_;
};

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_DRIVER_MANAGER_TEST_BASE_H_
