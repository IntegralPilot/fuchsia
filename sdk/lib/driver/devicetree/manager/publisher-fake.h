// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_FAKE_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_FAKE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/testing/cpp/driver_runtime.h>

#include "lib/driver/devicetree/manager/test-publisher.h"

namespace fdf_devicetree::testing {

class FakePlatformBus final : public fdf::Server<fuchsia_hardware_platform_bus::PlatformBus> {
 public:
  void NodeAdd(NodeAddRequest& request, NodeAddCompleter::Sync& completer) override;
  void AddCompositeNodeSpec(AddCompositeNodeSpecRequest& request,
                            AddCompositeNodeSpecCompleter::Sync& completer) override;
  void GetBoardInfo(GetBoardInfoCompleter::Sync& completer) override;
  void SetBoardInfo(SetBoardInfoRequest& request, SetBoardInfoCompleter::Sync& completer) override;
  void SetBootloaderInfo(SetBootloaderInfoRequest& request,
                         SetBootloaderInfoCompleter::Sync& completer) override;
  void RegisterSysSuspendCallback(RegisterSysSuspendCallbackRequest& request,
                                  RegisterSysSuspendCallbackCompleter::Sync& completer) override;
  void RegisterIommu(RegisterIommuRequest& request,
                     RegisterIommuCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_platform_bus::PlatformBus> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  std::vector<fuchsia_hardware_platform_bus::Node>& nodes() { return nodes_; }
  std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu>& iommus() { return iommus_; }

 private:
  std::vector<fuchsia_hardware_platform_bus::Node> nodes_;
  std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu> iommus_;
};

class FakeCompositeNodeManager final
    : public fidl::Server<fuchsia_driver_framework::CompositeNodeManager> {
 public:
  void AddSpec(AddSpecRequest& request, AddSpecCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_framework::CompositeNodeManager> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  std::vector<fuchsia_driver_framework::CompositeNodeSpec>& composite_node_specs() {
    return requests_;
  }

 private:
  std::vector<fuchsia_driver_framework::CompositeNodeSpec> requests_;
};

class FakeNode final : public fidl::Server<fuchsia_driver_framework::Node> {
 public:
  void AddChild(AddChildRequest& request, AddChildCompleter::Sync& completer) override;
  void ProvideResource(ProvideResourceRequest& request,
                       ProvideResourceCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_framework::Node> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  std::vector<std::shared_ptr<AddChildRequest>>& requests() { return requests_; }

 private:
  std::vector<std::shared_ptr<AddChildRequest>> requests_;
};

class FakeEnvWrapper {
 public:
  void Bind(fdf::ServerEnd<fuchsia_hardware_platform_bus::PlatformBus> pbus_endpoints_server,
            fidl::ServerEnd<fuchsia_driver_framework::CompositeNodeManager> mgr_endpoints_server,
            fidl::ServerEnd<fuchsia_driver_framework::Node> node_endpoint_server);

  std::vector<BoardChildNode> GetBoardChildNodes();
  std::vector<fuchsia_hardware_platform_bus::Node> GetPbusNodes();
  std::vector<fuchsia_driver_framework::CompositeNodeSpec> GetCompositeNodeSpecs();
  std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu> GetIommus();

 private:
  FakePlatformBus pbus_;
  FakeCompositeNodeManager mgr_;
  FakeNode node_;
};

class PublisherFake : public TestPublisher {
 public:
  PublisherFake();
  ~PublisherFake();

  // PublisherInterface
  zx::result<> AddPbusNode(fuchsia_hardware_platform_bus::Node& pbus_node,
                           std::vector<std::optional<std::string>> metadata_text = {},
                           std::vector<std::optional<std::string>> power_config_text = {}) override;
  zx::result<> AddBoardChildNode(BoardChildNode args) override;
  zx::result<> AddCompositeNodeSpec(std::string name,
                                    std::vector<fuchsia_driver_framework::ParentSpec2> parents,
                                    std::optional<std::string> driver_host) override;
  zx::result<> RegisterIommu(uint32_t iommu_id,
                             fuchsia_hardware_platform_bus::Iommu iommu) override;

  // TestPublisher
  const std::vector<BoardChildNode>& GetBoardChildNodes() override;
  const std::vector<fuchsia_hardware_platform_bus::Node>& GetPbusNodes() override;
  const std::vector<fuchsia_driver_framework::CompositeNodeSpec>& GetCompositeNodeSpecs() override;
  const std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu>& GetIommus() override;

 private:
  fdf_testing::DriverRuntime runtime_;
  fdf::UnownedSynchronizedDispatcher env_dispatcher_ = runtime_.StartBackgroundDispatcher();
  async_patterns::TestDispatcherBound<FakeEnvWrapper> env_{env_dispatcher_->async_dispatcher(),
                                                           std::in_place};

  // PublisherDev holds client endpoints.
  std::unique_ptr<PublisherInterface> publisher_dev_;

  // Storage for binding.
  fidl::SyncClient<fuchsia_driver_framework::Node> node_client_;
  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus> pbus_client_;
  fidl::SyncClient<fuchsia_driver_framework::CompositeNodeManager> mgr_client_;

  std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu> iommus_;
  std::vector<BoardChildNode> board_child_nodes_;
  std::vector<fuchsia_hardware_platform_bus::Node> pbus_nodes_;
  std::vector<fuchsia_driver_framework::CompositeNodeSpec> composite_node_specs_;
};

}  // namespace fdf_devicetree::testing

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_PUBLISHER_FAKE_H_
