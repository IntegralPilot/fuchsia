// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/vsock-sshd-host/service.h"

#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.vsock/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/socket.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <vector>

#include "src/lib/fxl/strings/string_printf.h"

namespace sshd_host {

Service::Service(async_dispatcher_t* dispatcher, uint16_t port) : dispatcher_(dispatcher) {
  auto connector = component::Connect<fuchsia_vsock::Connector>();
  if (connector.is_error()) {
    FX_LOGS(FATAL) << "Failed to connect to vsock connector" << connector.status_string();
  }

  FX_LOG_KV(INFO, "listen() for inbound SSH connections", FX_KV("port", int{port}));
  auto [acceptor_client, acceptor_server] = fidl::Endpoints<fuchsia_vsock::Acceptor>::Create();
  auto listen_result = fidl::Call(*connector)
                           ->Listen({{
                               .local_port = port,
                               .acceptor = std::move(acceptor_client),
                           }});
  if (listen_result.is_error()) {
    FX_LOGS(FATAL) << "Failed to listen: " << listen_result.error_value();
  }

  binding_.emplace(dispatcher_, std::move(acceptor_server), this, fidl::kIgnoreBindingClosure);
}

Service::~Service() = default;

void Service::Accept(AcceptRequest& request, AcceptCompleter::Sync& completer) {
  FX_LOG_KV(INFO, "Accepted connection", FX_KV("local_port", request.addr().local_port()),
            FX_KV("remote_cid", request.addr().remote_cid()),
            FX_KV("remote_port", request.addr().remote_port()));

  zx::socket sock1, sock2;
  ZX_ASSERT(zx::socket::create(0, &sock1, &sock2) == ZX_OK);

  Launch(std::move(sock1));

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_vsock::Connection>::Create();
  client_ends_.push_back(std::move(client_end));
  completer.Reply({{std::make_unique<fuchsia_vsock::ConnectionTransport>(std::move(sock2),
                                                                         std::move(server_end))}});
}

void Service::Launch(zx::socket socket) {
  uint64_t child_num = next_child_num_++;
  std::string child_name = fxl::StringPrintf("sshd-%lu", child_num);

  auto realm_client_end = component::Connect<fuchsia_component::Realm>();
  if (realm_client_end.is_error()) {
    FX_PLOGS(ERROR, realm_client_end.status_value()) << "Failed to connect to realm service";
    return;
  }

  fidl::SyncClient<fuchsia_component::Realm> realm{std::move(*realm_client_end)};

  auto controller_endpoints = fidl::CreateEndpoints<fuchsia_component::Controller>();
  if (controller_endpoints.is_error()) {
    FX_PLOGS(ERROR, controller_endpoints.status_value())
        << "Failed to connect to create controller endpoints";
    return;
  }

  fidl::SyncClient<fuchsia_component::Controller> controller{
      std::move(controller_endpoints->client)};
  {
    fuchsia_component_decl::CollectionRef collection{{
        .name = std::string(kShellCollection),
    }};
    fuchsia_component_decl::Child decl{{.name = child_name,
                                        .url = "#meta/vsock-sshd.cm",
                                        .startup = fuchsia_component_decl::StartupMode::kLazy}};

    fuchsia_component::CreateChildArgs args{
        {.controller = std::move(controller_endpoints->server)}};

    auto result = realm->CreateChild(
        {{.collection = collection, .decl = std::move(decl), .args = std::move(args)}});
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Failed to create sshd child: " << result.error_value().FormatDescription();
      return;
    }
  }

  auto execution_controller_endpoints =
      fidl::CreateEndpoints<fuchsia_component::ExecutionController>();
  if (execution_controller_endpoints.is_error()) {
    FX_LOGS(ERROR) << "Failed to create execution controller endpoints: "
                   << execution_controller_endpoints.status_string();
    return;
  }

  controllers_.emplace(std::piecewise_construct, std::forward_as_tuple(child_num),
                       std::forward_as_tuple(this, child_num, std::move(child_name),
                                             std::move(execution_controller_endpoints->client),
                                             dispatcher_, std::move(realm)));
  auto remove_controller_on_error =
      fit::defer([this, child_num]() { controllers_.erase(child_num); });

  // Pass the socket as stdin and stdout handles to the sshd component.
  std::vector<fuchsia_process::HandleInfo> numbered_handles;
  zx::socket socket_dup;
  if (zx_status_t status = socket.duplicate(ZX_RIGHT_SAME_RIGHTS, &socket_dup); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to clone connection socket " << socket.get();
    return;
  }
  numbered_handles.push_back(fuchsia_process::HandleInfo{
      {.handle = std::move(socket_dup), .id = PA_HND(PA_FD, STDIN_FILENO)}});
  numbered_handles.push_back(fuchsia_process::HandleInfo{
      {.handle = std::move(socket), .id = PA_HND(PA_FD, STDOUT_FILENO)}});

  auto result = controller->Start(
      {{.args = {{
            .numbered_handles = std::move(numbered_handles),
            .namespace_entries = {},
        }},
        .execution_controller = std::move(execution_controller_endpoints->server)}});

  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to start sshd child: " << result.error_value().FormatDescription();
    return;
  }

  remove_controller_on_error.cancel();
}

void Service::OnStop(zx_status_t status, Controller* ptr) {
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "sshd component stopped with error";
  }

  // Destroy the component.
  auto result = ptr->realm_->DestroyChild({{.child = {{
                                                .name = ptr->child_name_,
                                                .collection = std::string(kShellCollection),

                                            }}}});
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to destroy sshd child: " << result.error_value().FormatDescription();
  }

  // Remove the controller.
  controllers_.erase(ptr->child_num_);
}

}  // namespace sshd_host
