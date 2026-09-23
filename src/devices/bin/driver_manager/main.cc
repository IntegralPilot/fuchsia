// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.driver.index/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/io.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/component/cpp/service.h>
#include <lib/zx/event.h>
#include <lib/zx/port.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmo.h>
#include <malloc.h>
#include <threads.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/policy.h>
#include <zircon/types.h>

#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>

#include <fbl/unique_fd.h>

#include "src/devices/bin/driver_manager/devfs/devfs.h"
#include "src/devices/bin/driver_manager/driver_development/driver_development_service.h"
#include "src/devices/bin/driver_manager/driver_host_loader_service.h"
#include "src/devices/bin/driver_manager/driver_manager_config.h"
#include "src/devices/bin/driver_manager/driver_runner.h"
#include "src/devices/bin/driver_manager/firmware_crash/firmware_crash_service.h"
#include "src/devices/bin/driver_manager/offer_injection.h"
#include "src/devices/bin/driver_manager/shutdown/shutdown_manager.h"
#include "src/devices/lib/log/log.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"
#include "src/sys/lib/stdout-to-debuglog/cpp/stdout-to-debuglog.h"

namespace fio = fuchsia_io;

namespace {

void InitLogging() {
  char process_name[ZX_MAX_NAME_LEN] = "";

  zx_status_t name_status =
      zx::process::self()->get_property(ZX_PROP_NAME, process_name, sizeof(process_name));
  if (name_status != ZX_OK) {
    process_name[0] = '\0';
  }

  const char* tags[] = {process_name};
  fdf_log::InitGlobalLogger(tags, FUCHSIA_LOG_INFO);
}

}  // namespace

int main(int argc, char** argv) {
  zx_status_t status = StdoutToDebuglog::Init();
  if (status != ZX_OK) {
    fdf_log::info(
        "Failed to redirect stdout to debuglog, assuming test environment and continuing");
  }

  auto config = driver_manager_config::Config::TakeFromStartupHandle();

  InitLogging();

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto outgoing = component::OutgoingDirectory(loop.dispatcher());
  inspect::ComponentInspector inspector{loop.dispatcher(), {}};
  inspector.inspector().CreateStatsNode();

  // Launch DriverRunner for DFv2 drivers.
  auto realm_result = component::Connect<fuchsia_component::Realm>();
  if (realm_result.is_error()) {
    return realm_result.error_value();
  }

  auto introspector_result = component::Connect<fuchsia_component::Introspector>();
  if (introspector_result.is_error()) {
    return introspector_result.error_value();
  }

  fidl::ClientEnd<fuchsia_power_broker::Topology> topology_client;
  std::optional<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>> cpu_element_mgr;
  if (config.power_suspend_enabled()) {
    zx::result<fidl::ClientEnd<fuchsia_power_broker::Topology>> topology_result =
        component::Connect<fuchsia_power_broker::Topology>();
    if (topology_result.is_error()) {
      return topology_result.error_value();
    }
    topology_client = std::move(*topology_result);

    zx::result<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>> cpu_element_result =
        component::Connect<fuchsia_power_system::CpuElementManager>();
    if (cpu_element_result.is_error()) {
      return cpu_element_result.error_value();
    }
    cpu_element_mgr.emplace(std::move(cpu_element_result.value()));
  }

  std::optional<fidl::ClientEnd<fuchsia_hardware_power_statecontrol::Admin>> statecontrol_admin;
  zx::result statecontrol_admin_result =
      component::Connect<fuchsia_hardware_power_statecontrol::Admin>();
  if (statecontrol_admin_result.is_ok()) {
    statecontrol_admin.emplace(std::move(statecontrol_admin_result.value()));
  } else {
    fdf_log::warn("Failed to connect to fuchsia.hardware.power.statecontrol.Admin: {}",
                  statecontrol_admin_result.status_string());
  }

  auto capability_store_result = component::Connect<fuchsia_component_sandbox::CapabilityStore>();
  if (capability_store_result.is_error()) {
    return capability_store_result.error_value();
  }

  auto driver_index_result = component::Connect<fuchsia_driver_index::DriverIndex>();
  if (driver_index_result.is_error()) {
    fdf_log::error("Failed to connect to driver_index: {}", driver_index_result);
    return driver_index_result.error_value();
  }
  fbl::unique_fd lib_fd;
  constexpr uint64_t kOpenFlags = static_cast<uint64_t>(
      fio::wire::Flags::kProtocolDirectory | fio::wire::kPermReadable | fio::wire::kPermExecutable);
  if (zx_status_t status = fdio_open3_fd("/pkg/lib/", kOpenFlags, lib_fd.reset_and_get_address());
      status != ZX_OK) {
    fdf_log::error("Failed to open /pkg/lib/ : {}", zx_status_get_string(status));
    return status;
  }
  // The loader needs its own thread because DriverManager makes synchronous calls to the
  // DriverHosts, which make synchronous calls to load their shared libraries.
  async::Loop loader_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  loader_loop.StartThread("loader-loop");
  auto loader_service =
      driver_manager::DriverHostLoaderService::Create(loader_loop.dispatcher(), std::move(lib_fd));
  std::shared_ptr<driver_manager::DriverRunner> driver_runner =
      std::make_shared<driver_manager::DriverRunner>(
          std::move(realm_result.value()), std::move(introspector_result.value()),
          std::move(capability_store_result.value()), std::move(driver_index_result.value()),
          inspector,
          [loader_service]() -> zx::result<fidl::ClientEnd<fuchsia_ldsvc::Loader>> {
            zx::result client = loader_service->Connect();
            if (client.is_error()) {
              return client.take_error();
            }
    // TODO(https://fxbug.dev/42076026): Find a better way to set this config.
#ifdef DRIVERHOST_LDSVC_CONFIG
            // Inform the loader service to look for libraries of the right variant.
            fidl::WireResult result =
                fidl::WireCall(client.value())->Config(DRIVERHOST_LDSVC_CONFIG);
            if (!result.ok()) {
              return zx::error(result.status());
            }
            if (result->rv != ZX_OK) {
              return zx::error(result->rv);
            }
#endif
            return client;
          },
          loop.dispatcher(), config.enable_test_shutdown_delays(),
          driver_manager::OfferInjector{{
              .power_inject_offer = config.power_inject_offer(),
              .power_suspend_enabled = config.power_suspend_enabled(),
          }},
          std::move(topology_client), std::nullopt, std::move(cpu_element_mgr),
          config.wait_for_suspending_token(), std::move(statecontrol_admin));
  // Setup devfs.
  std::shared_ptr<driver_manager::Devfs> devfs;
  driver_runner->root_node()->SetupDevfsForRootNode(devfs);
  driver_runner->PublishComponentRunner(outgoing);
  driver_runner->power_manager()->PublishCpuElementManager(outgoing);
  driver_runner->StartDevfsDriver(devfs);
  driver_runner->power_manager()->FetchCpuToken();

  // If power management is enabled, but we're not supposed to wait for a storage token from a
  // storage driver, initialize the storage power element immediately.
  //
  // TODO(https://fxbug.dev/479254641) Be lazier, only initialize storage once we hit drivers
  // that need it.
  if (config.power_suspend_enabled() && !config.wait_for_suspending_token()) {
    driver_runner->power_manager()->CreateStoragePowerElement(
        fuchsia_power_broker::DependencyToken(zx::event()),
        static_cast<fuchsia_power_broker::PowerLevel>(1), []() {});
  }

  // Find and load v2 Drivers.
  fdf_log::info("Starting DriverRunner with root driver URL: {}", config.root_driver());
  if (auto start = driver_runner->StartRootDriver(config.root_driver()); start.is_error()) {
    return start.error_value();
  }

  driver_manager::DriverDevelopmentService driver_development_service(*driver_runner,
                                                                      loop.dispatcher());
  driver_development_service.Publish(outgoing);
  driver_runner->TryBindAllAvailable();

  driver_manager::FirmwareCrashService firmware_crash_service(loop.dispatcher());
  firmware_crash_service.Publish(outgoing);

  driver_manager::ShutdownManager shutdown_manager(driver_runner.get(), loop.dispatcher());
  shutdown_manager.Publish(outgoing);

  fs::SynchronousVfs vfs(loop.dispatcher());

  // Add the devfs folder to the tree:
  {
    zx::result devfs_client = devfs->Connect(vfs);
    ZX_ASSERT_MSG(devfs_client.is_ok(), "%s", devfs_client.status_string());
    const zx::result result = outgoing.AddDirectory(std::move(devfs_client.value()), "dev");
    ZX_ASSERT(result.is_ok());
  }

  {
    const zx::result result = outgoing.ServeFromStartupInfo();
    ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());
  }

  async::PostTask(loop.dispatcher(), [] { fdf_log::info("driver_manager main loop is running"); });

  driver_runner->WaitForBootup([dispatcher = loop.dispatcher()]() {
    // Once bootup is complete we can free some heap memory.
    async::PostTask(dispatcher, []() { mallopt(M_PURGE, 0); });
  });

  status = loop.Run();
  fdf_log::error("Driver Manager exited unexpectedly: {}", zx_status_get_string(status));
  return status;
}
