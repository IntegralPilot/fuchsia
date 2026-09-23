// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_LOADER_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_LOADER_H_

#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <list>
#include <string>
#include <vector>

#include "src/graphics/bin/vulkan_loader/app.h"

// Implements the vulkan loader's Loader service which provides the client
// driver portion to the loader as a VMO.
class LoaderImpl final : public fidl::Server<fuchsia_vulkan_loader::Loader>,
                         public LoaderApp::Observer {
 public:
  // Add a handler for this protocol to |outgoing_dir|. Any connections made to the protocol will
  // create a new loader instance.  The loader instance will be alive as long as |dispatcher| has
  // tasks with active connections.  The owner of an instance of this class must ensure that
  // |app| and |dispatcher| outlive the instance.
  static zx::result<> Add(component::OutgoingDirectory& outgoing_dir, LoaderApp* app,
                          async_dispatcher_t* dispatcher);

  ~LoaderImpl() final;

 private:
  explicit LoaderImpl(LoaderApp* app, bool trusted) : app_(app), trusted_(trusted) {}

  // LoaderApp::Observer implementation.
  void OnIcdListChanged(LoaderApp* app) override;

  // fidl::Server<fuchsia_vulkan_loader::Loader> implementation.
  void Get(GetRequest& request, GetCompleter::Sync& completer) override;
  void ConnectToDeviceFs(ConnectToDeviceFsRequest& request,
                         ConnectToDeviceFsCompleter::Sync& completer) override;
  void ConnectToManifestFs(ConnectToManifestFsRequest& request,
                           ConnectToManifestFsCompleter::Sync& completer) override;
  void GetSupportedFeatures(GetSupportedFeaturesCompleter::Sync& completer) override;
  void GetVmexResource(GetVmexResourceCompleter::Sync& completer) override;

  void AddCallback(std::string name, GetCompleter::Async completer);

  bool waiting_for_callbacks() const {
    return !callbacks_.empty() || !connect_manifest_handles_.empty();
  }

  LoaderApp* app_;
  bool trusted_;
  std::list<std::pair<std::string, GetCompleter::Async>> callbacks_;
  std::vector<fidl::ServerEnd<fuchsia_io::Directory>> connect_manifest_handles_;
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_LOADER_H_
