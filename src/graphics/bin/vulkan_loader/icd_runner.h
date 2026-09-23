// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_BIN_VULKAN_LOADER_ICD_RUNNER_H_
#define SRC_GRAPHICS_BIN_VULKAN_LOADER_ICD_RUNNER_H_

#include <fidl/fuchsia.component.runner/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

class ComponentControllerImpl final
    : public fidl::Server<fuchsia_component_runner::ComponentController> {
 public:
  // Binds `controller` to a new component controller using the given `outgoing_dir` and
  // `pkg_directory`. On error, `controller` will be closed with an epitaph.
  static zx::result<std::unique_ptr<fidl::Server<fuchsia_component_runner::ComponentController>>>
  Bind(async_dispatcher_t* dispatcher,
       fidl::ServerEnd<fuchsia_component_runner::ComponentController> controller,
       fidl::ServerEnd<fuchsia_io::Directory> outgoing_dir,
       fidl::ClientEnd<fuchsia_io::Directory> pkg_directory);

 private:
  explicit ComponentControllerImpl(async_dispatcher_t* dispatcher);
  void Stop(StopCompleter::Sync& completer) override { binding_->Close(ZX_OK); }
  void Kill(KillCompleter::Sync& completer) override { binding_->Close(ZX_OK); }
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentController>,
      fidl::UnknownMethodCompleter::Sync&) override {}

  fs::SynchronousVfs vfs_;
  std::optional<fidl::ServerBindingRef<fuchsia_component_runner::ComponentController>> binding_;
};

// This implements the icd_runner interface.
class IcdRunnerImpl : public fidl::Server<fuchsia_component_runner::ComponentRunner> {
 public:
  explicit IcdRunnerImpl(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}
  static zx::result<> Add(std::unique_ptr<IcdRunnerImpl> component_runner,
                          component::OutgoingDirectory& outgoing);

  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_component_runner::ComponentRunner>,
                             fidl::UnknownMethodCompleter::Sync&) override {}

 private:
  async_dispatcher_t* const dispatcher_;
  std::unique_ptr<fidl::Server<fuchsia_component_runner::ComponentController>> controller_server_;
};

#endif  // SRC_GRAPHICS_BIN_VULKAN_LOADER_ICD_RUNNER_H_
