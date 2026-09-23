// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_POWER_TESTING_SYSTEM_INTEGRATION_UTIL_TEST_UTIL_H_
#define SRC_POWER_TESTING_SYSTEM_INTEGRATION_UTIL_TEST_UTIL_H_

#include <fidl/fuchsia.component.sandbox/cpp/fidl.h>
#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/fidl.h>
#include <fidl/test.sagcontrol/cpp/natural_ostream.h>
#include <fidl/test.suspendcontrol/cpp/fidl.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>

namespace system_integration_utils {

/// Basic implementation of fuchsia_power_broker::ElementRunner for integration tests.
/// Serves level change requests (SetLevel) for test-created power elements like the
/// "fake-all-drivers" aggregator power element.
class BasicElementRunner;

class Connector final : public fidl::Server<fuchsia_component_sandbox::Receiver> {
 public:
  explicit Connector(async_dispatcher_t* dispatcher, std::string path,
                     fidl::ServerEnd<fuchsia_component_sandbox::Receiver> server)
      : dispatcher_(dispatcher),
        path_(std::move(path)),
        binding_(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure) {}

  void Receive(ReceiveRequest& request, ReceiveCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_sandbox::Receiver> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  std::string path_;
  fidl::ServerBinding<fuchsia_component_sandbox::Receiver> binding_;
};

struct CustomDictionaryEntry {
  std::string name;
  fidl::ClientEnd<fuchsia_component_sandbox::Receiver> client_end;
};

class TestLoopBase : public loop_fixture::RealLoop {
 public:
  TestLoopBase();
  virtual ~TestLoopBase();

  // Get the duplicated DependencyToken captured by the intercepted Topology.AddElement call.
  zx::event GetCapturedToken(const std::string& element_name);

 protected:
  void Initialize();

  test_sagcontrol::SystemActivityGovernorState GetBootCompleteState();

  bool SetBootComplete();

  zx_status_t AwaitSystemSuspend();
  zx_status_t StartSystemResume();

  // Change the SAG state and wait for the transition to complete.
  zx_status_t ChangeSagState(test_sagcontrol::SystemActivityGovernorState state,
                             zx::duration poll_delay = zx::sec(1));

  // Wait for an inspect selector to match a specific value.
  void MatchInspectData(diagnostics::reader::ArchiveReader& reader, const std::string& moniker,
                        const std::optional<std::string>& inspect_tree_name,
                        const std::vector<std::string>& inspect_path,
                        std::variant<bool, uint64_t> value);

  zx::result<std::string> GetPowerElementId(diagnostics::reader::ArchiveReader& reader,
                                            const std::string& pb_moniker,
                                            const std::string& power_element_name);

  // Prepare the target driver for power system testing. This is done by creating a dictionary
  // with the power protocols from the test-specific instances of the SAG and power broker,
  // and restarting the target driver and its children with access to this dictionary.
  //
  // |expect_new_koid| whether to expect the restarted node to have a new driver host koid, this
  // should be true if the target driver is not colocated with its parent driver, otherwise it
  // should be false.
  //
  // Returns a zx::eventpair that should be held onto for the duration of the test. When released
  // the target driver and children are restarted again and lose access to the test-specific
  // power protocols.
  zx::eventpair PrepareDriver(std::string_view node_filter, std::string_view driver_url_suffix,
                              bool expect_new_koid, bool use_df_elements = false,
                              std::vector<CustomDictionaryEntry> custom_entries = {},
                              std::vector<std::string> target_child_nodes = {});

  /// Prepare the target driver and target child nodes with power token overrides for non-intrusive
  /// driver power testing.
  ///
  /// Workflow performed by this helper:
  /// 1. Discovers the target driver matching |node_filter| and |driver_url_suffix|.
  /// 2. Creates power token overrides for each child node listed in |target_child_nodes|.
  /// 3. Creates an aggregator power element ("fake-all-drivers") in the test realm's Power Broker
  ///    topology with level dependencies on each child node token override.
  /// 4. Registers the aggregator element ("fake-all-drivers") dependency token with the
  ///    System Activity Governor (SAG) via CpuElementManager.AddExecutionStateDependency.
  /// 5. Restarts the target driver subtree with the test dictionary, CPU token override, and child
  ///    node token overrides via Driver Manager.
  /// 6. Awaits the target driver and child nodes to restart and successfully bind.
  ///
  /// @param node_filter Filter string or moniker substring matching the target parent node.
  /// @param driver_url_suffix Expected URL suffix of the target bound driver.
  /// @param expect_new_koid Whether to expect a new driver host KOID after restart (true if
  /// non-colocated).
  /// @param target_child_nodes Names of child nodes to apply power token overrides to.
  /// @param custom_entries Additional capability store entries to insert into the test dictionary.
  /// @return A zx::eventpair (release fence) that must be retained for the duration of the test.
  ///         When dropped, Driver Manager restarts the driver subtree and restores original
  ///         bindings.
  zx::eventpair PrepareDriverWithPowerTokenOverrides(
      std::string_view node_filter, std::string_view driver_url_suffix, bool expect_new_koid,
      const std::vector<std::string>& target_child_nodes,
      std::vector<CustomDictionaryEntry> custom_entries = {});

  // Create and export a component framework dictionary that contains connectors for the various
  // power framework protocols, that are connected to the test-specific SAG and power broker that
  // is accessible in the incoming namespace of the test component. See 'meta/client.shard.cml'.
  fuchsia_component_sandbox::DictionaryRef CreateDictionaryForTest(
      std::vector<CustomDictionaryEntry> custom_entries = {});

  // Query the driver framework for nodes that have a moniker matching the |node_filter|.
  std::vector<fuchsia_driver_development::NodeInfo> GetNodeInfo(std::string_view node_filter);

 private:
  async::Loop sandbox_connector_loop_{&kAsyncLoopConfigNeverAttachToThread};
  uint32_t next_cap_id_ = 1;
  fidl::ClientEnd<test_sagcontrol::State> sag_control_state_client_end_;
  fidl::ClientEnd<test_suspendcontrol::Device> suspend_device_client_end_;
  fidl::ClientEnd<fuchsia_driver_development::Manager> driver_manager_client_end_;
  fidl::ClientEnd<fuchsia_power_system::CpuElementManager> cpu_element_manager_client_end_;

  fidl::ClientEnd<fuchsia_power_broker::ElementControl> test_all_drivers_control_;
  std::unique_ptr<BasicElementRunner> test_all_drivers_runner_;

  async_patterns::DispatcherBound<Connector> sag_connector_{sandbox_connector_loop_.dispatcher()};
  async_patterns::DispatcherBound<Connector> broker_connector_{
      sandbox_connector_loop_.dispatcher()};
  async_patterns::DispatcherBound<Connector> cpu_element_connector_{
      sandbox_connector_loop_.dispatcher()};
};

}  // namespace system_integration_utils

#endif  // SRC_POWER_TESTING_SYSTEM_INTEGRATION_UTIL_TEST_UTIL_H_
