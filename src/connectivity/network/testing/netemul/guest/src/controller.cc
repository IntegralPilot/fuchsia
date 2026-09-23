// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/testing/netemul/guest/src/controller.h"

#include <optional>

#include "fidl/fuchsia.netemul.guest/cpp/wire_types.h"
#include "lib/zx/time.h"
#include "src/lib/fxl/macros.h"
#include "src/virtualization/tests/lib/guest_console.h"

ControllerImpl::ControllerImpl(
    async::Loop& loop, fidl::SyncClient<fuchsia_virtualization::DebianGuestManager> guest_manager)
    : loop_(loop), guest_manager_(std::move(guest_manager)), guest_session_(std::nullopt) {}

void ControllerImpl::CreateGuest(CreateGuestRequestView request,
                                 CreateGuestCompleter::Sync& completer) {
  std::string name(request->name.get());

  if (HasActiveGuest()) {
    FX_LOGST(ERROR, name.c_str()) << "CreateGuest called but guest already exists";
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kAlreadyRunning);
    return;
  }

  // Disable unnecessary virtio devices.
  fuchsia_virtualization::GuestConfig cfg;
  cfg.virtio_gpu(false);
  cfg.virtio_sound(false);
  cfg.virtio_rng(false);
  cfg.virtio_balloon(false);

  if (request->mac) {
    std::array<uint8_t, 6> mac;
    std::copy(std::begin(request->mac->octets), std::end(request->mac->octets), std::begin(mac));
    fuchsia_virtualization::NetSpec out(mac, /*enable_bridge=*/true);
    cfg.net_devices(std::vector<fuchsia_virtualization::NetSpec>{out});
    cfg.default_net(false);
  }

  zx::result guest_endpoints = fidl::CreateEndpoints<fuchsia_virtualization::Guest>();
  if (guest_endpoints.is_error()) {
    FX_PLOGST(ERROR, name.c_str(), guest_endpoints.status_value())
        << "failed to create guest endpoints";
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }
  auto [guest_client_end, guest_server_end] = *std::move(guest_endpoints);
  if (fidl::Result<fuchsia_virtualization::DebianGuestManager::Launch> launch_result =
          guest_manager_->Launch({std::move(cfg), std::move(guest_server_end)});
      !launch_result.is_ok()) {
    FX_LOGST(ERROR, name.c_str()) << "failed to launch guest with error: "
                                  << launch_result.error_value().FormatDescription();
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }

  fidl::SyncClient<fuchsia_virtualization::Guest> guest =
      fidl::SyncClient<fuchsia_virtualization::Guest>({std::move(guest_client_end)});

  fidl::Result<fuchsia_virtualization::Guest::GetConsole> get_serial_console_result =
      guest->GetConsole();
  if (!get_serial_console_result.is_ok()) {
    FX_LOGST(ERROR, name.c_str()) << "failed to get guest console with error: "
                                  << get_serial_console_result.error_value().FormatDescription();
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }

  FX_LOGST(INFO, name.c_str()) << "connecting to guest over serial";
  GuestConsole serial(std::make_unique<ZxSocket>(std::move(get_serial_console_result->socket())));
  if (zx_status_t status = serial.Start(zx::time::infinite()); status != ZX_OK) {
    FX_PLOGST(ERROR, name.c_str(), status) << "failed to start serial";
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }
  FX_LOGST(INFO, name.c_str()) << "successfully connected to guest over serial";

  // Wait until the guest-interaction daemon wakes up and logs that it's ready to accept
  // incoming connections.
  constexpr std::string_view kGuestInteractionOutputMarker = "Listening";
  std::stringstream command;
  command << "journalctl -f --no-tail -u guest_interaction_daemon | grep -m1 "
          << kGuestInteractionOutputMarker;
  if (zx_status_t status = serial.RepeatCommandTillSuccess(
          command.str(), "$", std::string(kGuestInteractionOutputMarker), zx::time::infinite(),
          zx::sec(30));
      status != ZX_OK) {
    FX_PLOGST(ERROR, name.c_str(), status) << "failed to wait for guest_interaction daemon";
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }

  zx::result vsock_endpoints = fidl::CreateEndpoints<fuchsia_virtualization::HostVsockEndpoint>();
  if (vsock_endpoints.is_error()) {
    FX_PLOGST(ERROR, name.c_str(), vsock_endpoints.status_value())
        << "failed to create vsock endpoints";
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }
  auto [vsock_client_end, vsock_server_end] = *std::move(vsock_endpoints);
  fidl::SyncClient vsock{std::move(vsock_client_end)};

  fidl::Result<fuchsia_virtualization::Guest::GetHostVsockEndpoint> vsock_res =
      guest->GetHostVsockEndpoint(std::move(vsock_server_end));
  if (!vsock_res.is_ok()) {
    FX_LOGST(ERROR, name.c_str()) << "failed to get host vsock endpoint with error: "
                                  << vsock_res.error_value().FormatDescription();
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }

  fidl::Result<fuchsia_virtualization::HostVsockEndpoint::Connect> connect_res =
      vsock->Connect(GUEST_INTERACTION_PORT);
  if (!connect_res.is_ok()) {
    FX_LOGST(ERROR, name.c_str()) << "failed to get connect on vsock with error: "
                                  << connect_res.error_value().FormatDescription();
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }

  fbl::unique_fd vsock_fd;
  if (zx_status_t status =
          fdio_fd_create(connect_res->socket().release(), vsock_fd.reset_and_get_address());
      status != ZX_OK) {
    FX_PLOGST(ERROR, name.c_str(), status) << "failed to create fd for vsock";
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }
  if (int err = SetNonBlocking(vsock_fd); err != 0) {
    FX_LOGST(ERROR, name.c_str()) << "failed to set the vsock to non-blocking with errno: "
                                  << strerror(err);
    completer.ReplyError(fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
    return;
  }

  guest_session_.emplace(std::move(name), loop_, std::move(vsock_fd), guest_manager_,
                         fidl::SyncClient(std::move(request->network)), completer.ToAsync());
}

void ControllerImpl::CreateNetwork(CreateNetworkRequestView request,
                                   CreateNetworkCompleter::Sync& completer) {
  if (!HasActiveGuest()) {
    FX_LOGS(ERROR) << "CreateNetwork called but no guest found";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  if (guest_session_->create_network_called) {
    FX_LOGST(ERROR, guest_session_->impl.GetName().c_str())
        << "CreateNetwork already called for guest";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  guest_session_->create_network_called = true;
  fidl::BindServer(loop_.dispatcher(), std::move(request->network), &guest_session_->impl);
}

bool ControllerImpl::HasActiveGuest() const {
  return guest_session_.has_value() && !guest_session_->impl.IsShutdown();
}

// ControllerImpl::GuestSession implementation.

ControllerImpl::GuestSession::GuestSession(
    std::string name, async::Loop& loop, fbl::unique_fd vsock_fd,
    fidl::SyncClient<fuchsia_virtualization::DebianGuestManager>& guest_manager,
    fidl::SyncClient<fuchsia_netemul_network::Network> network,
    fidl::internal::WireCompleter<::fuchsia_netemul_guest::Controller::CreateGuest>::Async
        create_guest_completer)
    : impl(std::move(name), loop, std::move(vsock_fd), guest_manager, std::move(network),
           std::move(create_guest_completer)) {}
