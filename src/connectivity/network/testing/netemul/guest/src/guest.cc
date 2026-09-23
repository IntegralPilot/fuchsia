// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/testing/netemul/guest/src/guest.h"

#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include "fidl/fuchsia.io/cpp/fidl.h"
#include "fidl/fuchsia.netemul.guest/cpp/wire_types.h"
#include "fidl/fuchsia.virtualization.guest.interaction/cpp/fidl.h"
#include "fidl/fuchsia.virtualization/cpp/markers.h"
#include "lib/fidl/cpp/wire/status.h"

GuestImpl::GuestImpl(
    std::string name, async::Loop& loop, fbl::unique_fd vsock_fd,
    fidl::SyncClient<fuchsia_virtualization::DebianGuestManager>& guest_manager,
    fidl::SyncClient<fuchsia_netemul_network::Network> network,
    fidl::internal::WireCompleter<::fuchsia_netemul_guest::Controller::CreateGuest>::Async
        create_guest_completer)
    : name_(std::move(name)),
      loop_(loop),
      guest_interaction_client_(vsock_fd.release()),
      network_(std::move(network)),
      guest_manager_(guest_manager),
      create_guest_completer_(std::move(create_guest_completer)) {
  int ret = guest_interaction_client_.Start(guest_interaction_service_thread_);
  ZX_ASSERT_MSG(ret == thrd_success, "[%s] failed to create thread with error: %d", name_.c_str(),
                ret);
}

GuestImpl::~GuestImpl() { OnTeardown(); }

const std::string& GuestImpl::GetName() const { return name_; }

bool GuestImpl::IsShutdown() const { return shutdown_; }

void GuestImpl::PutFile(PutFileRequestView request, PutFileCompleter::Sync& completer) {
  CheckGuestValidForGuestFIDLMethod("PutFile");
  guest_interaction_client_.Put(fidl::ClientEnd<fuchsia_io::File>(request->local_file.TakeHandle()),
                                std::string(request->remote_path.get()),
                                [completer = completer.ToAsync()](zx_status_t put_result) mutable {
                                  completer.Reply(put_result);
                                });
}

void GuestImpl::GetFile(GetFileRequestView request, GetFileCompleter::Sync& completer) {
  CheckGuestValidForGuestFIDLMethod("GetFile");
  guest_interaction_client_.Get(std::string(request->remote_path.get()),
                                fidl::ClientEnd<fuchsia_io::File>(request->local_file.TakeHandle()),
                                [completer = completer.ToAsync()](zx_status_t get_result) mutable {
                                  completer.Reply(get_result);
                                });
}

void GuestImpl::ExecuteCommand(ExecuteCommandRequestView request,
                               ExecuteCommandCompleter::Sync& completer) {
  CheckGuestValidForGuestFIDLMethod("ExecuteCommand");
  std::map<std::string, std::string> env_variables;
  for (const auto& var : request->env) {
    env_variables.insert({std::string(var.key.get()), std::string(var.value.get())});
  }
  guest_interaction_client_.Exec(
      std::string(request->command.get()), env_variables, std::move(request->stdin_),
      std::move(request->stdout_), std::move(request->stderr_),
      fidl::ServerEnd<fuchsia_virtualization_guest_interaction::CommandListener>(
          request->command_listener.TakeHandle()),
      loop_.dispatcher());
}

void GuestImpl::AddPort(AddPortRequestView request, AddPortCompleter::Sync& completer) {
  if (!create_guest_completer_.has_value()) {
    FX_LOGST(ERROR, name_.c_str()) << "AddPort previously called";
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  ZX_ASSERT_MSG(
      !shutdown_,
      "[%s] Guest was shutdown even though `fuchsia_netemul_guest::Guest was not served yet",
      name_.c_str());
  fidl::internal::WireCompleter<::fuchsia_netemul_guest::Controller::CreateGuest>::Async
      create_guest_completer = std::move(create_guest_completer_.value());
  create_guest_completer_.reset();

  fit::result<fidl::Error> res =
      network_->AddPort({std::move(request->port), std::move(request->interface)});
  if (res.is_ok()) {
    zx::result guest_endpoints = fidl::CreateEndpoints<fuchsia_netemul_guest::Guest>();
    if (guest_endpoints.is_error()) {
      FX_PLOGST(ERROR, name_.c_str(), guest_endpoints.status_value())
          << "failed to create guest endpoints";
      create_guest_completer.ReplyError(
          fuchsia_netemul_guest::wire::ControllerCreateGuestError::kLaunchFailed);
      return;
    }

    auto [guest_client_end, guest_server_end] = *std::move(guest_endpoints);
    fidl::BindServer(loop_.dispatcher(), std::move(guest_server_end), this,
                     std::mem_fn(&GuestImpl::OnGuestUnbound));
    create_guest_completer.ReplySuccess(std::move(guest_client_end));
  } else {
    create_guest_completer.ReplyError(
        fuchsia_netemul_guest::wire::ControllerCreateGuestError::kAttachFailed);
  }
}

void GuestImpl::Shutdown(ShutdownCompleter::Sync& completer) {
  CheckGuestValidForGuestFIDLMethod("Shutdown");
  DoShutdown();
  completer.Reply();
  completer.Close(ZX_OK);
}

void GuestImpl::CheckGuestValidForGuestFIDLMethod(const char* method_name) const {
  // We only begin serving `fuchsia_netemul_guest::Guest` after the guest has
  // been attached to the `fuchsia_netemul_network::Network` provided on
  // construction, in the `AddPort` method. At that point, the guest handle is
  // moved out of `add_port_hooks_`, which is itself reset.
  //
  // Validate here that this invariant is respected.
  ZX_ASSERT_MSG(!create_guest_completer_.has_value(),
                "[%s] %s called even though fuchsia_netemul_guest::Guest is not being served",
                name_.c_str(), method_name);
  // We close the `fuchsia_netemul_guest::Guest` channel when the guest is
  // shutdown, therefore none of its FIDL methods can be called post-shutdown.
  ZX_ASSERT_MSG(!shutdown_, "[%s] %s called even though guest was shutdown", name_.c_str(),
                method_name);
}

void GuestImpl::DoShutdown() {
  guest_interaction_client_.Stop();
  int32_t ret_code;
  int ret = thrd_join(guest_interaction_service_thread_, &ret_code);
  ZX_ASSERT_MSG(ret == thrd_success, "[%s] failed to join thread with error: %d", name_.c_str(),
                ret);
  ZX_ASSERT_MSG(ret_code == 0, "[%s] Thread exited with non-zero return code: %d", name_.c_str(),
                ret_code);

  fidl::Result<fuchsia_virtualization::DebianGuestManager::ForceShutdown> shutdown_result =
      guest_manager_->ForceShutdown();
  if (!shutdown_result.is_ok()) {
    std::string err_msg = shutdown_result.error_value().FormatDescription();
    ZX_PANIC("[%s] failed to shutdown guest with error: %s", name_.c_str(), err_msg.c_str());
  }
  shutdown_ = true;
}

void GuestImpl::OnGuestUnbound(fidl::UnbindInfo info,
                               fidl::ServerEnd<fuchsia_netemul_guest::Guest> server_end) {
  OnTeardown();
}

void GuestImpl::OnTeardown() {
  if (!shutdown_) {
    // Since `GuestImpl` is owned by the controller and new guests
    // can't be created unless the existing guest has been shutdown,
    // this condition can only arise when a client has forgotten
    // to call `fuchsia_netemul_guest::Shutdown` before the client handle
    // is dropped.
    // Log an error since this usage is brittle and can lead to errors
    // if/when other tests are introduced.
    // TODO(https://fxbug.dev/42069886): Consider blocking in CreateGuest instead.
    FX_LOGST(ERROR, name_.c_str()) << "Unbinding guest without Shutdown called";
    DoShutdown();
  }
}
