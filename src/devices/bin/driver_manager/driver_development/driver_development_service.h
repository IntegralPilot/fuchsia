// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_DEVELOPMENT_DRIVER_DEVELOPMENT_SERVICE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_DEVELOPMENT_DRIVER_DEVELOPMENT_SERVICE_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "src/devices/bin/driver_manager/driver_runner.h"

namespace driver_manager {

class DriverDevelopmentService : public fidl::WireServer<fuchsia_driver_development::Manager> {
 public:
  explicit DriverDevelopmentService(driver_manager::DriverRunner& driver_runner,
                                    async_dispatcher_t* dispatcher);

  void Publish(component::OutgoingDirectory& outgoing);

 private:
  // fidl::WireServer<fuchsia_driver_development::DriverDevelopmentService>
  void RestartDriverHosts(RestartDriverHostsRequestView request,
                          RestartDriverHostsCompleter::Sync& completer) override;
  void GetDriverInfo(GetDriverInfoRequestView request,
                     GetDriverInfoCompleter::Sync& completer) override;
  void GetCompositeNodeSpecs(GetCompositeNodeSpecsRequestView request,
                             GetCompositeNodeSpecsCompleter::Sync& completer) override;
  void DisableDriver(DisableDriverRequestView request,
                     DisableDriverCompleter::Sync& completer) override;
  void EnableDriver(EnableDriverRequestView request,
                    EnableDriverCompleter::Sync& completer) override;
  void GetNodeInfo(GetNodeInfoRequestView request, GetNodeInfoCompleter::Sync& completer) override;
  void GetCompositeInfo(GetCompositeInfoRequestView request,
                        GetCompositeInfoCompleter::Sync& completer) override;
  void GetDriverHostInfo(GetDriverHostInfoRequestView request,
                         GetDriverHostInfoCompleter::Sync& completer) override;
  void BindAllUnboundNodes(BindAllUnboundNodesCompleter::Sync& completer) override;
  void BindAllUnboundNodes2(BindAllUnboundNodes2Completer::Sync& completer) override;
  void AddTestNode(AddTestNodeRequestView request, AddTestNodeCompleter::Sync& completer) override;
  void RemoveTestNode(RemoveTestNodeRequestView request,
                      RemoveTestNodeCompleter::Sync& completer) override;
  void WaitForBootup(WaitForBootupCompleter::Sync& completer) override;
  void RestartWithDictionary(RestartWithDictionaryRequestView request,
                             RestartWithDictionaryCompleter::Sync& completer) override;

  void RestartWithDictionaryAndPowerDependencies(
      RestartWithDictionaryAndPowerDependenciesRequestView request,
      RestartWithDictionaryAndPowerDependenciesCompleter::Sync& completer) override;

  void RebindCompositesWithDriver(RebindCompositesWithDriverRequestView request,
                                  RebindCompositesWithDriverCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_development::Manager> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  driver_manager::DriverRunner& driver_runner_;
  // A map of the test nodes that have been created.
  std::map<std::string, NodeWkPtr> test_nodes_;
  fidl::ServerBindingGroup<fuchsia_driver_development::Manager> bindings_;

  async_dispatcher_t* const dispatcher_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_DEVELOPMENT_DRIVER_DEVELOPMENT_SERVICE_H_
