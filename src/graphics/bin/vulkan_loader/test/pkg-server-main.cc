// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/test_base.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process.lifecycle/cpp/test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

class FakeMagmaDevice : public fidl::testing::TestBase<fuchsia_gpu_magma::CombinedDevice> {
 public:
  explicit FakeMagmaDevice(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void CloseAll() { bindings_.CloseAll(ZX_OK); }

  auto ProtocolConnector() {
    return [this](fidl::ServerEnd<fuchsia_gpu_magma::CombinedDevice> server_end) -> zx_status_t {
      bindings_.AddBinding(dispatcher_, std::move(server_end), this, fidl::kIgnoreBindingClosure);
      return ZX_OK;
    };
  }

 private:
  void GetIcdList(GetIcdListCompleter::Sync& completer) override {
    fuchsia_gpu_magma::IcdInfo info;
    info.component_url() =
        "fuchsia-pkg://fuchsia.com/vulkan_loader_tests#meta/test_vulkan_driver.cm";
    info.flags() = fuchsia_gpu_magma::IcdFlags::kSupportsVulkan;
    completer.Reply(std::vector{std::move(info)});
  }

  void Query(QueryRequest& request, QueryCompleter::Sync& completer) override {
    completer.Reply(fit::ok(fuchsia_gpu_magma::DeviceQueryResponse::WithSimpleResult(5)));
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ZX_PANIC("unexpected call to %s", name.c_str());
  }

  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_gpu_magma::CombinedDevice> bindings_;
};
class LifecycleHandler : public fidl::testing::TestBase<fuchsia_process_lifecycle::Lifecycle> {
 public:
  static LifecycleHandler Create(async::Loop* loop) {
    fidl::ServerEnd server_end = fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>{
        zx::channel(zx_take_startup_handle(PA_LIFECYCLE))};
    ZX_ASSERT_MSG(server_end.is_valid(), "Invalid handle for PA_LIFECYCLE!");
    return LifecycleHandler(loop, std::move(server_end));
  }

 private:
  explicit LifecycleHandler(async::Loop* loop,
                            fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> server_end)
      : loop_(loop),
        binding_(loop->dispatcher(), std::move(server_end), this, fidl::kIgnoreBindingClosure) {}

  void Stop(StopCompleter::Sync& completer) override {
    loop_->Quit();
    binding_.Close(ZX_OK);
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ZX_PANIC("unexpected call to %s", name.c_str());
  }

  async::Loop* loop_;
  fidl::ServerBinding<fuchsia_process_lifecycle::Lifecycle> binding_;
};

int main(int argc, const char* const* argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  LifecycleHandler handler = LifecycleHandler::Create(&loop);
  fxl::SetLogSettingsFromCommandLine(fxl::CommandLineFromArgcArgv(argc, argv));

  fs::SynchronousVfs vfs(loop.dispatcher());
  auto root = fbl::MakeRefCounted<fs::PseudoDir>();

  FakeMagmaDevice magma_device(loop.dispatcher());
  {
    // Add a svc directory that the loader can watch for devices to be added.
    auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    zx_status_t status = root->AddEntry("svc", svc_dir);
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to add /svc: %s", zx_status_get_string(status));

    auto magma_service_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    svc_dir->AddEntry("fuchsia.gpu.magma.Service", magma_service_dir);

    auto magma_dependency_injection_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    svc_dir->AddEntry("fuchsia.gpu.magma.DependencyInjectionService",
                      magma_dependency_injection_dir);

    auto service_instance_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    magma_service_dir->AddEntry("some-instance-name", service_instance_dir);

    status = service_instance_dir->AddEntry(
        "device", fbl::MakeRefCounted<fs::Service>(magma_device.ProtocolConnector()));
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to add service: %s", zx_status_get_string(status));

    auto trusted_magma_service_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    svc_dir->AddEntry("fuchsia.gpu.magma.TrustedService", trusted_magma_service_dir);

    auto trusted_service_instance_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    trusted_magma_service_dir->AddEntry("some-instance-name", trusted_service_instance_dir);

    status = trusted_service_instance_dir->AddEntry(
        "device", fbl::MakeRefCounted<fs::Service>(magma_device.ProtocolConnector()));
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to add trusted service: %s",
                  zx_status_get_string(status));
  }

  auto dev_gpu_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  root->AddEntry("dev-gpu", dev_gpu_dir);

  auto dev_goldfish_pipe_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  zx_status_t status = root->AddEntry("dev-goldfish-pipe", dev_goldfish_pipe_dir);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to add goldfish pipe: %s", zx_status_get_string(status));

  auto dev_goldfish_sync_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  status = dev_goldfish_sync_dir->AddEntry(
      "000", fbl::MakeRefCounted<fs::Service>([](zx::channel channel) { return ZX_OK; }));
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to add goldfish sync entry: %s",
                zx_status_get_string(status));
  status = root->AddEntry("dev-goldfish-sync", dev_goldfish_sync_dir);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to add goldfish sync: %s", zx_status_get_string(status));

  auto dev_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  status = root->AddEntry("dev", dev_gpu_dir);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to add /dev: %s", zx_status_get_string(status));

  fidl::ServerEnd<fuchsia_io::Directory> dir_request{
      zx::channel(zx_take_startup_handle(PA_DIRECTORY_REQUEST))};
  status = vfs.ServeDirectory(root, std::move(dir_request), fuchsia_io::kRStarDir);
  ZX_ASSERT_MSG(status == ZX_OK, "Failed to serve outgoing: %s", zx_status_get_string(status));

  loop.Run();
  return 0;
}
