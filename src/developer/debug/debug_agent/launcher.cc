// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/launcher.h"

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.debugger/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/processargs.h>

#include <charconv>
#include <deque>
#include <random>

#include "lib/async/cpp/task.h"
#include "lib/fit/defer.h"

namespace {

constexpr char kChildUrl[] = "fuchsia-pkg://fuchsia.com/debug_agent#meta/debug_agent.cm";
constexpr char kCollectionName[] = "agents";
constexpr char kAgentPrefix[] = "agent-";

// Convert various result types to zx_status_t.
template <typename ResultType>
zx_status_t ResultToStatus(const ResultType& result) {
  zx_status_t error = ZX_OK;
  if (result.is_error()) {
    if (result.error_value().is_domain_error()) {
      error = fidl::ToUnderlying(result.error_value().domain_error());
    } else {
      error = result.error_value().framework_error().status();
    }
  }
  return error;
}

std::string GetRandomHexString() {
  std::mt19937 prng{std::random_device{}()};
  uint32_t num = prng();

  std::string s;
  s.resize(8);
  auto [end, ec] = std::to_chars(s.data(), s.data() + s.size(), num, 16);
  FX_CHECK(ec == std::errc()) << std::make_error_code(ec).message();

  return std::string{s.data(), end};
}

fidl::ClientEnd<fuchsia_component::Realm> ConnectToRealmQuery() {
  zx::result realm_client_end = component::Connect<fuchsia_component::Realm>();
  FX_CHECK(realm_client_end.is_ok());
  return std::move(*realm_client_end);
}

class AgentIterator;

void ConnectToAgentAt(const std::string& name,
                      fidl::ServerEnd<fuchsia_debugger::DebugAgent> server_end) {
  fidl::Client<fuchsia_component::Realm> realm(ConnectToRealmQuery(),
                                               async_get_default_dispatcher());

  fuchsia_component_decl::ChildRef child_ref(name, kCollectionName);

  auto [dir_client_end, dir_server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  realm->OpenExposedDir({{.child = child_ref, .exposed_dir = std::move(dir_server_end)}})
      .Then([name, server_end = std::move(server_end), dir_client_end = std::move(dir_client_end),
             realm = std::move(realm)](auto& result) mutable {
        if (auto status = ResultToStatus(result); status != ZX_OK) {
          FX_LOGS(WARNING) << "OpenExposedDir failed: " << result.error_value();
          return;
        }

        auto connect_at_result = component::ConnectAt<fuchsia_debugger::DebugAgent>(
            dir_client_end, std::move(server_end));
        if (connect_at_result.is_error()) {
          FX_LOGS(WARNING) << "Failed to connect to " << name << ": "
                           << connect_at_result.error_value();
          return;
        }
      });
}

class AgentIterator : public fidl::Server<fuchsia_debugger::AgentIterator> {
 public:
  void GetNext(GetNextCompleter::Sync& completer) override {
    GetNextCompleter::Async async_completer = completer.ToAsync();

    // First call, get the remote iterator from ComponentManager.
    if (!child_iterator_.is_valid()) {
      fidl::Client<fuchsia_component::Realm> realm(ConnectToRealmQuery(),
                                                   async_get_default_dispatcher());

      fuchsia_component_decl::CollectionRef collection(kCollectionName);
      auto [client_end, server_end] = fidl::Endpoints<fuchsia_component::ChildIterator>::Create();

      child_iterator_ = fidl::Client(std::move(client_end), async_get_default_dispatcher());

      realm->ListChildren({collection, std::move(server_end)})
          .Then([this, completer = &async_completer, realm = std::move(realm)](
                    fidl::Result<fuchsia_component::Realm::ListChildren>& result) mutable {
            if (auto status = ResultToStatus(result); status != ZX_OK) {
              FX_LOGS(WARNING) << "ListChildren failed: " << result.error_value();
              if (auto unbind_result = child_iterator_.UnbindMaybeGetEndpoint();
                  unbind_result.is_error()) {
                FX_LOGS(WARNING) << "Failed to unbind iterator: " << unbind_result.error_value();
              }
              completer->Close(status);
            }
          });
    } else if (!agents_.empty()) {
      async::PostTask(async_get_default_dispatcher(),
                      [this, completer = std::move(async_completer)]() mutable {
                        completer.Reply({{std::move(agents_)}});
                      });
      return;
    }

    // Get the next batch of results, |children| will be an empty vector when there are no more
    // child instances. It is possible for this call to return more results than we can send in a
    // single response.
    if (child_iterator_.is_valid()) {
      child_iterator_->Next().Then(
          [this, completer = std::move(async_completer)](
              fidl::Result<fuchsia_component::ChildIterator::Next>& children) mutable {
            if (children.is_error()) {
              FX_LOGS(WARNING) << "Child iterator returned error: " << children.error_value();
              completer.Close(ZX_ERR_UNAVAILABLE);
              return;
            } else if (children->children().empty()) {
              // No more children, reply with what we have an hang up.
              completer.Reply(std::move(agents_));
              completer.Close(ZX_OK);
              return;
            }

            OnChildrenReady(children->children(), std::move(completer));
          });
    }
  }

 private:
  void OnChildrenReady(const std::vector<fuchsia_component_decl::ChildRef>& children,
                       GetNextCompleter::Async completer) {
    std::vector<fuchsia_debugger::Agent> batch;

    // There's no way to query the completer to know that we've given a response already, but we can
    // still collate data from the child iterator while we wait for another request from the client.
    bool completer_called = false;

    for (auto it = children.begin(); it != children.end(); ++it) {
      auto [client_end, server_end] = fidl::Endpoints<fuchsia_debugger::DebugAgent>::Create();

      if (!completer_called) {
        batch.emplace_back(it->name(), std::move(client_end));

        if (batch.size() == kMaxBatchedAgents || std::next(it) == children.end()) {
          completer.Reply(std::move(batch));
          completer_called = true;
        }
      } else {
        // Buffer the rest of the agents to be returned in the next |GetNext| call from the client.
        agents_.emplace_back(it->name(), std::move(client_end));
      }

      // This will finish asynchronously, but we don't need to worry about the result here. If an
      // error occurs, the client_end will see a PEER_CLOSED.
      ConnectToAgentAt(it->name(), std::move(server_end));
    }
  }

  // Each struct is a string and a handle, the string is the child component's name which can be up
  // to 1024 bytes, which is conveniently the same as the 64 handle limit per message.
  static constexpr size_t kMaxBatchedAgents = 64;

  fidl::Client<fuchsia_component::ChildIterator> child_iterator_;
  std::vector<fuchsia_debugger::Agent> agents_;
};

}  // namespace

void DebugAgentLauncher::Launch(LaunchRequest& request, LaunchCompleter::Sync& completer) {
  std::string name(kAgentPrefix);
  name += GetRandomHexString();

  LaunchDebugAgent(std::move(request.agent()), request.options(), name,
                   [completer = completer.ToAsync()](zx_status_t status) mutable {
                     completer.Reply(zx::make_result(status));
                   });
}

void DebugAgentLauncher::GetAgents(GetAgentsRequest& request, GetAgentsCompleter::Sync& completer) {
  fidl::BindServer(async_get_default_dispatcher(),
                   fidl::ServerEnd<fuchsia_debugger::AgentIterator>(std::move(request.iterator())),
                   std::make_unique<AgentIterator>());
}

fuchsia_component_decl::Child DebugAgentLauncher::CreateChildDecl(
    const std::string& name, const fuchsia_debugger::LaunchOptions& options) {
  fuchsia_component_decl::Child child_decl;
  child_decl.name(name);
  child_decl.url(kChildUrl);
  child_decl.startup(fuchsia_component_decl::StartupMode::kLazy);

  if (options.monitor_root_job().has_value()) {
    fuchsia_component_decl::ConfigOverride override;
    override.key("monitor_root_job");
    override.value(fuchsia_component_decl::ConfigValue::WithSingle(
        fuchsia_component_decl::ConfigSingleValue::WithBool_(options.monitor_root_job().value())));
    child_decl.config_overrides({{override}});
  }
  return child_decl;
}

void DebugAgentLauncher::LaunchDebugAgent(fidl::ServerEnd<fuchsia_debugger::DebugAgent> server_end,
                                          const fuchsia_debugger::LaunchOptions& options,
                                          const std::string& name,
                                          fit::callback<void(zx_status_t)> cb) {
  zx::result client_end = component::Connect<fuchsia_component::Realm>();
  fidl::Client<fuchsia_component::Realm> realm(std::move(*client_end),
                                               async_get_default_dispatcher());

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_component::Controller>::Create();

  fidl::Client<fuchsia_component::Controller> controller(std::move(controller_client_end),
                                                         async_get_default_dispatcher());

  // Create the child decl and all the starting parameters.
  fuchsia_component_decl::Child child_decl = CreateChildDecl(name, options);

  fuchsia_component_decl::CollectionRef collection(kCollectionName);

  std::vector<fuchsia_process::HandleInfo> handles;
  handles.emplace_back(server_end.TakeHandle(), PA_HND(PA_USER0, 0));

  fuchsia_component::CreateChildArgs args;
  args.numbered_handles() = std::move(handles);
  args.controller(std::move(controller_server_end));

  // Create and Start the child.
  realm->CreateChild({collection, child_decl, std::move(args)})
      .Then([realm = std::move(realm), controller = std::move(controller), cb = std::move(cb)](
                fidl::Result<fuchsia_component::Realm::CreateChild>& result) mutable {
        if (auto status = ResultToStatus(result); status != ZX_OK) {
          return cb(status);
        }

        controller->IsStarted().Then(
            [controller = std::move(controller), cb = std::move(cb)](
                fidl::Result<fuchsia_component::Controller::IsStarted>& result) mutable {
              if (auto status = ResultToStatus(result); status != ZX_OK) {
                return cb(status);
              }

              // If the component is already started, then we're done. This is
              // not guaranteed by the Realm protocol.
              if (result.value().is_started()) {
                return cb(ZX_OK);
              }

              // Otherwise, we need to explicitly start the child.
              auto [exec_controller_client, exec_controller_server] =
                  fidl::Endpoints<fuchsia_component::ExecutionController>::Create();
              fidl::Client<fuchsia_component::ExecutionController> execution_controller(
                  std::move(exec_controller_client), async_get_default_dispatcher());

              controller->Start({{.execution_controller = std::move(exec_controller_server)}})
                  .Then([controller = std::move(controller), cb = std::move(cb)](
                            fidl::Result<fuchsia_component::Controller::Start>& result) mutable {
                    if (auto status = ResultToStatus(result); status != ZX_OK) {
                      return cb(status);
                    }

                    cb(ZX_OK);
                  });
            });
      });
}
