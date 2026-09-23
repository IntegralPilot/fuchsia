// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_VSOCK_SSHD_HOST_SERVICE_H_
#define SRC_DEVELOPER_VSOCK_SSHD_HOST_SERVICE_H_

#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.vsock/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>
#include <sys/socket.h>

namespace sshd_host {

inline constexpr char kSshDirectory[] = "/data/ssh";
inline constexpr char kAuthorizedKeysPath[] = "/data/ssh/authorized_keys";
// Name of the collection that contains sshd shell child components.
inline constexpr std::string_view kShellCollection = "shell";

// Service relies on the default async dispatcher and is not thread safe.
class Service : public fidl::Server<fuchsia_vsock::Acceptor> {
 public:
  Service(async_dispatcher_t* dispatcher, uint16_t port);
  ~Service();

  void Accept(AcceptRequest& request, AcceptCompleter::Sync& completer) override;

 private:
  class Controller;

  void Launch(zx::socket socket);

  void OnStop(zx_status_t status, Controller* controller);

  async_dispatcher_t* dispatcher_;
  uint64_t next_child_num_ = 0;
  std::optional<fidl::ServerBinding<fuchsia_vsock::Acceptor>> binding_;

  class Controller final : public fidl::AsyncEventHandler<fuchsia_component::ExecutionController> {
   public:
    Controller(Service* service, uint64_t child_num, std::string child_name,
               fidl::ClientEnd<fuchsia_component::ExecutionController> client_end,
               async_dispatcher_t* dispatcher, fidl::SyncClient<fuchsia_component::Realm> realm)
        : service_(service),
          child_num_(child_num),
          child_name_(std::move(child_name)),
          client_(std::move(client_end), dispatcher, this),
          realm_(std::move(realm)) {}
    void OnStop(fidl::Event<fuchsia_component::ExecutionController::OnStop>& event) final {
      service_->OnStop(event.stopped_payload().status().value_or(ZX_OK), this);
    }
    void on_fidl_error(fidl::UnbindInfo error) final {
      FX_LOGS(WARNING) << "encountered FIDL error " << error;
      service_->OnStop(error.ToError().status(), this);
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_component::ExecutionController> metadata) override {
      FX_LOGS(WARNING) << "fuchsia.component/ExecutionController delivered unknown event "
                       << metadata.event_ordinal;
    }

    fidl::Client<fuchsia_component::ExecutionController>& operator->() { return client_; }

    Service* service_;
    uint64_t child_num_;
    std::string child_name_;
    fidl::Client<fuchsia_component::ExecutionController> client_;
    fidl::SyncClient<fuchsia_component::Realm> realm_;
  };

  std::vector<fidl::ClientEnd<fuchsia_vsock::Connection>> client_ends_;
  std::map<uint64_t, Controller> controllers_;
};

}  // namespace sshd_host

#endif  // SRC_DEVELOPER_VSOCK_SSHD_HOST_SERVICE_H_
