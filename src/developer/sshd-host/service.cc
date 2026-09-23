// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/sshd-host/service.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.developer.console/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>
#include <zircon/types.h>

#include <array>
#include <format>
#include <vector>

#include <fbl/unique_fd.h>

namespace sshd_host {

Service::Service(async_dispatcher_t* dispatcher, uint16_t port)
    : dispatcher_(dispatcher),
      sock_(fbl::unique_fd(socket(AF_INET6, SOCK_STREAM, IPPROTO_TCP))),
      waiter_(dispatcher) {
  if (!sock_.is_valid()) {
    FX_LOGS(FATAL) << "Failed to create socket: " << strerror(errno);
  }
  sockaddr_storage addr;
  *reinterpret_cast<struct sockaddr_in6*>(&addr) = sockaddr_in6{
      .sin6_family = AF_INET6,
      .sin6_port = htons(port),
      .sin6_addr = in6addr_any,
  };
  if (bind(sock_.get(), reinterpret_cast<const sockaddr*>(&addr), sizeof addr) < 0) {
    FX_LOGS(FATAL) << "Failed to bind to " << port << ": " << strerror(errno);
  }

  FX_LOG_KV(INFO, "listen() for inbound SSH connections", FX_KV("port", (int)port));
  if (listen(sock_.get(), 10) < 0) {
    FX_LOGS(FATAL) << "Failed to listen: " << strerror(errno);
  }

  if (zx_status_t status = zx::eventpair::create(0, &console_stopper_, &console_stopper_local_);
      status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to create eventpair";
  }

  Wait();
}

Service::~Service() = default;

void Service::Wait() {
  FX_LOG_KV(DEBUG, "Waiting for next connection");

  waiter_.Wait(
      [this](zx_status_t status, uint32_t /*events*/) {
        if (status != ZX_OK) {
          FX_PLOGS(FATAL, status) << "Failed to wait on socket";
        }

        struct sockaddr_storage peer_addr{};
        socklen_t peer_addr_len = sizeof(peer_addr);
        fbl::unique_fd conn(
            accept(sock_.get(), reinterpret_cast<struct sockaddr*>(&peer_addr), &peer_addr_len));
        if (!conn.is_valid()) {
          if (errno == EPIPE) {
            FX_LOGS(ERROR) << "The netstack died. Terminating.";
            // Avoid a crash here because the netstack terminating already
            // causes the system to reboot. This prevents cascading crash
            // reports.
            exit(1);
          } else {
            FX_LOGS(ERROR) << "Failed to accept: " << strerror(errno);
            // Wait for another connection.
            Wait();
          }
          return;
        }

        std::string peer_name = "unknown";
        char host[NI_MAXHOST];
        char port[NI_MAXSERV];
        if (int res =
                getnameinfo(reinterpret_cast<struct sockaddr*>(&peer_addr), peer_addr_len, host,
                            sizeof(host), port, sizeof(port), NI_NUMERICHOST | NI_NUMERICSERV);
            res == 0) {
          peer_name = std::format("[{}]:{}", host, port);
        } else {
          FX_LOGS(WARNING)
              << "Error from getnameinfo(.., NI_NUMERICHOST | NI_NUMERICSERV) for peer address: "
              << gai_strerror(res);
        }
        FX_LOG_KV(DEBUG, "Accepted connection", FX_KV("remote", peer_name.c_str()));

        LaunchConsole(std::move(conn));
        Wait();
      },
      sock_.get(), POLLIN);
}

Service::LogRedirect::LogRedirect(async_dispatcher_t* dispatcher, zx::socket socket,
                                  uint64_t child_tag)
    : dispatcher_(dispatcher),
      socket_(std::move(socket)),
      child_tag_(child_tag),
      waiter_(this, socket_.get(), ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED) {
  Wait();
}

Service::LogRedirect::~LogRedirect() { waiter_.Cancel(); }

void Service::LogRedirect::Wait() {
  zx_status_t status = waiter_.Begin(dispatcher_);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to wait on stderr socket for " << child_tag_;
  }
}

void Service::LogRedirect::OnLog(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                 zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Wait on stderr failed for " << child_tag_;
    return;
  }

  // It's possible for the socket to be both readable and closed in the same signal.
  if (signal->observed & ZX_SOCKET_READABLE) {
    constexpr size_t kStderrBufSize = 1024;
    std::array<char, kStderrBufSize> buf;
    size_t actual;
    if (zx_status_t status = socket_.read(0, buf.data(), buf.size(), &actual); status != ZX_OK) {
      if (status != ZX_ERR_PEER_CLOSED) {
        FX_PLOGS(ERROR, status) << "Failed to read from stderr socket for " << child_tag_;
      }
      return;
    }

    buf_.append(buf.data(), actual);

    constexpr size_t kMaxStderrBufSize = 16 * 1024;  // 16 KiB
    if (buf_.length() > kMaxStderrBufSize) {
      FX_LOGS(WARNING) << "sshd stderr buffer for " << child_tag_
                       << " is full, flushing without newline.";
      FX_LOGS(DEBUG) << "ssh stderr(" << child_tag_ << "): " << buf_;
      buf_.clear();
    }

    std::string_view msg_stream(buf_);
    while (!msg_stream.empty()) {
      size_t msg_end = msg_stream.find('\n');
      // no msg in stream (e.g. line break not found).
      if (msg_end == std::string_view::npos) {
        break;
      }
      // include '\n'
      std::string_view msg = msg_stream.substr(0, msg_end + 1);
      msg_stream.remove_prefix(msg.size());
      // remove '\n'
      msg.remove_suffix(1);
      // remove '\r' if present, '\r' may only be inserted
      // in certain systems preceding `\n`.
      if (msg.ends_with('\r')) {
        msg.remove_suffix(1);
      }
      // output msg even if empty
      FX_LOGS(DEBUG) << "ssh stderr(" << child_tag_ << "): " << msg;
    }

    // If the entire buffer was processed, the view will be empty
    if (msg_stream.empty()) {
      buf_.clear();
    } else if (msg_stream.data() != buf_.data()) {
      buf_ = std::string(msg_stream);
    }
  }
  if (signal->observed & ZX_SOCKET_PEER_CLOSED) {
    if (!buf_.empty()) {
      FX_LOGS(DEBUG) << "ssh stderr(" << child_tag_ << "): " << buf_;
    }
    // Do not re-arm the wait, the socket is closed.
    return;
  }
  Wait();
}

void Service::LaunchConsole(fbl::unique_fd conn) {
  uint64_t child_num = next_child_num_++;
  std::string child_name = std::format("sshd-{}", child_num);

  auto realm_client_end = component::Connect<fuchsia_component::Realm>();
  if (realm_client_end.is_error()) {
    FX_PLOGS(ERROR, realm_client_end.status_value()) << "Failed to connect to realm service";
    return;
  }
  fidl::Result resolve_info_result = fidl::Call(*realm_client_end)->GetResolvedInfo();
  if (resolve_info_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to retrieve realm info "
                   << resolve_info_result.error_value().FormatDescription();
    return;
  }
  std::optional maybe_package = std::move(resolve_info_result.value().resolved_info().package());
  if (!maybe_package.has_value()) {
    FX_LOGS(ERROR) << "Resolve info doesn't provide package value";
    return;
  }
  auto package = std::move(maybe_package.value());

  auto console_launcher_client_end = component::Connect<fuchsia_developer_console::Launcher>();
  if (console_launcher_client_end.is_error()) {
    FX_PLOGS(ERROR, console_launcher_client_end.status_value())
        << "Failed to connect to console launcher service";
    return;
  }
  auto developer_console_launcher =
      std::make_unique<fidl::Client<fuchsia_developer_console::Launcher>>(
          std::move(*console_launcher_client_end), dispatcher_);

  fuchsia_developer_console::PackageProgram package_program{{
      .package = std::move(package),
      .path = "bin/sshd",
  }};

  // Ensure we kill all shells if sshd-host itself goes away.
  zx::eventpair stopper;
  if (zx_status_t status = console_stopper_.duplicate(ZX_RIGHT_SAME_RIGHTS, &stopper) != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to duplicate console stopper";
    return;
  };

  std::vector<fuchsia_process::NameInfo> namespace_entries;

  auto clone_namespace_entry = [&namespace_entries](const char* path,
                                                    fuchsia_io::wire::Flags flags) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    if (endpoints.is_error()) {
      return endpoints.status_value();
    }
    if (zx_status_t status = fdio_open3(path, static_cast<uint64_t>(flags),
                                        endpoints->server.TakeChannel().release());
        status != ZX_OK) {
      return status;
    }
    namespace_entries.emplace_back(path, std::move(endpoints->client));
    return ZX_OK;
  };

  if (zx_status_t status = clone_namespace_entry("/config/data", fuchsia_io::wire::kPermReadable);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "failed to clone /config/data";
    return;
  }
  if (zx_status_t status =
          clone_namespace_entry("/data", fuchsia_io::kPermReadable | fuchsia_io::kPermWritable);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "failed to clone /data";
    return;
  }
  zx::socket stderr_socket, child_stderr;
  if (zx_status_t status = zx::socket::create(0, &stderr_socket, &child_stderr); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to create stderr socket";
    return;
  }

  fuchsia_developer_console::RawHandles raw_handles{{.stderr_ = std::move(child_stderr)}};
  for (auto x : {
           std::make_tuple(conn.get(), &raw_handles.stdin_()),
           {conn.get(), &raw_handles.stdout_()},
       }) {
    auto [file, target] = x;
    zx::handle conn_handle;
    if (zx_status_t status = fdio_fd_clone(file, conn_handle.reset_and_get_address());
        status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to clone file descriptor " << file;
      return;
    }
    *target = std::move(conn_handle);
  }

  fuchsia_developer_console::LaunchOptions options{{
      .name = std::move(child_name),
      .args = std::vector<std::string>{"-ie", "-f", "/config/data/sshd_config"},
      .program = fuchsia_developer_console::Program::WithFromPackage(std::move(package_program)),
      .io_handles = fuchsia_developer_console::IoHandles::WithRawHandles(std::move(raw_handles)),
      .namespace_entries = std::move(namespace_entries),
      .stopper = std::move(stopper),
      .directories_fixup = true,
  }};

  auto log_redirect =
      std::make_unique<LogRedirect>(dispatcher_, std::move(stderr_socket), child_num);
  auto* launcher_raw = developer_console_launcher.get();
  (*launcher_raw)
      ->Launch(std::move(options))
      .Then([developer_console_launcher = std::move(developer_console_launcher),
             log_redirect = std::move(log_redirect)](
                fidl::Result<fuchsia_developer_console::Launcher::Launch>& result) {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "launch failed: " << result.error_value().FormatDescription();
          return;
        }
        int64_t return_code = result.value().return_code();
        FX_LOGS(DEBUG) << "shell finished with code " << return_code;
      });
}

}  // namespace sshd_host
