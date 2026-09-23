// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "icd_runner.h"

#include <fidl/fuchsia.component.runner/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/remote_dir.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

ComponentControllerImpl::ComponentControllerImpl(async_dispatcher_t* dispatcher)
    : vfs_(dispatcher) {}

zx::result<std::unique_ptr<fidl::Server<fuchsia_component_runner::ComponentController>>>
ComponentControllerImpl::Bind(
    async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_component_runner::ComponentController> controller,
    fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir,
    fidl::ClientEnd<fuchsia_io::Directory> pkg_directory) {
  std::unique_ptr<ComponentControllerImpl> server(new ComponentControllerImpl(dispatcher));

  auto root = fbl::MakeRefCounted<fs::PseudoDir>();
  auto remote = fbl::MakeRefCounted<fs::RemoteDir>(std::move(pkg_directory));
  root->AddEntry("pkg", remote);
  zx_status_t status =
      server->vfs_.ServeDirectory(root, std::move(outgoing_dir), fuchsia_io::kRxStarDir);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to serve package directory!";
    return zx::error(status);
  }
  server->binding_ = fidl::BindServer(dispatcher, std::move(controller), server.get());
  return zx::ok(std::move(server));
}

zx::result<> IcdRunnerImpl::Add(std::unique_ptr<IcdRunnerImpl> component_runner,
                                component::OutgoingDirectory& outgoing_dir) {
  return outgoing_dir.AddProtocol<fuchsia_component_runner::ComponentRunner>(
      std::move(component_runner));
}

void IcdRunnerImpl::Start(StartRequest& request, StartCompleter::Sync& completer) {
  fidl::ServerEnd controller = std::move(request.controller());
  if (!controller.is_valid()) {
    FX_LOGS(ERROR) << "Invalid controller handle in start request!";
    controller.Close(ZX_ERR_BAD_HANDLE);
    return;
  }

  if (!request.start_info().outgoing_dir()) {
    FX_LOGS(ERROR) << "Missing outgoing directory handle in start request!";
    controller.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  fidl::ServerEnd outgoing_dir = *std::move(request.start_info().outgoing_dir());
  if (!outgoing_dir.is_valid()) {
    FX_LOGS(ERROR) << "Invalid outgoing directory handle in start request!";
    controller.Close(ZX_ERR_BAD_HANDLE);
    return;
  }

  fidl::ClientEnd<fuchsia_io::Directory> pkg_directory;
  for (auto& ns_entry : *request.start_info().ns()) {
    if (ns_entry.path() == std::nullopt || ns_entry.directory() == std::nullopt) {
      break;
    }
    if (ns_entry.path() != "/pkg") {
      continue;
    }
    pkg_directory = std::move(*ns_entry.directory());
    break;
  }
  if (!pkg_directory.is_valid()) {
    FX_LOGS(ERROR) << "No package directory found for " << *request.start_info().resolved_url();
    request.controller().Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  zx::result controller_server = ComponentControllerImpl::Bind(
      dispatcher_, std::move(controller), std::move(outgoing_dir), std::move(pkg_directory));
  if (controller_server.is_ok()) {
    controller_server_ = *std::move(controller_server);
  } else {
    FX_LOGS(ERROR) << "Failed to bind controller: " << controller_server.status_string();
  }
}
