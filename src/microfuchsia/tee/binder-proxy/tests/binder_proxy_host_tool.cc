// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>

#include <android/system/microfuchsia/trusted_app/ITrustedApp.h>
#include <android/system/microfuchsia/vm_service/IMicrofuchsia.h>
#include <binder/Binder.h>
#include <binder/RpcSession.h>
#include <binder/unique_fd.h>
#include <linux/vm_sockets.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/lib/fxl/strings/string_number_conversions.h"

#if defined(__linux__)
#include <linux/vm_sockets.h>
#endif

namespace {

using IMicrofuchsia = android::system::microfuchsia::vm_service::IMicrofuchsia;
using ITrustedApp = android::system::microfuchsia::trusted_app::ITrustedApp;

fit::result<int, android::sp<android::RpcSession>> ConnectToBinderRPCSession(bool inet,
                                                                             unsigned port,
                                                                             unsigned cid) {
  auto session = android::RpcSession::make();
  session->setMaxIncomingThreads(1);

  auto status = session->setupPreconnectedClient({}, [inet, port, cid] {
    FX_LOGS(TRACE) << "preconnected client callback invoked";
    android::binder::unique_fd socket_fd;
    if (inet) {
      socket_fd = android::binder::unique_fd(socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0));
    } else {
      socket_fd = android::binder::unique_fd(socket(AF_VSOCK, SOCK_STREAM, 0));
    }
    if (!socket_fd.ok()) {
      FX_LOGS(ERROR) << "socket failed: " << errno << ": " << strerror(errno);
      return android::binder::unique_fd();
    }

    if (inet) {
      int no_delay = 1;
      int rv = setsockopt(socket_fd.get(), IPPROTO_TCP, TCP_NODELAY, &no_delay, sizeof(no_delay));
      if (rv < 0) {
        FX_LOGS(ERROR) << "setsockopt failed: " << errno << ": " << strerror(errno);
        return android::binder::unique_fd();
      }
    }

    struct sockaddr sockaddr = {};
    size_t sockaddr_len = 0;
    if (inet) {
      const sockaddr_in loopback_addr{
          .sin_family = AF_INET,
          .sin_port = htons(port),
          .sin_addr =
              {
                  .s_addr = htonl(INADDR_LOOPBACK),
              },
      };
      sockaddr_len = sizeof(loopback_addr);
      memcpy(&sockaddr, &loopback_addr, sockaddr_len);
    } else {
      const sockaddr_vm addr{
          .svm_family = AF_VSOCK,
          .svm_port = port,
          .svm_cid = cid,
          .svm_zero = {},
      };
      sockaddr_len = sizeof(addr);
      memcpy(&sockaddr, &addr, sockaddr_len);
    }

    int rv = 0;

    for (int tries = 0; tries < 5; ++tries) {
      FX_LOGS(TRACE) << "connecting";
      rv = connect(socket_fd.get(), &sockaddr, sockaddr_len);
      if (rv == 0) {
        break;
      }
      // If we can't connect, sleep for a bit to let the target port come up.
      sleep(2);
    }
    if (rv != 0) {
      FX_LOGS(ERROR) << "Could not connect: " << errno << ": " << strerror(errno);
      return android::binder::unique_fd();
    }

    return socket_fd;
  });

  if (status != android::OK) {
    FX_LOGS(ERROR) << "setupPreconnectedClient failed: " << android::statusToString(status);
    return fit::error(EXIT_FAILURE);
  }

  return fit::ok(session);
}

int BinderProxyHostTool(const fxl::CommandLine& command_line) {
  std::string port_str = command_line.GetOptionValueWithDefault("port", "5680");
  unsigned port = fxl::StringToNumber<unsigned>(port_str);

  bool inet = command_line.HasOption("inet");
  std::string cid_str = command_line.GetOptionValueWithDefault("cid", "3");
  unsigned cid = fxl::StringToNumber<unsigned>(cid_str);

  auto session = ConnectToBinderRPCSession(inet, port, cid);
  if (session.is_error()) {
    return session.error_value();
  }

  auto root_object = session->getRootObject();
  auto status = root_object->pingBinder();
  if (status != android::OK) {
    FX_LOGS(ERROR) << "pingBinder failed: " << android::statusToString(status);
    return EXIT_FAILURE;
  }

  FX_LOGS(INFO) << "binder ping received";

  auto microfuchsia = android::checked_interface_cast<IMicrofuchsia>(root_object);

  {
    std::vector<std::string> uuids;
    auto result = microfuchsia->trustedAppUuids(&uuids);

    FX_LOGS(INFO) << "trustedAppUuids result: " << result;
    FX_LOGS(INFO) << "uuids len: " << uuids.size();
    for (const auto& uuid : uuids) {
      FX_LOGS(INFO) << "uuid: " << uuid;
    }
  }

  int ta_port = port + 1;

  auto ta_session = ConnectToBinderRPCSession(inet, ta_port, cid);
  if (ta_session.is_error()) {
    FX_LOGS(ERROR) << "Could not connect to RPC session for first TA over port " << ta_port
                   << " using " << (inet ? "inet" : "vsock") << " socket and cid " << cid;
    return EXIT_FAILURE;
  }

  auto ta_root_object = ta_session->getRootObject();

  auto ta_binder = android::checked_interface_cast<ITrustedApp>(ta_root_object);

  android::sp<android::system::microfuchsia::trusted_app::ITrustedAppSession> session_binder;
  android::system::microfuchsia::trusted_app::OpResult op_result;
  auto result = ta_binder->openSession({}, &op_result, &session_binder);
  FX_LOGS(INFO) << "openSession result: " << result;

  op_result = {};
  result = session_binder->invokeCommand(0, {}, &op_result);
  FX_LOGS(INFO) << "invokeCommand result: " << result;

  return EXIT_SUCCESS;
}

void Usage() { printf("Usage: binder_proxy_host_tool [--inet] [--port=5680] [--cid=3]\n"); }

}  // namespace

int main(int argc, char** argv) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (command_line.HasOption("help")) {
    Usage();
    return EXIT_SUCCESS;
  }
  fxl::SetLogSettingsFromCommandLine(command_line);
  return BinderProxyHostTool(command_line);
}