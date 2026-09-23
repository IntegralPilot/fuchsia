// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/test.placeholders/cpp/test_base.h>
#include <fuchsia/vulkan/loader/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/service.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"

namespace {

class EchoImpl final : public fidl::testing::TestBase<test_placeholders::Echo> {
 protected:
  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply({{.response = request.value()}});
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ZX_PANIC("Not implemented!");
  }
};

// This is a fake Vulkan loader service that implements just enough for the libvulkan.so to work.
class LoaderImpl final : public fuchsia::vulkan::loader::Loader {
 public:
  explicit LoaderImpl() : device_fs_(std::make_unique<vfs::PseudoDir>()) {
    auto echo_service =
        std::make_unique<vfs::Service>(echo_instance_.bind_handler(async_get_default_dispatcher()));
    device_fs_->AddEntry("echo", std::move(echo_service));
  }

  ~LoaderImpl() final = default;

  // Adds a binding for fuchsia::vulkan::loader::Loader to |outgoing|
  void Add(const std::shared_ptr<sys::OutgoingDirectory>& outgoing) {
    outgoing->AddPublicService(fidl::InterfaceRequestHandler<fuchsia::vulkan::loader::Loader>(
        [this](fidl::InterfaceRequest<fuchsia::vulkan::loader::Loader> request) {
          bindings_.AddBinding(this, std::move(request), nullptr);
        }));
  }

 private:
  // fuchsia::vulkan::loader::Loader impl
  void Get(std::string name, GetCallback callback) override {
    // libvulkan_fake.so is located inside this package.
    std::string load_path = "/pkg/lib/" + name;
    int fd;
    zx_status_t status = fdio_open3_fd(
        load_path.c_str(),
        static_cast<uint64_t>(fuchsia::io::PERM_READABLE | fuchsia::io::PERM_EXECUTABLE), &fd);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Could not open path " << load_path << ":" << status;
      callback({});
      return;
    }
    zx::vmo vmo;
    status = fdio_get_vmo_exec(fd, vmo.reset_and_get_address());
    close(fd);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Could not clone vmo exec: " << status;
    }
    callback(std::move(vmo));
  }

  void ConnectToDeviceFs(zx::channel channel) override {
    // The fake libvulkan implementation tries to connect to the echo protocol at "echo"
    ZX_ASSERT(device_fs_->Serve(fuchsia_io::wire::kPermReadable,
                                fidl::ServerEnd<fuchsia_io::Directory>(std::move(channel))) ==
              ZX_OK);
  }

  void GetSupportedFeatures(GetSupportedFeaturesCallback callback) override {
    fuchsia::vulkan::loader::Features features =
        fuchsia::vulkan::loader::Features::CONNECT_TO_DEVICE_FS |
        fuchsia::vulkan::loader::Features::GET |
        fuchsia::vulkan::loader::Features::CONNECT_TO_MANIFEST_FS;

    callback(features);
  }

  void GetVmexResource(GetVmexResourceCallback callback) override {
    // Can't pass the enum value directly to `WithErr()` because it expects a "non-const &&"" arg.
    auto err = fuchsia::vulkan::loader::GetVmexResourceError::LAVAPIPE_ICD_NOT_ALLOWED;
    callback(fuchsia::vulkan::loader::Loader_GetVmexResource_Result::WithErr(std::move(err)));
  }

  void ConnectToManifestFs(fuchsia::vulkan::loader::ConnectToManifestOptions options,
                           zx::channel channel) override {
    fdio_open3("/pkg/data/manifest", static_cast<uint64_t>(fuchsia::io::PERM_READABLE),
               channel.release());
  }

  EchoImpl echo_instance_;
  std::unique_ptr<vfs::PseudoDir> device_fs_;
  fidl::BindingSet<fuchsia::vulkan::loader::Loader> bindings_;
};

}  // namespace

int main(int argc, const char* const* argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  fxl::SetLogSettingsFromCommandLine(command_line);
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  LoaderImpl loader_impl;
  loader_impl.Add(context->outgoing());
  loop.Run();
  return 0;
}
