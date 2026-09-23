// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test_util.h"

#include <fidl/fuchsia.hardware.power.suspend/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/test.suspendcontrol/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>

#include <mutex>
#include <unordered_map>

#include <gtest/gtest.h>

namespace system_integration_utils {
namespace {

namespace fh_suspend = fuchsia_hardware_power_suspend;

template <typename T>
fidl::ostream::Formatted<T> FidlFormat(const T& value) {
  return fidl::ostream::Formatted<T>(value);
}

std::mutex g_tokens_mutex;
std::unordered_map<std::string, zx::event> g_captured_tokens;

class TestElementControlServer : public fidl::Server<fuchsia_power_broker::ElementControl> {
 public:
  TestElementControlServer(std::string element_name,
                           fidl::ClientEnd<fuchsia_power_broker::ElementControl> real_control)
      : element_name_(element_name), real_control_(std::move(real_control)) {}

  void RegisterDependencyToken(RegisterDependencyTokenRequest& request,
                               RegisterDependencyTokenCompleter::Sync& completer) override {
    FX_LOGS(DEBUG) << "[INTERCEPTOR] RegisterDependencyToken called for element: " << element_name_;
    zx::event token;
    zx_status_t status = request.token().duplicate(ZX_RIGHT_SAME_RIGHTS, &token);
    if (status == ZX_OK) {
      std::lock_guard<std::mutex> lock(g_tokens_mutex);
      g_captured_tokens[element_name_] = std::move(token);
      FX_LOGS(DEBUG) << "[INTERCEPTOR] Successfully duplicated and stashed token for element: "
                     << element_name_;
    } else {
      FX_LOGS(ERROR) << "[INTERCEPTOR] Failed to duplicate token: " << status;
    }

    auto result = fidl::Call(real_control_)->RegisterDependencyToken(std::move(request));
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        completer.Close(result.error_value().framework_error().status());
      } else {
        completer.Reply(fit::error(result.error_value().domain_error()));
      }
    } else {
      completer.Reply(fit::ok());
    }
  }

  void OpenStatusChannel(OpenStatusChannelRequest& request,
                         OpenStatusChannelCompleter::Sync& completer) override {
    auto result = fidl::Call(real_control_)->OpenStatusChannel(std::move(request));
    if (result.is_error()) {
      FX_LOGS(ERROR) << "[INTERCEPTOR] OpenStatusChannel failed: " << result.error_value();
    }
  }

  void UnregisterDependencyToken(UnregisterDependencyTokenRequest& request,
                                 UnregisterDependencyTokenCompleter::Sync& completer) override {
    auto result = fidl::Call(real_control_)->UnregisterDependencyToken(std::move(request));
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        completer.Close(result.error_value().framework_error().status());
      } else {
        completer.Reply(fit::error(result.error_value().domain_error()));
      }
    } else {
      completer.Reply(fit::ok());
    }
  }

  void AddDependency(AddDependencyRequest& request,
                     AddDependencyCompleter::Sync& completer) override {
    auto result = fidl::Call(real_control_)->AddDependency(std::move(request));
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        completer.Close(result.error_value().framework_error().status());
      } else {
        completer.Reply(fit::error(result.error_value().domain_error()));
      }
    } else {
      completer.Reply(fit::ok());
    }
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementControl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  std::string element_name_;
  fidl::ClientEnd<fuchsia_power_broker::ElementControl> real_control_;
};

class TestTopologyServer : public fidl::Server<fuchsia_power_broker::Topology> {
 public:
  TestTopologyServer(async_dispatcher_t* dispatcher,
                     fidl::ClientEnd<fuchsia_power_broker::Topology> real_topology)
      : dispatcher_(dispatcher), real_topology_(std::move(real_topology)) {}

  void AddElement(AddElementRequest& request, AddElementCompleter::Sync& completer) override {
    std::string name = request.element_name().has_value() ? *request.element_name() : "";

    if (request.element_control().has_value()) {
      auto [real_control_client_end, real_control_server_end] =
          fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();

      auto driver_runner_server_end = std::move(*request.element_control());

      auto local_server =
          std::make_unique<TestElementControlServer>(name, std::move(real_control_client_end));
      fidl::BindServer(dispatcher_, std::move(driver_runner_server_end), std::move(local_server));

      request.element_control(std::move(real_control_server_end));
    }

    auto real_topology = component::Connect<fuchsia_power_broker::Topology>();
    if (!real_topology.is_ok()) {
      completer.Close(real_topology.status_value());
      return;
    }

    auto result = fidl::Call(*real_topology)->AddElement(std::move(request));
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        completer.Close(result.error_value().framework_error().status());
      } else {
        completer.Reply(fit::error(result.error_value().domain_error()));
      }
    } else {
      completer.Reply(fit::ok());
    }
  }

  void Lease(LeaseRequest& request, LeaseCompleter::Sync& completer) override {
    FX_LOGS(DEBUG) << "[INTERCEPTOR] Topology::Lease called!";
    auto real_topology = component::Connect<fuchsia_power_broker::Topology>();
    if (!real_topology.is_ok()) {
      FX_LOGS(ERROR) << "[INTERCEPTOR] Failed to connect to real Topology: "
                     << real_topology.status_string();
      completer.Close(real_topology.status_value());
      return;
    }

    auto result = fidl::Call(*real_topology)->Lease(std::move(request));
    if (result.is_error()) {
      FX_LOGS(ERROR) << "[INTERCEPTOR] Real Topology::Lease call failed: " << result.error_value();
      if (result.error_value().is_framework_error()) {
        completer.Close(result.error_value().framework_error().status());
      } else {
        completer.Reply(fit::error(result.error_value().domain_error()));
      }
    } else {
      FX_LOGS(DEBUG) << "[INTERCEPTOR] Real Topology::Lease call succeeded!";
      completer.Reply(fit::ok());
    }
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Topology> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ClientEnd<fuchsia_power_broker::Topology> real_topology_;
};

}  // namespace

/// Minimal implementation of fuchsia_power_broker::ElementRunner for dummy or aggregator
/// power elements (such as "fake-all-drivers") created during system integration testing.
/// Responds synchronously to level modification requests.
class BasicElementRunner : public fidl::Server<fuchsia_power_broker::ElementRunner> {
 public:
  explicit BasicElementRunner(fidl::ServerEnd<fuchsia_power_broker::ElementRunner> server_end)
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {
    loop_.StartThread("basic-element-runner");
    fidl::BindServer(loop_.dispatcher(), std::move(server_end), this);
  }

  void SetLevel(SetLevelRequest& request, SetLevelCompleter::Sync& completer) override {
    completer.Reply();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  async::Loop loop_;
};

void Connector::Receive(ReceiveRequest& request, ReceiveCompleter::Sync& completer) {
  if (path_ == fidl::DiscoverableProtocolDefaultPath<fuchsia_power_broker::Topology>) {
    auto real_topology = component::Connect<fuchsia_power_broker::Topology>();
    if (real_topology.is_ok()) {
      auto local_server =
          std::make_unique<TestTopologyServer>(dispatcher_, std::move(real_topology.value()));
      fidl::BindServer(
          dispatcher_,
          fidl::ServerEnd<fuchsia_power_broker::Topology>(std::move(request.channel())),
          std::move(local_server));
      return;
    }
  }

  zx_status_t status = fdio_service_connect(path_.c_str(), request.channel().release());
  if (status != ZX_OK) {
    completer.Close(status);
  }
}

TestLoopBase::TestLoopBase() = default;
TestLoopBase::~TestLoopBase() = default;

zx::event TestLoopBase::GetCapturedToken(const std::string& element_name) {
  std::lock_guard<std::mutex> lock(g_tokens_mutex);
  auto iter = g_captured_tokens.find(element_name);
  if (iter != g_captured_tokens.end()) {
    zx::event copy;
    zx_status_t status = iter->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &copy);
    if (status == ZX_OK) {
      return copy;
    }
  }
  return zx::event();
}

void TestLoopBase::Initialize() {
  {
    auto result = component::Connect<test_sagcontrol::State>();
    ASSERT_EQ(ZX_OK, result.status_value());
    sag_control_state_client_end_ = std::move(result.value());
  }

  {
    auto result = component::Connect<test_suspendcontrol::Device>();
    ASSERT_EQ(ZX_OK, result.status_value());
    suspend_device_client_end_ = std::move(result.value());
  }

  {
    auto result = component::Connect<fuchsia_driver_development::Manager>();
    ASSERT_EQ(ZX_OK, result.status_value());
    driver_manager_client_end_ = std::move(result.value());
  }

  {
    auto result = component::Connect<fuchsia_power_system::CpuElementManager>();
    ASSERT_EQ(ZX_OK, result.status_value());
    cpu_element_manager_client_end_ = std::move(result.value());
  }

  {
    fh_suspend::SuspendState state;
    state.resume_latency(zx::usec(100).to_nsecs());

    test_suspendcontrol::DeviceSetSuspendStatesRequest request;
    std::vector<fh_suspend::SuspendState> states = {state};
    request.suspend_states(states);

    auto result = fidl::Call(suspend_device_client_end_)->SetSuspendStates(request);
    ASSERT_TRUE(result.is_ok());
  }
}

test_sagcontrol::SystemActivityGovernorState TestLoopBase::GetBootCompleteState() {
  test_sagcontrol::SystemActivityGovernorState state;
  state.execution_state_level(fuchsia_power_system::ExecutionStateLevel::kActive);
  state.application_activity_level(fuchsia_power_system::ApplicationActivityLevel::kActive);
  return state;
}

zx_status_t TestLoopBase::AwaitSystemSuspend() {
  FX_LOGS(INFO) << "Awaiting suspend";
  auto wait_result = fidl::Call(suspend_device_client_end_)->AwaitSuspend();
  if (!wait_result.is_ok()) {
    FX_LOGS(ERROR) << "Failed to await suspend: " << wait_result.error_value();
    return ZX_ERR_INTERNAL;
  }
  FX_LOGS(INFO) << "Suspend confirmed";

  return ZX_OK;
}

bool TestLoopBase::SetBootComplete() {
  {
    auto status = fidl::WireCall(sag_control_state_client_end_)->SetBootComplete();
    if (!status.ok()) {
      FX_LOGS(ERROR) << "Failed to SetBootComplete";
      return false;
    }
  }
  return true;
}

zx_status_t TestLoopBase::StartSystemResume() {
  FX_LOGS(INFO) << "Starting system resume";
  test_suspendcontrol::SuspendResult suspend_result;
  // Assign suspend results to provide the appearance of a normal return from suspension.
  suspend_result.suspend_duration(2);
  suspend_result.suspend_overhead(1);
  auto request = test_suspendcontrol::DeviceResumeRequest::WithResult(suspend_result);
  auto resume_result = fidl::Call(suspend_device_client_end_)->Resume(request);
  if (!resume_result.is_ok()) {
    FX_LOGS(ERROR) << "Failed to await suspend: " << resume_result.error_value();
    return ZX_ERR_INTERNAL;
  }
  FX_LOGS(INFO) << "Resume started";

  return ZX_OK;
}

zx_status_t TestLoopBase::ChangeSagState(test_sagcontrol::SystemActivityGovernorState state,
                                         zx::duration poll_delay) {
  while (true) {
    FX_LOGS(INFO) << "Setting SAG state: " << FidlFormat(state);
    auto set_result = fidl::Call(sag_control_state_client_end_)->Set(state);
    if (!set_result.is_ok()) {
      FX_LOGS(ERROR) << "Failed to set SAG state: " << set_result.error_value();
      return ZX_ERR_INTERNAL;
    }
    zx::nanosleep(zx::deadline_after(poll_delay));

    const auto get_result = fidl::Call(sag_control_state_client_end_)->Get();
    if (get_result.value() == state) {
      break;
    }

    FX_LOGS(INFO) << "Retrying SAG state change. Last known state: "
                  << FidlFormat(get_result.value());
    set_result = fidl::Call(sag_control_state_client_end_)->Set(GetBootCompleteState());
    if (!set_result.is_ok()) {
      FX_LOGS(ERROR) << "Failed to set boot complete SAG state: " << set_result.error_value();
      return ZX_ERR_INTERNAL;
    }
    zx::nanosleep(zx::deadline_after(poll_delay));
  }
  FX_LOGS(INFO) << "SAG state change complete.";
  return ZX_OK;
}

void TestLoopBase::MatchInspectData(diagnostics::reader::ArchiveReader& reader,
                                    const std::string& moniker,
                                    const std::optional<std::string>& inspect_tree_name,
                                    const std::vector<std::string>& inspect_path,
                                    std::variant<bool, uint64_t> value) {
  const auto selector = diagnostics::reader::MakeSelector(
      moniker,
      inspect_tree_name.has_value()
          ? std::optional<std::vector<std::string>>{{inspect_tree_name.value()}}
          : std::nullopt,
      std::vector<std::string>(inspect_path.begin(), inspect_path.end() - 1),
      inspect_path[inspect_path.size() - 1]);

  std::string path_str;
  for (const auto& path : inspect_path) {
    path_str += "[" + path + "]";
  }
  FX_LOGS(INFO) << "Matching inspect data for moniker = " << moniker << ", path = " << path_str
                << ", selector = " << selector;

  bool match = false;
  do {
    auto result = RunPromise(reader.SetSelectors({selector}).GetInspectSnapshot());
    auto data = result.take_value();
    for (const auto& datum : data) {
      bool* bool_value = std::get_if<bool>(&value);
      if (bool_value != nullptr) {
        const auto& actual_value = datum.GetByPath(inspect_path);
        // IsBool will return false if the value at inspect_path doesn't exist yet,
        // whereas GetBool will crash the test
        if (!actual_value.IsBool()) {
          FX_LOGS(INFO) << moniker << ": Expected value " << std::boolalpha << *bool_value
                        << ", but got nothing for selector " << selector
                        << ". Taking another snapshot.";
          // look through all the trees
          continue;
        }
        if (actual_value.GetBool() == *bool_value) {
          match = true;
          FX_LOGS(INFO) << moniker << ": Got expected value " << std::boolalpha << *bool_value
                        << " for selector " << selector;
          break;
        } else {
          FX_LOGS(INFO) << moniker << ": Expected value " << std::boolalpha << *bool_value
                        << ", but got " << actual_value.GetBool() << ". Taking another snapshot.";
        }
      }

      uint64_t* uint64_value = std::get_if<uint64_t>(&value);
      if (uint64_value != nullptr) {
        const auto& actual_value = datum.GetByPath(inspect_path);
        // IsUint64 will return false if the value at inspect_path doesn't exist yet,
        // whereas GetUint64 will crash the test
        if (!actual_value.IsUint64()) {
          FX_LOGS(INFO) << moniker << ": Expected value " << *uint64_value
                        << ", but got nothing for selector " << selector
                        << ". Taking another snapshot.";
          // look through all the trees
          continue;
        }
        if (actual_value.GetUint64() == *uint64_value) {
          match = true;
          FX_LOGS(INFO) << moniker << ": Got expected value " << *uint64_value << " for selector "
                        << selector;
          break;
        } else {
          FX_LOGS(INFO) << moniker << ": Expected value " << *uint64_value << ", but got "
                        << actual_value.GetUint64() << ". Taking another snapshot.";
        }
      }
    }
  } while (!match);
}

zx::result<std::string> TestLoopBase::GetPowerElementId(diagnostics::reader::ArchiveReader& reader,
                                                        const std::string& pb_moniker,
                                                        const std::string& power_element_name) {
  FX_LOGS(INFO) << "Searching for power element with name '" << power_element_name
                << "' in Power Broker's topology listing.";
  while (true) {
    auto result = RunPromise(reader.SnapshotInspectUntilPresent({pb_moniker}));
    auto data = result.take_value();
    for (const auto& datum : data) {
      if (datum.moniker() == pb_moniker && datum.payload().has_value()) {
        auto topology = datum.payload().value()->GetByPath(
            {"broker", "topology", "fuchsia.inspect.Graph", "topology"});
        if (topology == nullptr) {
          FX_LOGS(INFO) << "No topology listing in Power Broker's inspect data for moniker: "
                        << datum.moniker();
          break;
        }
        for (const auto& child : topology->children()) {
          const auto& name_node =
              datum.GetByPath({"root", "broker", "topology", "fuchsia.inspect.Graph", "topology",
                               child.name(), "meta", "name"});
          if (name_node.IsNull() || !name_node.IsString()) {
            continue;
          }
          if (name_node.GetString() == power_element_name) {
            FX_LOGS(INFO) << "Power element '" << power_element_name << "' has ID '" << child.name()
                          << "'.";
            return zx::ok(child.name());
          }
        }
        FX_LOGS(INFO) << "Did not find power element '" << power_element_name
                      << "' in Power Broker's topology listing for moniker '" << datum.moniker()
                      << "'.";
      }
    }
    FX_LOGS(INFO) << "Taking another snapshot.";
  }
}

zx::eventpair TestLoopBase::PrepareDriver(std::string_view node_filter,
                                          std::string_view driver_url_suffix, bool expect_new_koid,
                                          bool use_df_elements,
                                          std::vector<CustomDictionaryEntry> custom_entries,
                                          std::vector<std::string> target_child_nodes) {
  if (!use_df_elements && !target_child_nodes.empty()) {
    ADD_FAILURE() << "target_child_nodes requires use_df_elements to be true.";
    return {};
  }

  // Find the node running our target driver.
  if (target_child_nodes.empty()) {
    FX_LOGS(INFO) << "Preparing driver '" << driver_url_suffix << "' for test...";
  } else {
    FX_LOGS(INFO) << "Preparing driver '" << driver_url_suffix
                  << "' with power token overrides for test...";
  }
  std::optional<std::string> found = std::nullopt;
  uint64_t old_koid;
  while (!found) {
    auto node_vec = GetNodeInfo(node_filter);
    FX_LOGS(INFO) << "Found " << node_vec.size() << " nodes matching filter '" << node_filter
                  << "'.";
    for (auto& node : node_vec) {
      if (node.bound_driver_url().has_value() &&
          node.bound_driver_url().value().ends_with(driver_url_suffix)) {
        FX_LOGS(INFO) << "driver found with moniker '" << node.moniker().value() << "'";
        found.emplace(node.moniker().value());
        old_koid = node.driver_host_koid().value();
        break;
      }
    }

    if (!found) {
      FX_LOGS(INFO) << "driver not found, retrying...";
      // Small loop delay.
      RunLoopWithTimeout(zx::msec(10));
    }
  }

  // Setup the power dictionary and restart the node with this dictionary.
  if (target_child_nodes.empty()) {
    FX_LOGS(INFO) << "restarting driver with test dictionary...";
  } else {
    FX_LOGS(INFO) << "restarting driver with test dictionary and child power token overrides...";
  }
  auto dict_ref = CreateDictionaryForTest(std::move(custom_entries));
  zx::eventpair release_fence;

  std::vector<fuchsia_power_broker::LevelDependency> level_deps;

  if (use_df_elements) {
    // Retrieve the CPU token to override.
    auto cpu_token_result = fidl::Call(cpu_element_manager_client_end_)->GetCpuDependencyToken();
    EXPECT_TRUE(cpu_token_result.is_ok());
    if (cpu_token_result.is_error()) {
      FX_LOGS(ERROR) << "Failed to call GetCpuDependencyToken: " << cpu_token_result.error_value();
      return {};
    }
    if (!cpu_token_result->assertive_dependency_token().has_value()) {
      FX_LOGS(ERROR) << "GetCpuDependencyToken response missing assertive_dependency_token";
      return {};
    }

    zx::event cpu_token_override;
    if (zx_status_t status = cpu_token_result->assertive_dependency_token()->duplicate(
            ZX_RIGHT_SAME_RIGHTS, &cpu_token_override);
        status != ZX_OK) {
      EXPECT_EQ(ZX_OK, status);
      return {};
    }

    std::vector<fuchsia_driver_development::NodePowerTokenOverride> node_token_overrides;

    for (const auto& child_name : target_child_nodes) {
      zx_status_t status = ZX_OK;
      zx::event token_for_node, token_for_dep, token_for_dev_mgr;
      if (status = zx::event::create(0, &token_for_node); status != ZX_OK) {
        EXPECT_EQ(ZX_OK, status);
        return {};
      }
      if (status = token_for_node.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_for_dep);
          status != ZX_OK) {
        EXPECT_EQ(ZX_OK, status);
        return {};
      }
      if (status = token_for_node.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_for_dev_mgr);
          status != ZX_OK) {
        EXPECT_EQ(ZX_OK, status);
        return {};
      }

      fuchsia_power_broker::LevelDependency level_dep;
      level_dep.dependent_level(1);
      level_dep.requires_token(std::move(token_for_dep));
      level_dep.requires_level_by_preference(std::vector<uint8_t>{1});
      level_deps.push_back(std::move(level_dep));

      fuchsia_driver_development::NodePowerTokenOverride override_item;
      override_item.target_node(child_name);
      override_item.token(std::move(token_for_dev_mgr));
      node_token_overrides.push_back(std::move(override_item));
    }

    auto result =
        fidl::Call(driver_manager_client_end_)
            ->RestartWithDictionaryAndPowerDependencies({*found,
                                                         std::move(dict_ref),
                                                         {},
                                                         std::move(cpu_token_override),
                                                         std::move(node_token_overrides)});
    EXPECT_EQ(true, result.is_ok());

    if (result.is_error()) {
      return {};
    }
    release_fence = std::move(result.value().release_fence());
  } else {
    auto result = fidl::Call(driver_manager_client_end_)
                      ->RestartWithDictionary({*found, std::move(dict_ref)});
    EXPECT_EQ(true, result.is_ok());

    if (result.is_error()) {
      return {};
    }
    release_fence = std::move(result.value().release_fence());
  }

  if (!expect_new_koid) {
    // Let the restart make progress, otherwise our next loop could run early enough to see the old
    // instance of the target node since its not doing the koid comparison.
    RunLoopWithTimeout(zx::sec(1));
  }

  FX_LOGS(INFO) << "checking for the driver to be up again...";
  found = std::nullopt;

  // Wait until the node is available again, possibly under a new driver host.
  while (!found) {
    auto node_vec = GetNodeInfo(node_filter);
    for (auto& node : node_vec) {
      if (node.bound_driver_url().has_value() &&
          node.bound_driver_url().value().ends_with(driver_url_suffix)) {
        if (node.driver_host_koid().has_value()) {
          if (!expect_new_koid || old_koid != node.driver_host_koid().value()) {
            found.emplace(node.moniker().value());
            break;
          }
        }
      }
    }

    if (!found) {
      FX_LOGS(INFO) << "driver not found, retrying...";
      // Small loop delay.
      RunLoopWithTimeout(zx::msec(10));
    }
  }

  if (target_child_nodes.empty()) {
    FX_LOGS(INFO) << "proceeding with test!";
    return release_fence;
  }

  for (const auto& child_name : target_child_nodes) {
    FX_LOGS(INFO) << "checking for target child node '" << child_name << "' to be up and bound...";
    bool child_found = false;
    while (!child_found) {
      auto child_vec = GetNodeInfo(child_name);
      for (auto& child_node : child_vec) {
        std::string driver_url = child_node.bound_driver_url().has_value()
                                     ? child_node.bound_driver_url().value()
                                     : "<none>";
        FX_LOGS(INFO) << "Found candidate child node '" << child_name << "' with moniker '"
                      << child_node.moniker().value_or("<none>") << "' and driver URL '"
                      << driver_url << "'";
        if (child_node.bound_driver_url().has_value() &&
            !child_node.bound_driver_url().value().empty() &&
            child_node.bound_driver_url().value() != "unbound" &&
            child_node.bound_driver_url().value() != "owned by composite(s)") {
          FX_LOGS(INFO) << "child node '" << child_name << "' successfully bound to driver '"
                        << child_node.bound_driver_url().value() << "'";
          child_found = true;
          break;
        }
      }
      if (!child_found) {
        RunLoopWithTimeout(zx::msec(10));
      }
    }
  }

  FX_LOGS(INFO)
      << "all target child nodes are up! Creating 'fake-all-drivers' aggregator power element...";
  auto [control_client, control_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();
  auto [runner_client, runner_server] =
      fidl::Endpoints<fuchsia_power_broker::ElementRunner>::Create();

  test_all_drivers_runner_ = std::make_unique<BasicElementRunner>(std::move(runner_server));

  fuchsia_power_broker::ElementSchema schema;
  schema.element_name("fake-all-drivers");
  schema.initial_current_level(1);
  schema.valid_levels(std::vector<uint8_t>{0, 1});
  schema.dependencies(std::move(level_deps));
  schema.element_control(std::move(control_server));
  schema.element_runner(std::move(runner_client));

  auto topology_client = component::Connect<fuchsia_power_broker::Topology>();
  EXPECT_TRUE(topology_client.is_ok());
  if (topology_client.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to Topology protocol: " << topology_client.status_string();
    return {};
  }
  auto add_result = fidl::Call(*topology_client)->AddElement(std::move(schema));
  EXPECT_TRUE(add_result.is_ok());
  if (add_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add fake-all-drivers element: " << add_result.error_value();
    return {};
  }

  zx::event element_token, token_dupe;
  zx_status_t status = ZX_OK;
  if (status = zx::event::create(0, &element_token); status != ZX_OK) {
    EXPECT_EQ(ZX_OK, status);
    return {};
  }
  if (status = element_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_dupe); status != ZX_OK) {
    EXPECT_EQ(ZX_OK, status);
    return {};
  }

  auto reg_result =
      fidl::Call(control_client)->RegisterDependencyToken({{.token = std::move(token_dupe)}});
  EXPECT_TRUE(reg_result.is_ok());
  if (reg_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to register dependency token: " << reg_result.error_value();
    return {};
  }

  fuchsia_power_system::CpuElementManagerAddExecutionStateDependencyRequest add_dep_req;
  add_dep_req.dependency_token(std::move(element_token));
  add_dep_req.power_level(1);
  auto add_dep_result = fidl::Call(cpu_element_manager_client_end_)
                            ->AddExecutionStateDependency(std::move(add_dep_req));
  EXPECT_TRUE(add_dep_result.is_ok());
  if (add_dep_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add execution state dependency for fake-all-drivers: "
                   << add_dep_result.error_value();
    return {};
  }

  test_all_drivers_control_ = std::move(control_client);

  FX_LOGS(INFO) << "proceeding with test!";
  return release_fence;
}

zx::eventpair TestLoopBase::PrepareDriverWithPowerTokenOverrides(
    std::string_view node_filter, std::string_view driver_url_suffix, bool expect_new_koid,
    const std::vector<std::string>& target_child_nodes,
    std::vector<CustomDictionaryEntry> custom_entries) {
  return PrepareDriver(node_filter, driver_url_suffix, expect_new_koid,
                       /*use_df_elements=*/true, std::move(custom_entries), target_child_nodes);
}

fuchsia_component_sandbox::DictionaryRef TestLoopBase::CreateDictionaryForTest(
    std::vector<CustomDictionaryEntry> custom_entries) {
  // Start a background loop to run the sandbox connectors.
  sandbox_connector_loop_.StartThread("sandbox-loop");

  // Counter for the capability store.
  auto cap_store = component::Connect<fuchsia_component_sandbox::CapabilityStore>();
  uint32_t dict_id = next_cap_id_++;
  auto capstore_result = fidl::Call(*cap_store)->DictionaryCreate({dict_id});
  EXPECT_EQ(true, capstore_result.is_ok());

  auto [sag_client, sag_server] = fidl::Endpoints<fuchsia_component_sandbox::Receiver>::Create();
  uint32_t sag_id = next_cap_id_++;
  auto connector_res = fidl::Call(*cap_store)->ConnectorCreate({sag_id, {std::move(sag_client)}});
  EXPECT_EQ(true, connector_res.is_ok());

  auto [broker_client, broker_server] =
      fidl::Endpoints<fuchsia_component_sandbox::Receiver>::Create();
  uint32_t broker_id = next_cap_id_++;
  connector_res = fidl::Call(*cap_store)->ConnectorCreate({broker_id, {std::move(broker_client)}});
  EXPECT_EQ(true, connector_res.is_ok());

  auto [cpu_element_client, cpu_element_server] =
      fidl::Endpoints<fuchsia_component_sandbox::Receiver>::Create();
  uint32_t cpu_element_id = next_cap_id_++;
  connector_res =
      fidl::Call(*cap_store)->ConnectorCreate({cpu_element_id, {std::move(cpu_element_client)}});
  EXPECT_EQ(true, connector_res.is_ok());

  auto insert_res =
      fidl::Call(*cap_store)
          ->DictionaryInsert({dict_id, {"fuchsia.power.system.ActivityGovernor", sag_id}});
  EXPECT_EQ(true, insert_res.is_ok());

  insert_res = fidl::Call(*cap_store)
                   ->DictionaryInsert({dict_id, {"fuchsia.power.broker.Topology", broker_id}});
  EXPECT_EQ(true, insert_res.is_ok());

  insert_res =
      fidl::Call(*cap_store)
          ->DictionaryInsert({dict_id, {"fuchsia.power.system.CpuElementManager", cpu_element_id}});
  EXPECT_EQ(true, insert_res.is_ok());

  for (auto& entry : custom_entries) {
    uint32_t cap_id = next_cap_id_++;
    auto connector_res =
        fidl::Call(*cap_store)->ConnectorCreate({cap_id, {std::move(entry.client_end)}});
    EXPECT_EQ(true, connector_res.is_ok());

    auto insert_custom_res =
        fidl::Call(*cap_store)->DictionaryInsert({dict_id, {entry.name, cap_id}});
    EXPECT_EQ(true, insert_custom_res.is_ok());
  }

  auto dict_export = fidl::Call(*cap_store)->Export({dict_id});
  auto dict_ref = std::move(dict_export->capability().dictionary().value());

  sag_connector_.emplace(
      async_patterns::PassDispatcher,
      std::string(fidl::DiscoverableProtocolDefaultPath<fuchsia_power_system::ActivityGovernor>),
      std::move(sag_server));

  broker_connector_.emplace(
      async_patterns::PassDispatcher,
      std::string(fidl::DiscoverableProtocolDefaultPath<fuchsia_power_broker::Topology>),
      std::move(broker_server));

  cpu_element_connector_.emplace(
      async_patterns::PassDispatcher,
      std::string(fidl::DiscoverableProtocolDefaultPath<fuchsia_power_system::CpuElementManager>),
      std::move(cpu_element_server));

  return dict_ref;
}

std::vector<fuchsia_driver_development::NodeInfo> TestLoopBase::GetNodeInfo(
    std::string_view node_filter) {
  std::vector<fuchsia_driver_development::NodeInfo> result;

  auto [info_client, info_server] =
      fidl::Endpoints<fuchsia_driver_development::NodeInfoIterator>::Create();

  auto info = fidl::Call(driver_manager_client_end_)
                  ->GetNodeInfo({{std::string(node_filter)}, std::move(info_server), false});
  EXPECT_EQ(true, info.is_ok());

  while (true) {
    auto info_next = fidl::Call(info_client)->GetNext();
    EXPECT_EQ(true, info_next.is_ok());

    if (info_next->nodes().empty()) {
      break;
    }

    for (auto& node : info_next->nodes()) {
      result.push_back(node);
    }
  }

  return result;
}

}  // namespace system_integration_utils
