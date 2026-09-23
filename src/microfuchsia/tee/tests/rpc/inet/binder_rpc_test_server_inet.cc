// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <lib/fdio/fdio.h>
#include <lib/syslog/cpp/macros.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>

#include <utility>

// clang-format off: Don't let clang-format sort these include directives above syslog includes
// to avoid https://fxbug.dev/364711885
#include <BnBinderEcho.h>
#include <binder/Binder.h>
#include <binder/RpcServer.h>
// clang-format: on

#include "src/microfuchsia/tee/lib/dev_urandom_compat/dev_urandom_compat.h"

namespace {

bool RegisterDevUrandom() {
  zx_status_t status = register_dev_urandom_compat();
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to register /dev/urandom compat device: " << status;
    return false;
  }

  return true;
}

class BinderEcho : public BnBinderEcho {
 public:
  android::binder::Status echo(const std::string& str, std::string* aidl_return) final {
    *aidl_return = str;
    return android::binder::Status::ok();
  }
};

int RunBinderServer() {
  if (!RegisterDevUrandom()) {
    return EXIT_FAILURE;
  }

  auto server_fd = android::binder::unique_fd(socket(AF_INET, SOCK_STREAM, 0));
  if (!server_fd.ok()) {
    FX_LOGS(ERROR) << "socket failed: " << errno << ": " << strerror(errno);
    return EXIT_FAILURE;
  }

  constexpr int kLoopbackPort = 62000;
  sockaddr_in loopback_addr{
      .sin_family = AF_INET,
      .sin_port = htons(kLoopbackPort),
      .sin_addr =
          {
              .s_addr = htonl(INADDR_LOOPBACK),
          },
  };

  int rv = bind(server_fd.get(), reinterpret_cast<const sockaddr*>(&loopback_addr),
                sizeof(loopback_addr));
  if (rv != 0) {
    FX_LOGS(ERROR) << "bind failed: " << errno << ": " << strerror(errno);
    return EXIT_FAILURE;
  }

  auto server = android::RpcServer::make();
  FX_LOGS(TRACE) << "setting root object";
  auto root_object = android::sp<BinderEcho>::make();
  server->setRootObject(root_object);

  FX_LOGS(TRACE) << "setting up inet server";
  auto status = server->setupRawSocketServer(std::move(server_fd));
  if (status != android::OK) {
    FX_LOGS(ERROR) << "setupRawSocketServer failed: " << android::statusToString(status);
    return EXIT_FAILURE;
  }

  FX_LOGS(TRACE) << "joining server";
  server->join();

  return EXIT_SUCCESS;
}

}  // namespace

int main(int argc, char** argv) { return RunBinderServer(); }