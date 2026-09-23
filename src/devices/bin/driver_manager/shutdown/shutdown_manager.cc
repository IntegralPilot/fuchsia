// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "shutdown_manager.h"

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/common_types_format.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/mexec_boot/mexec_boot.h>
#include <zircon/processargs.h>  // PA_LIFECYCLE
#include <zircon/syscalls/system.h>

#include <src/devices/lib/log/log.h>

namespace driver_manager {
namespace {

SystemPowerState GetSystemPowerState() {
  zx::result client = component::Connect<fuchsia_system_state::SystemStateTransition>();
  if (client.is_error()) {
    fdf_log::error("Failed to connect to StateStateTransition: {}, falling back to default",
                   client.status_string());
    return SystemPowerState::kReboot;
  }

  fidl::Result result = fidl::Call(*client)->GetTerminationSystemState();
  if (result.is_error()) {
    fdf_log::error("Failed to get termination system state: {}, falling back to default",
                   result.error_value().FormatDescription());
    return SystemPowerState::kReboot;
  }

  return result->state();
}

template <typename SyncCompleter>
fit::callback<void(zx_status_t)> ToCallback(SyncCompleter& completer) {
  return [completer = completer.ToAsync()](zx_status_t status) mutable { completer.Close(status); };
}

// Get the power resource from the root resource service. Not receiving the
// startup handle is logged, but not fatal.  In test environments, it would not
// be present.
zx::result<zx::resource> get_power_resource() {
  zx::result client_end = component::Connect<fuchsia_kernel::PowerResource>();
  if (client_end.is_error()) {
    return client_end.take_error();
  }
  fidl::WireResult result = fidl::WireCall(client_end.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

// Get the mexec resource from the mexec resource service. Not receiving the
// startup handle is logged, but not fatal.  In test environments, it would not
// be present.
zx::result<zx::resource> get_mexec_resource() {
  zx::result client_end = component::Connect<fuchsia_kernel::MexecResource>();
  if (client_end.is_error()) {
    return client_end.take_error();
  }
  fidl::WireResult result = fidl::WireCall(client_end.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

}  // anonymous namespace

ShutdownManager::ShutdownManager(NodeRemover* node_remover, async_dispatcher_t* dispatcher)
    : node_remover_(node_remover),
      devfs_lifecycle_(
          [this](fit::callback<void(zx_status_t)> cb) { SignalBootShutdown(std::move(cb)); }),
      devfs_with_pkg_lifecycle_(
          [this](fit::callback<void(zx_status_t)> cb) { SignalPackageShutdown(std::move(cb)); }),
      dispatcher_(dispatcher) {
  if (zx::result power_resource = get_power_resource(); power_resource.is_error()) {
    fdf_log::info("Failed to get root resource, assuming test environment and continuing ({})",
                  power_resource.status_string());
  } else {
    power_resource_ = std::move(power_resource.value());
  }
  zx::result log_flush = component::Connect<fuchsia_diagnostics::LogFlusher>();
  ZX_ASSERT(log_flush.is_ok());
  log_flush_ = std::move(log_flush.value());
  if (zx::result mexec_resource = get_mexec_resource(); mexec_resource.is_error()) {
    fdf_log::info("Failed to get mexec resource, assuming test environment and continuing ({})",
                  mexec_resource.status_string());
  } else {
    mexec_resource_ = std::move(mexec_resource.value());
  }
  node_remover_->SetOnRemovalTimeoutCallback([&]() {
    fdf_log::info("Driver timed out during shutdown, issuing syscall to reboot/shutdown");
    SystemExecute();
  });
}

// Invoked when the channel is closed or on any binding-related error.
// If we were not shutting down, we should start shutting down, because
// we no longer have a way to get signals to shutdown the system.
void ShutdownManager::OnUnbound(const char* connection, fidl::UnbindInfo info) {
  if (info.is_user_initiated()) {
    fdf_log::debug("{} connection to ShutdownManager got unbound: {}", connection,
                   info.FormatDescription());
  } else {
    fdf_log::error("{} connection to ShutdownManager got unbound: {}", connection,
                   info.FormatDescription());
  }
  SignalBootShutdown(nullptr);
}

void ShutdownManager::Publish(component::OutgoingDirectory& outgoing) {
  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_process_lifecycle::Lifecycle>(
      lifecycle_bindings_.CreateHandler(&devfs_lifecycle_, dispatcher_,
                                        fidl::kIgnoreBindingClosure),
      "fuchsia.device.fs.lifecycle.Lifecycle");
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  result = outgoing.AddUnmanagedProtocol<fuchsia_process_lifecycle::Lifecycle>(
      lifecycle_bindings_.CreateHandler(&devfs_with_pkg_lifecycle_, dispatcher_,
                                        fidl::kIgnoreBindingClosure),
      "fuchsia.device.fs.with.pkg.lifecycle.Lifecycle");
  ZX_ASSERT_MSG(result.is_ok(), "%s", result.status_string());

  // Bind to lifecycle server
  fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle_server(
      zx::channel(zx_take_startup_handle(PA_LIFECYCLE)));

  if (lifecycle_server.is_valid()) {
    lifecycle_bindings_.AddBinding(dispatcher_, std::move(lifecycle_server), this,
                                   [](ShutdownManager* server, fidl::UnbindInfo info) {
                                     server->OnUnbound("Lifecycle", info);
                                   });
  } else {
    fdf_log::info(
        "No valid handle found for lifecycle events, assuming test environment and continuing");
  }
}

void ShutdownManager::OnPackageShutdownComplete() {
  fdf_log::info("Package shutdown complete");
  ZX_ASSERT(shutdown_state_ == State::kPackageStopping);
  shutdown_state_ = State::kPackageStopped;
  for (auto& callback : package_shutdown_complete_callbacks_) {
    callback(ZX_OK);
  }
  package_shutdown_complete_callbacks_.clear();
  if (received_boot_shutdown_signal_) {
    shutdown_state_ = State::kBootStopping;
    // In the middle of package shutdown we were told to shutdown everything.
    node_remover_->ShutdownAllDrivers(
        fit::bind_member(this, &ShutdownManager::OnBootShutdownComplete));
  }
}

void ShutdownManager::OnBootShutdownComplete() {
  ZX_ASSERT(shutdown_state_ == State::kBootStopping);
  shutdown_state_ = State::kStopped;
  SystemExecute();
  for (auto& callback : boot_shutdown_complete_callbacks_) {
    callback(ZX_OK);
  }
  boot_shutdown_complete_callbacks_.clear();
}

void ShutdownManager::AcquireShutdownLeases(fit::callback<void()> callback) {
  if (lease_state_ == LeaseState::kAcquired) {
    callback();
    return;
  }

  shutdown_lease_acquired_callbacks_.emplace_back(std::move(callback));

  if (lease_state_ == LeaseState::kRequested) {
    return;
  }

  lease_state_ = LeaseState::kRequested;
  node_remover_->LeaseAllDriversForShutdown([this]() {
    lease_state_ = LeaseState::kAcquired;
    auto callbacks = std::move(shutdown_lease_acquired_callbacks_);
    for (auto& cb : callbacks) {
      cb();
    }
  });
}

void ShutdownManager::SignalPackageShutdown(fit::callback<void(zx_status_t)> cb) {
  // Switch our logs to go to debuglog to ensure they are flushed and available in the crashlog.
  driver_logger::GetLogger().SwitchToStdout();

  // Expected case: we get the call during kPackageStopping, or right before.
  // Store the completer for when we finish.
  // Otherwise, we already finished package shutdown or we have already jumped
  // to doing a full shutdown. Notify the callback immediately.
  if (shutdown_state_ != State::kRunning && shutdown_state_ != State::kPackageStopping) {
    cb(ZX_OK);
    return;
  }

  package_shutdown_complete_callbacks_.emplace_back(std::move(cb));

  // If the state is already stopping for package drivers, then we don't need to do anything.
  if (shutdown_state_ == State::kPackageStopping) {
    return;
  }

  shutdown_state_ = State::kPackageStopping;
  AcquireShutdownLeases([this]() {
    node_remover_->ShutdownPkgDrivers(
        fit::bind_member(this, &ShutdownManager::OnPackageShutdownComplete));
  });
}

void ShutdownManager::Stop(StopCompleter::Sync& completer) {
  lifecycle_stop_ = true;
  SignalBootShutdown(ToCallback(completer));
}

void ShutdownManager::SignalBootShutdown(fit::callback<void(zx_status_t)> cb) {
  if (cb) {
    if (shutdown_state_ == State::kStopped) {
      cb(ZX_OK);
    } else {
      boot_shutdown_complete_callbacks_.emplace_back(std::move(cb));
    }
  }
  received_boot_shutdown_signal_ = true;
  // Expected case: we get the call while running, or after we shutdown the package drivers.
  if (shutdown_state_ == State::kRunning || shutdown_state_ == State::kPackageStopped) {
    shutdown_state_ = State::kBootStopping;
    node_remover_->ShutdownAllDrivers(
        fit::bind_member(this, &ShutdownManager::OnBootShutdownComplete));
  } else if (shutdown_state_ == State::kBootStopping) {
    fdf_log::error("SignalBootShutdown() called during shutdown.");
  }
}

void ShutdownManager::SystemExecute() {
  auto shutdown_system_state = GetSystemPowerState();
  fdf_log::info("Suspend fallback with flags {}", shutdown_system_state);
  const char* what = "zx_system_powerctl";
  zx_status_t status = ZX_OK;
  if (!mexec_resource_.is_valid() || !power_resource_.is_valid()) {
    fdf_log::warn("Invalid Power/mexec resources. Assuming test.");
    if (lifecycle_stop_) {
      exit(0);
    }
    return;
  }

  fdf_log::info("Flushing logs.");
  if (log_flush_.is_valid()) {
    std::ignore = fidl::WireCall(log_flush_)->WaitUntilFlushed();
  }

  fdf_log::info("Executing powerctl.");
  switch (shutdown_system_state) {
    case SystemPowerState::kReboot:
      status = zx_system_powerctl(power_resource_.get(), ZX_SYSTEM_POWERCTL_REBOOT, nullptr);
      break;
    case SystemPowerState::kRebootBootloader:
      status =
          zx_system_powerctl(power_resource_.get(), ZX_SYSTEM_POWERCTL_REBOOT_BOOTLOADER, nullptr);
      break;
    case SystemPowerState::kRebootRecovery:
      status =
          zx_system_powerctl(power_resource_.get(), ZX_SYSTEM_POWERCTL_REBOOT_RECOVERY, nullptr);
      break;
    case SystemPowerState::kRebootKernelInitiated:
      status = zx_system_powerctl(power_resource_.get(),
                                  ZX_SYSTEM_POWERCTL_ACK_KERNEL_INITIATED_REBOOT, nullptr);
      if (status == ZX_OK) {
        // Sleep indefinitely to give the kernel a chance to reboot the system. This results in a
        // cleaner reboot because it prevents driver_manager from exiting. If driver_manager exits
        // the other parts of the system exit, bringing down the root job. Crashing the root job
        // is innocuous at this point, but we try to avoid it to reduce log noise and possible
        // confusion.
        while (true) {
          sleep(5 * 60);
          // We really shouldn't still be running, so log if we are. Use `printf`
          // because messages from the devices are probably only visible over
          // serial at this point.
          printf("driver_manager: unexpectedly still running after successful reboot syscall\n");
        }
      }
      break;
    case SystemPowerState::kPoweroff:
      status = zx_system_powerctl(power_resource_.get(), ZX_SYSTEM_POWERCTL_SHUTDOWN, nullptr);
      break;
    case SystemPowerState::kMexec: {
      fdf_log::info("About to mexec...");
      status = mexec_boot(mexec_resource_.get());
      what = "zx_system_mexec";
      break;
    }
    case SystemPowerState::kFullyOn:
    case SystemPowerState::kSuspendRam:
      fdf_log::error("Unexpected shutdown state requested: {}", shutdown_system_state);
      break;
  }

  // This is mainly for test dev:
  if (lifecycle_stop_) {
    fdf_log::info("Exiting driver manager gracefully");
    // TODO(fxb:52627) This event handler should teardown devices and driver hosts
    // properly for system state transitions where driver manager needs to go down.
    // Exiting like so, will not run all the destructors and clean things up properly.
    // Instead the main devcoordinator loop should be quit.
    exit(0);
  }

  // Warning - and not an error - as a large number of tests unfortunately rely
  // on this syscall actually failing.
  fdf_log::warn("{}: {}", what, zx_status_get_string(status));
}

}  // namespace driver_manager
