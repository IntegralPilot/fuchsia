// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/lib/guest_interaction/interactive_guest/interactive_guest_impl.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zxio/zxio.h>
#include <zircon/errors.h>

#include <fbl/unique_fd.h>

#include "lib/zx/time.h"
#include "src/virtualization/lib/guest_interaction/common.h"
#include "src/virtualization/tests/lib/guest_console.h"
#include "src/virtualization/tests/lib/socket.h"

using fuchsia_virtualization::GuestManager;

namespace interactive_guest {

// How long to wait for the guest interaction daemon to become responsive, both whilst issuing the
// bringup command as well as pausing between re-attempting to issue the command.
const zx::duration kInteractionBringupRepeatRate = zx::sec(5);
// The total time to wait whilst attempting to bringup the guest interaction daemon.
const zx::duration kInteractionBringupDeadline = zx::sec(90);

namespace {

fbl::unique_fd GetHostVsockFd(const char* guest_name,
                              fidl::SyncClient<fuchsia_virtualization::Guest>& guest) {
  zx::result vsock_endpoints = fidl::CreateEndpoints<fuchsia_virtualization::HostVsockEndpoint>();
  FX_CHECK(!vsock_endpoints.is_error()) << std::format("[{}] Failed to create guest endpoints: {}",
                                                       guest_name, vsock_endpoints.status_value());

  auto [vsock_client_end, vsock_server_end] = *std::move(vsock_endpoints);
  fidl::SyncClient vsock{std::move(vsock_client_end)};
  fidl::Result<fuchsia_virtualization::Guest::GetHostVsockEndpoint> vsock_res =
      guest->GetHostVsockEndpoint(std::move(vsock_server_end));
  FX_CHECK(vsock_res.is_ok()) << std::format(
      "[{}] Failed to get host vsock endpoint with error: {}", guest_name,
      vsock_res.error_value().FormatDescription());

  fidl::Result<fuchsia_virtualization::HostVsockEndpoint::Connect> connect_res =
      vsock->Connect(/*guest_interaction/common::*/ GUEST_INTERACTION_PORT);
  FX_CHECK(connect_res.is_ok()) << std::format(
      "[{}] Failed to connect to the vsock server with error: {}", guest_name,
      connect_res.error_value().FormatDescription());

  fbl::unique_fd vsock_fd;
  zx_status_t status =
      fdio_fd_create(connect_res->socket().release(), vsock_fd.reset_and_get_address());
  FX_CHECK(status == ZX_OK) << std::format(
      "[{}] Failed to create vsock file descriptor from Zircon socket with error: {}", guest_name,
      status);

  return vsock_fd;
}

// https://fxbug.dev/432092584: We should bootstrap the daemon implicitly on guest bringup.
bool AttemptGuestInteractionDaemonBringup(const char* guest_name,
                                          fidl::SyncClient<fuchsia_virtualization::Guest>& guest) {
  FX_LOGST(INFO, guest_name) << "Attempting serial connection.";
  fidl::Result<fuchsia_virtualization::Guest::GetConsole> get_serial_console_result =
      guest->GetConsole();
  FX_CHECK(get_serial_console_result.is_ok())
      << std::format("[{}] Failed to get guest console with error: {}", guest_name,
                     get_serial_console_result.error_value().FormatDescription());

  GuestConsole serial(std::make_unique<ZxSocket>(std::move(get_serial_console_result->socket())));
  zx_status_t status = serial.Start(zx::time::infinite());
  FX_CHECK(status == ZX_OK) << std::format("[{}] Failed to start serial with error: {}", guest_name,
                                           status);

  FX_LOGST(INFO, guest_name)
      << "Serial connection established, attempting guest_interaction_deamon bringup.";
  constexpr std::string_view kGuestInteractionOutputMarker = "Listening";
  std::stringstream command;
  command << "journalctl -f --no-tail -u guest_interaction_daemon | grep -m1 "
          << kGuestInteractionOutputMarker;
  if (zx_status_t status = serial.RepeatCommandTillSuccess(
          command.str(), "$", std::string(kGuestInteractionOutputMarker),
          zx::deadline_after(kInteractionBringupDeadline), kInteractionBringupRepeatRate);
      status != ZX_OK) {
    FX_PLOGST(ERROR, guest_name, status) << "Failed to bringup the guest_interaction daemon.";
    return false;
  }

  return true;
}

}  // namespace

InteractiveGuestImpl::InteractiveGuestImpl(async::Loop& loop) : loop_(loop) {}

InteractiveGuestImpl::~InteractiveGuestImpl() {
  if (running_guest_) {
    // This represents an unexpected teardown, but since it's still feasible to gracefully
    // recover, we'll only log an error and then initiate the expected shutdown.
    FX_LOGS(ERROR) << "Running guest is present, indicating an improper teardown!";
    DoShutdown();
  }
}

void InteractiveGuestImpl::Start(StartRequest& request, StartCompleter::Sync& completer) {
  auto request_name = request.name().c_str();
  FX_LOGST(INFO, request_name) << "Start requested for an interactive guest.";

  if (running_guest_) {
    FX_LOGST(ERROR, request_name) << "Start requested, but an owned guest is already running.";
    completer.Close(ZX_ERR_ALREADY_BOUND);
    return;
  }

  if (guest_manager_sync_.is_valid()) {
    FX_LOGST(ERROR, request_name) << "Guest manager connection already exists.";
    completer.Close(ZX_ERR_ALREADY_BOUND);
    return;
  }

  zx::result<fidl::ClientEnd<GuestManager>> client_end_result;

  switch (request.guest_type()) {
    case fuchsia_virtualization_guest_interaction::GuestType::kDebian: {
      auto result = component::Connect<fuchsia_virtualization::DebianGuestManager>();
      if (result.is_error()) {
        FX_LOGST(ERROR, request_name)
            << "Failed to connect to DebianGuestManager: " << result.status_string();
        completer.Close(result.status_value());
        return;
      }
      client_end_result = zx::ok(fidl::ClientEnd<GuestManager>(result.value().TakeChannel()));
      break;
    }
    default:
      FX_LOGST(ERROR, request_name)
          << "Unsupported guest type: " << static_cast<uint32_t>(request.guest_type());
      completer.Close(ZX_ERR_NOT_SUPPORTED);
      return;
  }

  guest_manager_sync_ = fidl::SyncClient<GuestManager>(std::move(client_end_result.value()));

  // Attempt to launch the Guest session.
  zx::result guest_endpoints = fidl::CreateEndpoints<fuchsia_virtualization::Guest>();
  FX_CHECK(!guest_endpoints.is_error()) << std::format(
      "[{}] Failed to create guest endpoints: {}", request_name, guest_endpoints.status_value());

  auto [guest_client_end, guest_server_end] = *std::move(guest_endpoints);
  fidl::Result<fuchsia_virtualization::GuestManager::Launch> launch_result =
      guest_manager_sync_->Launch({std::move(request.guest_config()), std::move(guest_server_end)});

  FX_CHECK(launch_result.is_ok()) << std::format("[{}] Failed to launch guest with error: {}",
                                                 request_name,
                                                 launch_result.error_value().FormatDescription());

  fidl::SyncClient<fuchsia_virtualization::Guest> guest =
      fidl::SyncClient<fuchsia_virtualization::Guest>({std::move(guest_client_end)});

  // Bringup the guest interaction daemon, e.g. the Guest gRPC server
  // that fulfills interaction requests over vsock.
  if (!AttemptGuestInteractionDaemonBringup(request_name, guest)) {
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }

  // The host-side vsock, used for client-side gRPC interaction.
  auto vsock_fd = GetHostVsockFd(request_name, guest);
  running_guest_.emplace(std::move(request.name()), std::move(vsock_fd), loop_.dispatcher(),
                         std::move(guest));
  completer.Reply();
}

void InteractiveGuestImpl::Shutdown(ShutdownCompleter::Sync& completer) {
  FX_LOGS(INFO) << "Explicit shutdown via FIDL was requested.";
  DoShutdown();
  completer.Reply();
}

void InteractiveGuestImpl::PutFile(PutFileRequest& request, PutFileCompleter::Sync& completer) {
  FX_CHECK(running_guest_) << "PutFile requested without a running guest!";
  auto host_source = std::move(request.local_file());
  auto guest_dest = request.remote_path();
  running_guest_->PutFile(std::move(host_source), std::move(guest_dest),
                          [completer = completer.ToAsync()](zx_status_t put_result) mutable {
                            completer.Reply(put_result);
                          });
}

void InteractiveGuestImpl::GetFile(GetFileRequest& request, GetFileCompleter::Sync& completer) {
  FX_CHECK(running_guest_) << "GetFile requested without a running guest!";
  auto guest_source = request.remote_path();
  auto host_dest = std::move(request.local_file());
  running_guest_->GetFile(std::move(guest_source), std::move(host_dest),
                          [completer = completer.ToAsync()](zx_status_t put_result) mutable {
                            completer.Reply(put_result);
                          });
}

void InteractiveGuestImpl::ExecuteCommand(ExecuteCommandRequest& request,
                                          ExecuteCommandCompleter::Sync& completer) {
  FX_CHECK(running_guest_) << "ExecuteCommand requested without a running guest!";
  std::map<std::string, std::string> env_variables;
  for (const auto& var : request.env()) {
    env_variables.insert({var.key(), var.value()});
  }
  auto listener = std::move(request.command_listener());

  running_guest_->Execute(request.command(), env_variables, std::move(request.stdin_()),
                          std::move(request.stdout_()), std::move(request.stderr_()),
                          std::move(listener));
}

void InteractiveGuestImpl::DoShutdown() {
  FX_CHECK(running_guest_) << "Shutdown requested without a running guest!";
  running_guest_->Shutdown();
  guest_manager_sync_->ForceShutdown();
}

}  // namespace interactive_guest
