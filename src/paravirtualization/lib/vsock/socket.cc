// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/paravirtualization/lib/vsock/socket.h"

#include <fidl/fuchsia.vsock/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/fdio.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/thread_safety.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/socket.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <lib/zxio/zxio.h>
#include <sys/socket.h>
#include <zircon/types.h>

#include <memory>
#include <utility>
#include <variant>

#include <fbl/unaligned.h>
#include <fbl/unique_fd.h>

#include "src/paravirtualization/lib/vsock/vm_sockets.h"
#include "src/paravirtualization/lib/vsock/zxio_connected_socket.h"

namespace vsock {

struct UnconnectedState {};

struct BoundState {
  uint32_t port;
  fidl::ClientEnd<fuchsia_vsock::Listener> listener;
};

struct ListeningState {
  uint32_t port;
  fidl::ClientEnd<fuchsia_vsock::Listener> listener;
};

// TODO(https://fxbug.dev/361410840): In order to generate the proper errors on send/recvmsg we
// should also track a "Connecting" state.

struct ConnectedState {
  std::unique_ptr<zxio_storage_t> inner_pipe_storage;
  fidl::ClientEnd<fuchsia_vsock::Connection> connection;
  uint32_t port;
};

class Socket {
 public:
  explicit Socket(fidl::ClientEnd<fuchsia_vsock::Connector> connector_client)
      : vsock_connector_client_(std::move(connector_client)) {}
  zx_status_t Bind(const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code);
  zx_status_t Connect(const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code);
  zx_status_t Listen(int backlog, int16_t* out_code);
  zx_status_t Accept(struct sockaddr* addr, socklen_t* addrlen, zxio_storage_t* out_storage,
                     int16_t* out_code);
  void WaitBegin(zxio_signals_t zxio_signals, zx_handle_t* out_handle, zx_signals_t* out_signals);
  void WaitEnd(zx_signals_t signals, zxio_signals_t* out_zxio_signals);
  zx_status_t Readv(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                    size_t* out_actual);
  zx_status_t Writev(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                     size_t* out_actual);

 private:
  fidl::SyncClient<fuchsia_vsock::Connector> vsock_connector_client_;

  std::mutex state_lock_;
  std::variant<UnconnectedState, BoundState, ListeningState, ConnectedState> state_
      FIT_GUARDED(state_lock_);
};

zx_status_t Socket::Bind(const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
  if (addr->sa_family != AF_VSOCK) {
    *out_code = EADDRNOTAVAIL;
    return ZX_OK;
  }
  if (addrlen < sizeof(struct sockaddr_vm)) {
    return ZX_ERR_INTERNAL;
  }

  uint32_t port = fbl::UnalignedLoad<struct sockaddr_vm>(addr).svm_port;
  uint32_t cid = fbl::UnalignedLoad<struct sockaddr_vm>(addr).svm_cid;

  std::lock_guard lock(state_lock_);
  if (std::holds_alternative<UnconnectedState>(state_)) {
    auto [client, server] = fidl::Endpoints<fuchsia_vsock::Listener>::Create();
    auto result = vsock_connector_client_->Bind({cid, port, std::move(server)});
    if (result.is_error()) {
      FX_LOGS(ERROR) << "vsock connector listen call failed: " << result.error_value();
      return ZX_ERR_INTERNAL;
    }
    state_ = BoundState{.port = port, .listener = std::move(client)};
    *out_code = 0;
  } else {
    *out_code = EINVAL;
  }

  return ZX_OK;
}

zx_status_t Socket::Connect(const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
  if (addr == nullptr || addrlen < sizeof(struct sockaddr_vm)) {
    *out_code = EINVAL;
    return ZX_OK;
  }

  if (addr->sa_family != AF_VSOCK) {
    return EAFNOSUPPORT;
  }

  auto addr_vm = fbl::UnalignedLoad<struct sockaddr_vm>(addr);
  uint32_t remote_cid = addr_vm.svm_cid;
  uint32_t remote_port = addr_vm.svm_port;

  zx::socket sock0, sock1;
  zx_status_t status = zx::socket::create(ZX_SOCKET_STREAM, &sock0, &sock1);
  if (status != ZX_OK) {
    return status;
  }

  fuchsia_vsock::ConnectionTransport transport;
  transport.data(std::move(sock0));
  auto [connection_client, connection_server] =
      fidl::Endpoints<fuchsia_vsock::Connection>::Create();
  transport.con(std::move(connection_server));

  std::lock_guard lock(state_lock_);
  if (std::holds_alternative<UnconnectedState>(state_)) {
    auto connect_result =
        vsock_connector_client_->Connect({remote_cid, remote_port, std::move(transport)});

    if (connect_result.is_error()) {
      // TODO(https://fxbug.dev/361410840): translate status to |*out_code|
      *out_code = ECONNREFUSED;
      return ZX_OK;
    }

    uint32_t port = connect_result->local_port();

    auto inner_pipe_storage = std::make_unique<zxio_storage_t>();
    zxio_create(sock1.release(), inner_pipe_storage.get());

    state_ = ConnectedState{
        .inner_pipe_storage = std::move(inner_pipe_storage),
        .connection = std::move(connection_client),
        .port = port,
    };

    *out_code = 0;
    return ZX_OK;

  } else {
    *out_code = EISCONN;
    return ZX_OK;
  }
}

zx_status_t Socket::Listen(int backlog, int16_t* out_code) {
  std::lock_guard lock(state_lock_);
  if (auto state = std::get_if<BoundState>(&state_); state) {
    auto result = fidl::Call(state->listener)->Listen({static_cast<uint32_t>(backlog)});
    if (result.is_error()) {
      FX_LOGS(ERROR) << "vsock connector listen call failed: " << result.error_value();
      return ZX_ERR_INTERNAL;
    }
    state_ = ListeningState{
        .port = state->port,
        .listener = std::move(state->listener),
    };
    *out_code = 0;
    return ZX_OK;
  }
  // TODO(https://fxbug.dev/361410840): Check if this is the right error for the current state.
  *out_code = EADDRINUSE;
  return ZX_OK;
}

zx_status_t Socket::Accept(struct sockaddr* addr, socklen_t* addrlen, zxio_storage_t* out_storage,
                           int16_t* out_code) {
  zx::socket sock0, sock1;
  zx_status_t status = zx::socket::create(ZX_SOCKET_STREAM, &sock0, &sock1);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Could not create socket: " << zx_status_get_string(status);
    return ZX_ERR_INTERNAL;
  }
  auto [connection_client, connection_server] =
      fidl::Endpoints<fuchsia_vsock::Connection>::Create();
  fuchsia_vsock::ConnectionTransport transport{
      std::move(sock0),
      std::move(connection_server),
  };

  std::lock_guard lock(state_lock_);
  if (auto state = std::get_if<ListeningState>(&state_); state) {
    auto result = fidl::Call(state->listener)->Accept({std::move(transport)});
    if (result.is_error()) {
      if (result.error_value().is_domain_error() &&
          result.error_value().domain_error() == ZX_ERR_SHOULD_WAIT) {
        *out_code = EWOULDBLOCK;
      } else {
        // TODO(https://fxbug.dev/361410840): Check if this is the right error.
        *out_code = ECONNREFUSED;
      }
      return ZX_OK;
    }

    struct sockaddr_vm addr_vm{};
    addr_vm.svm_family = AF_VSOCK;
    // TODO(https://fxbug.dev/361410840): Double check which port and cid we are supposed to return
    // here. These are correct for the connected socket, but possibly not for addr that we return?
    addr_vm.svm_port = result->addr().remote_port();
    addr_vm.svm_cid = result->addr().remote_cid();

    if (addr != nullptr && addrlen != nullptr) {
      *addrlen = sizeof(struct sockaddr_vm);
      std::memcpy(addr, &addr_vm, sizeof(addr_vm));
    }

    new (out_storage) ZxioConnectedSocket(std::move(sock1), std::move(connection_client), addr_vm);

    *out_code = 0;
    return ZX_OK;
  }

  // TODO(https://fxbug.dev/361410840): Compute correct error code
  *out_code = EINVAL;
  return ZX_OK;
}

void Socket::WaitBegin(zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                       zx_signals_t* out_signals) {
  std::lock_guard lock(state_lock_);
  if (std::holds_alternative<UnconnectedState>(state_) ||
      std::holds_alternative<BoundState>(state_)) {
    // In this state we don't have any signals to wait on.
    *out_signals = ZX_SIGNAL_NONE;
  } else if (auto state = std::get_if<ListeningState>(&state_); state) {
    *out_handle = state->listener.channel().get();
    *out_signals = ZXIO_SIGNAL_PEER_CLOSED;
    if (zxio_signals & ZXIO_SIGNAL_READABLE) {
      // signal when a listening socket gets an incoming connection.
      *out_signals |= fuchsia_vsock::kSignalStreamIncoming;
    }
  } else if (auto state = std::get_if<ConnectedState>(&state_); state) {
    zxio_t* inner_pipe = reinterpret_cast<zxio_t*>(state->inner_pipe_storage.get());
    zxio_wait_begin(inner_pipe, zxio_signals, out_handle, out_signals);
  }
}
void Socket::WaitEnd(zx_signals_t signals, zxio_signals_t* out_zxio_signals) {
  std::lock_guard lock(state_lock_);
  if (std::holds_alternative<UnconnectedState>(state_) ||
      std::holds_alternative<BoundState>(state_)) {
    // In these states we don't generate any signals.
  } else if (std::holds_alternative<ListeningState>(state_)) {
    *out_zxio_signals = ZXIO_SIGNAL_NONE;
    if (signals & fuchsia_vsock::kSignalStreamIncoming) {
      *out_zxio_signals |= ZXIO_SIGNAL_READABLE;
    }
  } else if (auto state = std::get_if<ConnectedState>(&state_); state) {
    zxio_t* inner_pipe = reinterpret_cast<zxio_t*>(state->inner_pipe_storage.get());
    zxio_wait_end(inner_pipe, signals, out_zxio_signals);
  }
}

zx_status_t Socket::Readv(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                          size_t* out_actual) {
  std::lock_guard lock(state_lock_);
  if (auto state = std::get_if<ConnectedState>(&state_); state) {
    zxio_t* inner_pipe = reinterpret_cast<zxio_t*>(state->inner_pipe_storage.get());
    return zxio_readv(inner_pipe, vector, vector_count, flags, out_actual);
  }
  return ZX_ERR_BAD_STATE;
}

zx_status_t Socket::Writev(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                           size_t* out_actual) {
  std::lock_guard lock(state_lock_);
  if (auto state = std::get_if<ConnectedState>(&state_); state) {
    zxio_t* inner_pipe = reinterpret_cast<zxio_t*>(state->inner_pipe_storage.get());
    return zxio_writev(inner_pipe, vector, vector_count, flags, out_actual);
  }
  return ZX_ERR_BAD_STATE;
}

class ZxioSocket {
 public:
  explicit ZxioSocket(fidl::ClientEnd<fuchsia_vsock::Connector> connector_client);
  Socket& socket() { return socket_; }

 private:
  zxio_t io_;
  Socket socket_;
};

static_assert(std::is_standard_layout_v<ZxioSocket>);
static_assert(sizeof(ZxioSocket) <= sizeof(zxio_storage_t));

static Socket& to_socket(zxio_t* io) { return reinterpret_cast<ZxioSocket*>(io)->socket(); }

static constexpr zxio_ops_t kListenSocketOps = []() {
  zxio_ops_t ops = zxio_default_ops;
  ops.destroy = [](zxio_t* io) { reinterpret_cast<ZxioSocket*>(io)->~ZxioSocket(); };
  ops.bind = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return to_socket(io).Bind(addr, addrlen, out_code);
  };
  ops.listen = [](zxio_t* io, int backlog, int16_t* out_code) {
    return to_socket(io).Listen(backlog, out_code);
  };
  ops.accept = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                  zxio_storage_t* out_storage, int16_t* out_code) {
    return to_socket(io).Accept(addr, addrlen, out_storage, out_code);
  };
  ops.connect = [](zxio_t* io, const struct sockaddr* addr, socklen_t addrlen, int16_t* out_code) {
    return to_socket(io).Connect(addr, addrlen, out_code);
  };
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                      zxio_signals_t* out_signals) {
    to_socket(io).WaitBegin(zxio_signals, out_handle, out_signals);
  };
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    to_socket(io).WaitEnd(zx_signals, out_zxio_signals);
  };
  ops.readv = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                 size_t* out_actual) {
    return to_socket(io).Readv(vector, vector_count, flags, out_actual);
  };
  ops.writev = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                  size_t* out_actual) {
    return to_socket(io).Writev(vector, vector_count, flags, out_actual);
  };

  return ops;
}();

ZxioSocket::ZxioSocket(fidl::ClientEnd<fuchsia_vsock::Connector> connector_client)
    : socket_(std::move(connector_client)) {
  zxio_init(&io_, &kListenSocketOps);
}

zx::result<fbl::unique_fd> CreateVirtioStreamSocket() {
  zx::result client_end = component::Connect<fuchsia_vsock::Connector>();

  if (!client_end.is_ok()) {
    return client_end.take_error();
  }

  zxio_storage_t* listen_socket_storage = nullptr;
  fdio_t* listen_socket_fdio = fdio_zxio_create(&listen_socket_storage);
  if (listen_socket_fdio == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  new (listen_socket_storage) ZxioSocket(*std::move(client_end));

  return zx::ok(fbl::unique_fd(fdio_bind_to_fd(listen_socket_fdio, -1, 0)));
}

zx::result<uint32_t> GetLocalCid() {
  zx::result client_end = component::Connect<fuchsia_vsock::Connector>();

  if (!client_end.is_ok()) {
    return client_end.take_error();
  }

  fidl::Result result = fidl::Call(client_end.value())->GetCid();
  if (result.is_error()) {
    return zx::error(result.error_value().status());
  }

  return zx::ok(result->local_cid());
}

}  // namespace vsock

zx_status_t create_virtio_stream_socket(int* out_fd) {
  auto result = vsock::CreateVirtioStreamSocket();
  if (result.is_error()) {
    return result.status_value();
  }
  *out_fd = result->release();
  return ZX_OK;
}

zx_status_t get_local_cid(uint32_t* out_local_cid) {
  auto result = vsock::GetLocalCid();
  if (result.is_error()) {
    return result.status_value();
  }
  *out_local_cid = result.value();
  return ZX_OK;
}
