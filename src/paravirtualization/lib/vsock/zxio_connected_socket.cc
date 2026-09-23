// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/paravirtualization/lib/vsock/zxio_connected_socket.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/zxio.h>

#include <type_traits>

#include <fbl/inline_array.h>

namespace vsock {

static_assert(sizeof(ZxioConnectedSocket) <= sizeof(zxio_storage_t));
static_assert(std::is_standard_layout_v<ZxioConnectedSocket>);

namespace {
// NOTE: zxio_do_vector and zxio_stream_do_vector are copied from
// //sdk/lib/zxio/vector.h which is a private API.

// Calls the callback |fn| for every buffer in |vector| or until the callback
// returns |ZX_ERR_STOP|.
//
// **NOTE**: This method will return an error even if *some* progress has been
// made which is typically not what you want and does not match some POSIX APIs
// (generally on stream-like objects like Files, TCP sockets) which will return
// success for short reads and writes. If this method is used by such APIs, you
// most likely want to use |zxio_stream_do_vector| below, which will return
// partial success.
//
// TODO(https://fxbug.dev/377534441): This function is not thread-safe! Multithreaded reads/writes
// can become interleaved with each other.
template <typename F>
zx_status_t zxio_do_vector(const zx_iovec_t* vector, size_t vector_count, size_t* out_actual,
                           F fn) {
  size_t total = 0;
  for (size_t i = 0; i < vector_count; ++i) {
    size_t actual = 0;
    zx_status_t status = fn(vector[i].buffer, vector[i].capacity, total, &actual);
    switch (status) {
      case ZX_OK:
      case ZX_ERR_NEXT:
        total += actual;
        if (actual == vector[i].capacity) {
          continue;
        }
        // Short read.
        break;
      case ZX_ERR_STOP:
        total += actual;
        break;
      default:
        return status;
    }

    break;
  }
  *out_actual = total;
  return ZX_OK;
}

// This method is like |zxio_do_vector| except it implements stream semantics.
// That is, this method will swallow the error returned by |fn|, stop iterating
// the buffers and return |ZX_OK| as long as a previous call to |fn| successfully
// performed some I/O on at least one byte. If |fn| returns an error before any
// I/O was performed (no bytes were read/written), then that error is returned
// to the caller.
//
// TODO(https://fxbug.dev/377534441): This function is not thread-safe! Multithreaded reads/writes
// can become interleaved with each other.
template <typename F>
zx_status_t zxio_stream_do_vector(const zx_iovec_t* vector, size_t vector_count, size_t* out_actual,
                                  F fn) {
  return zxio_do_vector(
      vector, vector_count, out_actual,
      [&fn](void* buffer, size_t capacity, size_t total_so_far, size_t* out_actual) {
        zx_status_t status = fn(buffer, capacity, out_actual);
        if (status != ZX_OK) {
          if (total_so_far == 0) {
            return status;
          }
          return ZX_ERR_STOP;
        }
        return ZX_ERR_NEXT;
      });
}

ZxioConnectedSocket& connected_socket(zxio_t* io) {
  return *reinterpret_cast<ZxioConnectedSocket*>(io);
}

constexpr zxio_ops_t kZxioConnectedSocketOps = []() {
  zxio_ops_t ops = zxio_default_ops;

  ops.destroy = [](zxio_t* io) { connected_socket(io).~ZxioConnectedSocket(); };

  // Mostly copied from zxio/pipe.cc
  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                      zx_signals_t* out_zx_signals) {
    *out_handle = connected_socket(io).socket().get();

    zx_signals_t zx_signals = ZX_SIGNAL_NONE;
    if (zxio_signals & ZXIO_SIGNAL_READABLE) {
      zx_signals |= ZX_SOCKET_READABLE;
    }
    if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
      zx_signals |= ZX_SOCKET_WRITABLE;
    }
    if (zxio_signals & ZXIO_SIGNAL_READ_DISABLED) {
      zx_signals |= ZX_SOCKET_PEER_WRITE_DISABLED;
    }
    if (zxio_signals & ZXIO_SIGNAL_WRITE_DISABLED) {
      zx_signals |= ZX_SOCKET_WRITE_DISABLED;
    }
    if (zxio_signals & ZXIO_SIGNAL_READ_THRESHOLD) {
      zx_signals |= ZX_SOCKET_READ_THRESHOLD;
    }
    if (zxio_signals & ZXIO_SIGNAL_WRITE_THRESHOLD) {
      zx_signals |= ZX_SOCKET_WRITE_THRESHOLD;
    }
    if (zxio_signals & ZXIO_SIGNAL_PEER_CLOSED) {
      zx_signals |= ZX_SOCKET_PEER_CLOSED;
    }
    *out_zx_signals = zx_signals;
  };

  // Copied from zxio/pipe.cc
  ops.wait_end = [](zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    zxio_signals_t zxio_signals = ZXIO_SIGNAL_NONE;
    if (zx_signals & ZX_SOCKET_READABLE) {
      zxio_signals |= ZXIO_SIGNAL_READABLE;
    }
    if (zx_signals & ZX_SOCKET_WRITABLE) {
      zxio_signals |= ZXIO_SIGNAL_WRITABLE;
    }
    if (zx_signals & ZX_SOCKET_PEER_WRITE_DISABLED) {
      zxio_signals |= ZXIO_SIGNAL_READ_DISABLED;
    }
    if (zx_signals & ZX_SOCKET_WRITE_DISABLED) {
      zxio_signals |= ZXIO_SIGNAL_WRITE_DISABLED;
    }
    if (zx_signals & ZX_SOCKET_READ_THRESHOLD) {
      zxio_signals |= ZXIO_SIGNAL_READ_THRESHOLD;
    }
    if (zx_signals & ZX_SOCKET_WRITE_THRESHOLD) {
      zxio_signals |= ZXIO_SIGNAL_WRITE_THRESHOLD;
    }
    if (zx_signals & ZX_SOCKET_PEER_CLOSED) {
      zxio_signals |= ZXIO_SIGNAL_PEER_CLOSED;
    }
    *out_zxio_signals = zxio_signals;
  };

  // Mostly copied from zxio/pipe.cc
  ops.get_read_buffer_available = [](zxio_t* io, size_t* out_available) {
    if (out_available == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    zx_info_socket_t info;
    memset(&info, 0, sizeof(info));
    zx_status_t status = connected_socket(io).socket().get_info(ZX_INFO_SOCKET, &info, sizeof(info),
                                                                nullptr, nullptr);
    if (status != ZX_OK) {
      return status;
    }
    *out_available = info.rx_buf_available;
    return ZX_OK;
  };

  // Mostly copied from zxio/pipe.cc
  ops.shutdown = [](zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
    if ((options & ZXIO_SHUTDOWN_OPTIONS_MASK) != options) {
      return ZX_ERR_INVALID_ARGS;
    }
    uint32_t disposition = 0;
    if (options & ZXIO_SHUTDOWN_OPTIONS_WRITE) {
      disposition = ZX_SOCKET_DISPOSITION_WRITE_DISABLED;
    }
    uint32_t disposition_peer = 0;
    if (options & ZXIO_SHUTDOWN_OPTIONS_READ) {
      disposition_peer = ZX_SOCKET_DISPOSITION_WRITE_DISABLED;
    }
    *out_code = 0;
    return connected_socket(io).socket().set_disposition(disposition, disposition_peer);
  };

  ops.getpeername = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    const auto& peer_addr = connected_socket(io).peer_addr();
    if (addr != nullptr) {
      std::memcpy(addr, &peer_addr, sizeof(peer_addr));
    }
    if (addrlen != nullptr) {
      *addrlen = sizeof(peer_addr);
    }
    *out_code = 0;
    return ZX_OK;
  };

  // Mostly copied from zxio/pipe.cc's zxio_stream_pipe_ops
  ops.readv = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                 size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    zx::socket& socket = connected_socket(io).socket();

    return zxio_stream_do_vector(vector, vector_count, out_actual,
                                 [&](void* buffer, size_t capacity, size_t* out_actual) {
                                   return socket.read(0, buffer, capacity, out_actual);
                                 });
  };

  // Copied from zxio/pipe.cc's zxio_stream_pipe_ops
  ops.writev = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                  size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return zxio_stream_do_vector(
        vector, vector_count, out_actual, [&](void* buffer, size_t capacity, size_t* out_actual) {
          return connected_socket(io).socket().write(0, buffer, capacity, out_actual);
        });
  };

  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    // Copied from zxio/transitional.cc
    zxio_flags_t zxio_flags = 0;
    if (flags & MSG_PEEK) {
      zxio_flags |= ZXIO_PEEK;
      flags &= ~MSG_PEEK;
    }
    if (flags) {
      // TODO(https://fxbug.dev/42146893): support MSG_OOB
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (msg->msg_iovlen > IOV_MAX) {
      return ZX_ERR_INVALID_ARGS;
    }

    fbl::AllocChecker ac;
    fbl::InlineArray<zx_iovec_t, 1> zx_iov(&ac, msg->msg_iovlen);
    ac.check();

    for (int i = 0; i < msg->msg_iovlen; ++i) {
      iovec const& iov = msg->msg_iov[i];
      zx_iov[i] = {
          .buffer = iov.iov_base,
          .capacity = iov.iov_len,
      };
    }

    zx_status_t status = zxio_readv(io, zx_iov.get(), zx_iov.size(), zxio_flags, out_actual);
    if (status != ZX_OK) {
      return status;
    }

    *out_code = 0;
    return ZX_OK;
  };

  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    // Copied from zxio/transitional.cc
    if (flags) {
      // TODO(https://fxbug.dev/42146893): support MSG_OOB
      return ZX_ERR_NOT_SUPPORTED;
    }

    if (msg->msg_iovlen > IOV_MAX) {
      return ZX_ERR_INVALID_ARGS;
    }

    fbl::AllocChecker ac;
    fbl::InlineArray<zx_iovec_t, 1> zx_iov(&ac, msg->msg_iovlen);
    ac.check();

    for (int i = 0; i < msg->msg_iovlen; ++i) {
      zx_iov[i] = {
          .buffer = msg->msg_iov[i].iov_base,
          .capacity = msg->msg_iov[i].iov_len,
      };
    }
    zx_status_t status = zxio_writev(io, zx_iov.get(), zx_iov.size(), 0, out_actual);
    if (status != ZX_OK) {
      return status;
    }
    *out_code = 0;
    return ZX_OK;
  };

  return ops;
}();

}  // namespace

ZxioConnectedSocket::ZxioConnectedSocket(
    zx::socket socket, fidl::ClientEnd<fuchsia_vsock::Connection> connection_client_end,
    struct sockaddr_vm& peer_addr)
    : socket_(std::move(socket)),
      connection_client_end_(std::move(connection_client_end)),
      peer_addr_(std::make_unique<struct sockaddr_vm>(peer_addr)) {
  zxio_init(&io_, &kZxioConnectedSocketOps);
}

}  // namespace vsock
