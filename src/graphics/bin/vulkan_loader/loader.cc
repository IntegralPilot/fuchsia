// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/bin/vulkan_loader/loader.h"

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

LoaderImpl::~LoaderImpl() { app_->RemoveObserver(this); }

zx::result<> LoaderImpl::Add(component::OutgoingDirectory& outgoing_dir, LoaderApp* app,
                             async_dispatcher_t* dispatcher) {
  auto protocol_handler = [app,
                           dispatcher](fidl::ServerEnd<fuchsia_vulkan_loader::Loader> server_end) {
    std::unique_ptr<LoaderImpl> impl(new LoaderImpl(app, false));
    fidl::BindServer(dispatcher, std::move(server_end), std::move(impl));
  };
  if (auto result =
          outgoing_dir.AddUnmanagedProtocol<fuchsia_vulkan_loader::Loader>(protocol_handler);
      result.is_error()) {
    return result;
  }

  auto trusted_protocol_handler =
      [app, dispatcher](fidl::ServerEnd<fuchsia_vulkan_loader::Loader> server_end) mutable {
        std::unique_ptr<LoaderImpl> impl(new LoaderImpl(app, true));
        fidl::BindServer(dispatcher, std::move(server_end), std::move(impl));
      };

  fuchsia_vulkan_loader::TrustedService::InstanceHandler handler(
      {.loader = std::move(trusted_protocol_handler)});

  return outgoing_dir.AddService<fuchsia_vulkan_loader::TrustedService>(std::move(handler));
}

// LoaderApp::Observer implementation.
void LoaderImpl::OnIcdListChanged(LoaderApp* app) {
  auto it = callbacks_.begin();
  while (it != callbacks_.end()) {
    std::optional<zx::vmo> vmo = app->GetMatchingIcd(it->first);
    if (!vmo) {
      ++it;
    } else {
      it->second.Reply(*std::move(vmo));
      it = callbacks_.erase(it);
    }
  }
  if (!app->HavePendingActions()) {
    for (auto& handle : connect_manifest_handles_) {
      app_->ServeManifestFs(std::move(handle));
    }
    connect_manifest_handles_.clear();
  }
  if (!waiting_for_callbacks()) {
    app_->RemoveObserver(this);
  }
}

// fuchsia::vulkan::loader::Loader impl
void LoaderImpl::Get(GetRequest& request, GetCompleter::Sync& completer) {
  AddCallback(std::move(request.name()), completer.ToAsync());
}

void LoaderImpl::ConnectToDeviceFs(ConnectToDeviceFsRequest& request,
                                   ConnectToDeviceFsCompleter::Sync& completer) {
  if (trusted_) {
    app_->ServeTrustedDeviceFs(
        fidl::ServerEnd<fuchsia_io::Directory>{std::move(request.channel())});
  } else {
    app_->ServeDeviceFs(fidl::ServerEnd<fuchsia_io::Directory>{std::move(request.channel())});
  }
}

void LoaderImpl::ConnectToManifestFs(ConnectToManifestFsRequest& request,
                                     ConnectToManifestFsCompleter::Sync& completer) {
  auto server_end = fidl::ServerEnd<fuchsia_io::Directory>{std::move(request.channel())};
  if (!(request.options() & fuchsia_vulkan_loader::ConnectToManifestOptions::kWaitForIdle) ||
      !app_->HavePendingActions()) {
    app_->ServeManifestFs(std::move(server_end));
    return;
  }

  bool was_waiting_for_callbacks = waiting_for_callbacks();
  connect_manifest_handles_.push_back(std::move(server_end));
  if (waiting_for_callbacks() && !was_waiting_for_callbacks) {
    app_->AddObserver(this);
  }
}

void LoaderImpl::GetSupportedFeatures(GetSupportedFeaturesCompleter::Sync& completer) {
  constexpr fuchsia_vulkan_loader::Features kFeatures =
      fuchsia_vulkan_loader::Features::kConnectToDeviceFs |
      fuchsia_vulkan_loader::Features::kConnectToManifestFs | fuchsia_vulkan_loader::Features::kGet;
  completer.Reply(kFeatures);
}

void LoaderImpl::GetVmexResource(GetVmexResourceCompleter::Sync& completer) {
  if (!app_->allow_lavapipe_icd()) {
    FX_LOGS(ERROR) << "Lavapipe is not allowed, GetVmexResource() shouldn't be called.";
    completer.Reply(
        fit::error(fuchsia_vulkan_loader::GetVmexResourceError::kLavapipeIcdNotAllowed));
    return;
  }

  auto client_end_or = component::Connect<fuchsia_kernel::VmexResource>();
  if (client_end_or.is_error()) {
    FX_LOGS(WARNING) << "Failed to connect to fuchsia.kernel.VmexResource: "
                     << client_end_or.status_string();
    completer.Reply(
        fit::error(fuchsia_vulkan_loader::GetVmexResourceError::kFailedToObtainResource));

    return;
  }

  auto result = fidl::WireCall(*client_end_or)->Get();
  if (!result.ok()) {
    FX_LOGS(WARNING) << "fuchsia.kernel.VmexResource.Get() failed: " << result.error();
    completer.Reply(
        fit::error(fuchsia_vulkan_loader::GetVmexResourceError::kFailedToObtainResource));

    return;
  }

  completer.Reply(fit::ok(std::move(result.value().resource)));
}

void LoaderImpl::AddCallback(std::string name, GetCompleter::Async completer) {
  bool was_waiting_for_callbacks = waiting_for_callbacks();
  std::optional<zx::vmo> vmo = app_->GetMatchingIcd(name);
  if (vmo) {
    completer.Reply(*std::move(vmo));
    return;
  }
  callbacks_.emplace_back(std::move(name), std::move(completer));
  if (waiting_for_callbacks() && !was_waiting_for_callbacks) {
    app_->AddObserver(this);
  }
}
