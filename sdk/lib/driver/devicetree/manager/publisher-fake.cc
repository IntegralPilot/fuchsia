// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/manager/publisher-fake.h"

#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/manager/publisher-dev.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <zircon/status.h>

namespace fdf_devicetree::testing {

std::unique_ptr<TestPublisher> CreateTestPublisher() { return std::make_unique<PublisherFake>(); }

// FakePlatformBus
// FakePlatformBus Implementation
void FakePlatformBus::NodeAdd(NodeAddRequest& request, NodeAddCompleter::Sync& completer) {
  nodes_.emplace_back(std::move(request.node()));
  completer.Reply(zx::ok());
}

void FakePlatformBus::AddCompositeNodeSpec(AddCompositeNodeSpecRequest& request,
                                           AddCompositeNodeSpecCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void FakePlatformBus::GetBoardInfo(GetBoardInfoCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}
void FakePlatformBus::SetBoardInfo(SetBoardInfoRequest& request,
                                   SetBoardInfoCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void FakePlatformBus::SetBootloaderInfo(SetBootloaderInfoRequest& request,
                                        SetBootloaderInfoCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void FakePlatformBus::RegisterSysSuspendCallback(
    RegisterSysSuspendCallbackRequest& request,
    RegisterSysSuspendCallbackCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void FakePlatformBus::RegisterIommu(RegisterIommuRequest& request,
                                    RegisterIommuCompleter::Sync& completer) {
  iommus_.insert_or_assign(request.iommu_id(), request.iommu());
  completer.Reply(zx::ok());
}

void FakePlatformBus::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_platform_bus::PlatformBus> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

// FakeCompositeNodeManager Implementation
void FakeCompositeNodeManager::AddSpec(AddSpecRequest& request, AddSpecCompleter::Sync& completer) {
  requests_.emplace_back(std::move(request));
  completer.Reply(zx::ok());
}

void FakeCompositeNodeManager::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::CompositeNodeManager> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

// FakeNode Implementation
void FakeNode::AddChild(AddChildRequest& request, AddChildCompleter::Sync& completer) {
  requests_.push_back(std::make_shared<AddChildRequest>(std::move(request)));
  completer.Reply(zx::ok());
}

void FakeNode::ProvideResource(ProvideResourceRequest& request,
                               ProvideResourceCompleter::Sync& completer) {
  completer.Reply(zx::ok());
}

void FakeNode::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_driver_framework::Node> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

// FakeEnvWrapper Implementation
void FakeEnvWrapper::Bind(
    fdf::ServerEnd<fuchsia_hardware_platform_bus::PlatformBus> pbus_server_end,
    fidl::ServerEnd<fuchsia_driver_framework::CompositeNodeManager> mgr_server_end,
    fidl::ServerEnd<fuchsia_driver_framework::Node> node_server_end) {
  fdf::BindServer(fdf::Dispatcher::GetCurrent()->get(), std::move(pbus_server_end), &pbus_);
  fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(mgr_server_end),
                   &mgr_);
  fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(node_server_end),
                   &node_);
}

std::vector<BoardChildNode> FakeEnvWrapper::GetBoardChildNodes() {
  std::vector<BoardChildNode> nodes;
  for (const auto& req : node_.requests()) {
    BoardChildNode args;
    args.name = req->args().name().value_or("");
    if (req->args().properties2()) {
      args.properties = *req->args().properties2();
    }
    if (req->args().driver_host()) {
      args.driver_host = *req->args().driver_host();
    }
    nodes.push_back(std::move(args));
  }
  return nodes;
}

std::vector<fuchsia_hardware_platform_bus::Node> FakeEnvWrapper::GetPbusNodes() {
  return pbus_.nodes();
}

std::vector<fuchsia_driver_framework::CompositeNodeSpec> FakeEnvWrapper::GetCompositeNodeSpecs() {
  return mgr_.composite_node_specs();
}

std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu> FakeEnvWrapper::GetIommus() {
  return pbus_.iommus();
}

// PublisherFake
PublisherFake::PublisherFake() {
  static fdf_testing::ScopedGlobalLogger logger;  // Ensures logger is active.
  auto pbus_endpoints = fdf::Endpoints<fuchsia_hardware_platform_bus::PlatformBus>::Create();
  auto mgr_endpoints = fidl::Endpoints<fuchsia_driver_framework::CompositeNodeManager>::Create();
  auto node_endpoints = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  node_client_.Bind(std::move(node_endpoints.client));
  env_.SyncCall(&FakeEnvWrapper::Bind, std::move(pbus_endpoints.server),
                std::move(mgr_endpoints.server), std::move(node_endpoints.server));
  pbus_client_.Bind(std::move(pbus_endpoints.client));

  mgr_client_.Bind(std::move(mgr_endpoints.client));

  publisher_dev_ = std::make_unique<PublisherDev>(pbus_client_, mgr_client_, node_client_);
}

PublisherFake::~PublisherFake() {}

zx::result<> PublisherFake::AddPbusNode(fuchsia_hardware_platform_bus::Node& pbus_node,
                                        std::vector<std::optional<std::string>> metadata_text,
                                        std::vector<std::optional<std::string>> power_config_text) {
  return publisher_dev_->AddPbusNode(pbus_node, std::move(metadata_text),
                                     std::move(power_config_text));
}

zx::result<> PublisherFake::AddBoardChildNode(BoardChildNode args) {
  return publisher_dev_->AddBoardChildNode(std::move(args));
}

zx::result<> PublisherFake::AddCompositeNodeSpec(
    std::string name, std::vector<fuchsia_driver_framework::ParentSpec2> parents,
    std::optional<std::string> driver_host) {
  return publisher_dev_->AddCompositeNodeSpec(std::move(name), std::move(parents),
                                              std::move(driver_host));
}

zx::result<> PublisherFake::RegisterIommu(uint32_t iommu_id,
                                          fuchsia_hardware_platform_bus::Iommu iommu) {
  return publisher_dev_->RegisterIommu(iommu_id, std::move(iommu));
}

const std::vector<BoardChildNode>& PublisherFake::GetBoardChildNodes() {
  board_child_nodes_ = env_.SyncCall(&FakeEnvWrapper::GetBoardChildNodes);
  return board_child_nodes_;
}

const std::vector<fuchsia_hardware_platform_bus::Node>& PublisherFake::GetPbusNodes() {
  pbus_nodes_ = env_.SyncCall(&FakeEnvWrapper::GetPbusNodes);
  return pbus_nodes_;
}

const std::vector<fuchsia_driver_framework::CompositeNodeSpec>&
PublisherFake::GetCompositeNodeSpecs() {
  composite_node_specs_ = env_.SyncCall(&FakeEnvWrapper::GetCompositeNodeSpecs);
  return composite_node_specs_;
}

const std::unordered_map<uint32_t, fuchsia_hardware_platform_bus::Iommu>&
PublisherFake::GetIommus() {
  iommus_ = env_.SyncCall(&FakeEnvWrapper::GetIommus);
  return iommus_;
}

}  // namespace fdf_devicetree::testing
