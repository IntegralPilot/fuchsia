// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adb-file-sync.h"

#include <fuchsia/io/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/fdio.h>
#include <lib/syslog/cpp/macros.h>

#include <vector>

#include "fidl/fuchsia.sys2/cpp/common_types.h"
#include "src/developer/adb/third_party/adb-file-sync/file_sync_service.h"
#include "src/developer/adb/third_party/adb-file-sync/util.h"

namespace adb_file_sync {

zx_status_t AdbFileSync::StartService(adb_file_sync_config::Config config) {
  FX_LOGS(DEBUG) << "Starting ADB File Sync Service";
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  AdbFileSync file_sync(std::move(config), loop.dispatcher());

  auto endpoints = fidl::CreateEndpoints<fuchsia_sys2::RealmQuery>();
  if (endpoints.is_error()) {
    FX_LOGS(ERROR) << "Could not create endpoints " << endpoints.error_value();
    return endpoints.error_value();
  }
  auto status = file_sync.context_->svc()->Connect("fuchsia.sys2.RealmQuery.root",
                                                   endpoints->server.TakeChannel());
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Could not connect to cache RealmQuery " << status;
    return status;
  }
  file_sync.realm_query_.Bind(std::move(endpoints->client));

  auto lifecycle_ep = fidl::CreateEndpoints<fuchsia_sys2::LifecycleController>();
  if (lifecycle_ep.is_error()) {
    FX_LOGS(ERROR) << "Could not create endpoints " << lifecycle_ep.error_value();
    return lifecycle_ep.error_value();
  }
  status = file_sync.context_->svc()->Connect("fuchsia.sys2.LifecycleController.root",
                                              lifecycle_ep->server.TakeChannel());
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Could not connect to cache RealmQuery " << status;
    return status;
  }
  file_sync.lifecycle_.Bind(std::move(lifecycle_ep->client));

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());

  auto result = outgoing.AddUnmanagedProtocol<fuchsia_hardware_adb::Provider>(
      [&file_sync, &loop](fidl::ServerEnd<fuchsia_hardware_adb::Provider> server_end) {
        file_sync.binding_ref_.emplace(fidl::BindServer(loop.dispatcher(), std::move(server_end),
                                                        &file_sync,
                                                        std::mem_fn(&AdbFileSync::OnUnbound)));
      });
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Could not publish service " << result.error_value();
    return result.error_value();
  }

  result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return result.error_value();
  }

  loop.Run();
  return ZX_OK;
}

void AdbFileSync::OnUnbound(fidl::UnbindInfo info,
                            fidl::ServerEnd<fuchsia_hardware_adb::Provider> server_end) {
  if (info.is_user_initiated()) {
    return;
  }
  if (info.is_peer_closed()) {
    // If the peer (the client) closed their endpoint, log that as DEBUG.
    FX_LOGS(DEBUG) << "Client disconnected";
  } else {
    // Treat other unbind causes as errors.
    FX_LOGS(ERROR) << "Server error: " << info;
  }
}

void AdbFileSync::ConnectToService(
    fuchsia_hardware_adb::wire::ProviderConnectToServiceRequest* request,
    ConnectToServiceCompleter::Sync& completer) {
  completer.Reply(fit::ok());
  file_sync_service(this, std::move(request->socket));
}

zx::result<zx::channel> AdbFileSync::ConnectToComponent(std::string name,
                                                        std::vector<std::string>* out_path) {
  const std::string kDeliminator = "::";

  // Parse component moniker
  const auto component_path = split_string(name, kDeliminator);
  std::string component_moniker;
  std::string path;
  if (component_path.size() == 1) {
    component_moniker = config_.filesync_moniker();
    path = component_path[0];
  } else if (component_path.size() == 2) {
    component_moniker = component_path[0];
    path = component_path[1];
  } else {
    FX_LOGS(ERROR) << "Invalid address! " << component_path.size() << " " << name;
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (component_moniker.empty()) {
    FX_LOGS(ERROR) << "Must have a component!";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (component_moniker[0] != '.') {
    component_moniker.insert(0, ".");
  }

  // Resolve component moniker
  auto resolve_result = lifecycle_->ResolveInstance(component_moniker);
  if (resolve_result.is_error()) {
    FX_LOGS(ERROR) << "FIDL call to resolve moniker failed" << resolve_result.error_value();
    return zx::error(resolve_result.error_value().is_domain_error()
                         ? static_cast<uint32_t>(resolve_result.error_value().domain_error())
                         : resolve_result.error_value().framework_error().status());
  }

  // Connect to component
  auto result = realm_query_->ConstructNamespace(component_moniker);
  if (result.is_error()) {
    FX_LOGS(ERROR) << "RealmQuery failed " << result.error_value().FormatDescription();
    return zx::error(result.error_value().is_domain_error()
                         ? static_cast<uint32_t>(result.error_value().domain_error())
                         : result.error_value().framework_error().status());
  }

  if (result->namespace_().empty()) {
    FX_LOGS(ERROR) << "RealmQuery did not return any directories.";
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (!path.starts_with("/")) {
    path = "/" + path;
  }
  for (auto& entry : result->namespace_()) {
    // `entry.path()` might contain more than one "/", like "/config/data"
    // `path` might include extra mode at the end like "/some/path,0755", and we should
    // keep that in out_path->back()
    if (entry.path().has_value() && path.starts_with(entry.path().value())) {
      auto sub_path = path.substr(entry.path()->size());
      // prevent matching if path is "/ab" and entry.path() is "/a"
      if (sub_path != "" && sub_path[0] != '/' && sub_path[0] != ',') {
        continue;
      }

      *out_path = split_string(sub_path, "/");
      return zx::success(entry.directory()->TakeChannel());
    }
  }

  FX_LOGS(ERROR) << "Could not find directory for " << path;
  return zx::error(ZX_ERR_NOT_FOUND);
}

}  // namespace adb_file_sync
