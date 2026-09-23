// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zxio/cpp/transitional.h>
#include <lib/zxio/fault_catcher.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <sys/stat.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/object.h>

#include <algorithm>

#include "sdk/lib/zxio/private.h"
#include "sdk/lib/zxio/vector.h"

namespace {
template <typename T>
zx_status_t set_sockopt_value(int16_t* out_code, void* output, socklen_t* output_size, T* input) {
  if (*output_size < sizeof(T)) {
    *out_code = EINVAL;
    return ZX_ERR_INVALID_ARGS;
  }
  memcpy(output, input, sizeof(T));
  *output_size = sizeof(T);
  *out_code = 0;
  return ZX_OK;
}

zx_status_t set_sockopt_value_int(int16_t* out_code, void* output, socklen_t* output_size,
                                  uint32_t input) {
  return set_sockopt_value(out_code, output, output_size, &input);
}

// Partial implementation of getsockopt for zx::Socket interpreted as Unix
// domain socket.
zx_status_t getsockopt(uint32_t so_type, zxio_t* io, int level, int optname, void* optval,
                       socklen_t* optlen, int16_t* out_code) {
  if (optval == nullptr || optlen == nullptr) {
    *out_code = EFAULT;
    return ZX_ERR_INVALID_ARGS;
  }
  if (*optlen < sizeof(uint32_t) || level != SOL_SOCKET) {
    *out_code = EINVAL;
    return ZX_ERR_INVALID_ARGS;
  }
  switch (optname) {
    case SO_DOMAIN:
      return set_sockopt_value_int(out_code, optval, optlen, AF_UNIX);
    case SO_TYPE:
      return set_sockopt_value_int(out_code, optval, optlen, so_type);
    case SO_PROTOCOL:
      return set_sockopt_value_int(out_code, optval, optlen, 0);
    case SO_LINGER: {
      struct linger response{};
      response.l_onoff = 0;
      response.l_linger = 0;
      return set_sockopt_value(out_code, optval, optlen, &response);
    }
    default:
      *out_code = EINVAL;
      return ZX_ERR_INVALID_ARGS;
  }
}
}  // namespace

static zxio_pipe_t& zxio_get_pipe(zxio_t* io) { return *reinterpret_cast<zxio_pipe_t*>(io); }

static constexpr zxio_ops_t zxio_pipe_ops = []() {
  zxio_ops_t ops = zxio_default_ops;
  ops.destroy = [](zxio_t* io) { zxio_get_pipe(io).~zxio_pipe_t(); };

  ops.release = [](zxio_t* io, zx_handle_t* out_handle) {
    *out_handle = zxio_get_pipe(io).socket.release();
    return ZX_OK;
  };

  ops.clone = [](zxio_t* io, zx_handle_t* out_handle) {
    zx::socket out_socket;
    zx_status_t status = zxio_get_pipe(io).socket.duplicate(ZX_RIGHT_SAME_RIGHTS, &out_socket);
    if (status != ZX_OK) {
      return status;
    }
    *out_handle = out_socket.release();
    return ZX_OK;
  };

  ops.attr_get = [](zxio_t* io, zxio_node_attributes_t* inout_attr) {
    if (inout_attr->has.abilities) {
      ZXIO_NODE_ATTR_SET(
          *inout_attr, abilities,
          ZXIO_OPERATION_READ_BYTES | ZXIO_OPERATION_WRITE_BYTES | ZXIO_OPERATION_GET_ATTRIBUTES);
    }
    if (inout_attr->has.object_type) {
      ZXIO_NODE_ATTR_SET(*inout_attr, object_type, ZXIO_OBJECT_TYPE_PIPE);
    }
    return ZX_OK;
  };

  ops.wait_begin = [](zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                      zx_signals_t* out_zx_signals) {
    *out_handle = zxio_get_pipe(io).socket.get();

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

  ops.get_read_buffer_available = [](zxio_t* io, size_t* out_available) {
    if (out_available == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    zx_info_socket_t info;
    memset(&info, 0, sizeof(info));
    zx_status_t status =
        zxio_get_pipe(io).socket.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr);
    if (status != ZX_OK) {
      return status;
    }
    *out_available = info.rx_buf_available;
    return ZX_OK;
  };

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
    return zxio_get_pipe(io).socket.set_disposition(disposition, disposition_peer);
  };

  ops.recvmsg = [](zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    *out_code = 0;
    return zxio_recvmsg_inner(io, msg, flags, out_actual);
  };

  ops.sendmsg = [](zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                   int16_t* out_code) {
    *out_code = 0;
    return zxio_sendmsg_inner(io, msg, flags, out_actual);
  };

  ops.getsockname = [](zxio_t* io, struct sockaddr* addr, socklen_t* addrlen, int16_t* out_code) {
    if (addrlen == nullptr || (*addrlen != 0 && addr == nullptr)) {
      *out_code = EFAULT;
      return ZX_OK;
    }
    struct sockaddr response;
    response.sa_family = AF_UNIX;
    const socklen_t response_size = sizeof(sa_family_t);
    memcpy(addr, &response, std::min(*addrlen, response_size));
    *addrlen = response_size;
    *out_code = 0;
    return ZX_OK;
  };

  ops.getpeername = ops.getsockname;

  ops.flags_get = [](zxio_t* io, uint64_t* out_flags) {
    zx_info_handle_basic_t info;
    zx_status_t status = zxio_get_pipe(io).socket.get_info(ZX_INFO_HANDLE_BASIC, &info,
                                                           sizeof(info), nullptr, nullptr);
    if (status != ZX_OK) {
      return status;
    }
    ZX_ASSERT(info.type == ZX_OBJ_TYPE_SOCKET);
    fuchsia_io::wire::Flags flags{};
    if (info.rights & ZX_RIGHT_READ) {
      flags |= fuchsia_io::wire::Flags::kPermReadBytes;
    }
    if (info.rights & ZX_RIGHT_WRITE) {
      flags |= fuchsia_io::wire::Flags::kPermWriteBytes;
    }
    *out_flags = static_cast<uint64_t>(flags);
    return ZX_OK;
  };

  ops.flags_set = [](zxio_t* io, uint64_t flags) {
    zx_info_handle_basic_t info;
    zx_status_t status = zxio_get_pipe(io).socket.get_info(ZX_INFO_HANDLE_BASIC, &info,
                                                           sizeof(info), nullptr, nullptr);
    if (status != ZX_OK) {
      return status;
    }
    // Ensure that the supported flags (readable, writeable) match the rights on the socket.
    const bool set_readable =
        flags & static_cast<uint64_t>(fuchsia_io::wire::Flags::kPermReadBytes);
    const bool set_writable =
        flags & static_cast<uint64_t>(fuchsia_io::wire::Flags::kPermWriteBytes);
    const bool is_readable = info.rights & ZX_RIGHT_READ;
    const bool is_writable = info.rights & ZX_RIGHT_WRITE;
    if ((set_readable != is_readable) || (set_writable != is_writable)) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    return ZX_OK;
  };

  return ops;
}();

static constexpr zxio_ops_t zxio_datagram_pipe_ops = []() {
  zxio_ops_t ops = zxio_pipe_ops;
  ops.readv = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                 size_t* out_actual) {
    uint32_t zx_flags = 0;
    if (flags & ZXIO_PEEK) {
      zx_flags |= ZX_SOCKET_PEEK;
      flags &= ~ZXIO_PEEK;
    }
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    size_t total = 0;
    for (size_t i = 0; i < vector_count; ++i) {
      total += vector[i].capacity;
    }
    std::unique_ptr<uint8_t[]> buf(new uint8_t[total]);

    size_t actual;
    zx_status_t status = zxio_get_pipe(io).socket.read(zx_flags, buf.get(), total, &actual);
    if (status != ZX_OK) {
      return status;
    }

    uint8_t* data = buf.get();
    size_t remaining = actual;
    return zxio_do_vector(
        vector, vector_count, out_actual,
        [&](void* buffer, size_t capacity, size_t total_so_far, size_t* out_actual) {
          size_t actual = std::min(capacity, remaining);
          if (unlikely(!zxio_maybe_faultable_copy(reinterpret_cast<uint8_t*>(buffer), data, actual,
                                                  true))) {
            return ZX_ERR_INVALID_ARGS;
          }
          data += actual;
          remaining -= actual;
          *out_actual = actual;
          return ZX_OK;
        });
  };

  ops.writev = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                  size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    size_t total = 0;
    for (size_t i = 0; i < vector_count; ++i) {
      total += vector[i].capacity;
    }
    std::unique_ptr<uint8_t[]> buf(new uint8_t[total]);

    uint8_t* data = buf.get();
    for (size_t i = 0; i < vector_count; ++i) {
      if (unlikely(!zxio_maybe_faultable_copy(data,
                                              reinterpret_cast<const uint8_t*>(vector[i].buffer),
                                              vector[i].capacity, false))) {
        return ZX_ERR_INVALID_ARGS;
      }
      data += vector[i].capacity;
    }

    return zxio_get_pipe(io).socket.write(0, buf.get(), total, out_actual);
  };

  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    return getsockopt(SOCK_DGRAM, io, level, optname, optval, optlen, out_code);
  };

  return ops;
}();

static constexpr zxio_ops_t zxio_stream_pipe_ops = []() {
  zxio_ops_t ops = zxio_pipe_ops;
  ops.readv = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                 size_t* out_actual) {
    if (flags & ZXIO_PEEK) {
      return zxio_datagram_pipe_ops.readv(io, vector, vector_count, flags, out_actual);
    }
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    zx::socket& socket = zxio_get_pipe(io).socket;

    return zxio_stream_do_vector(vector, vector_count, out_actual,
                                 [&](void* buffer, size_t capacity, size_t* out_actual) {
                                   return socket.read(0, buffer, capacity, out_actual);
                                 });
  };

  ops.writev = [](zxio_t* io, const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                  size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return zxio_stream_do_vector(
        vector, vector_count, out_actual, [&](void* buffer, size_t capacity, size_t* out_actual) {
          return zxio_get_pipe(io).socket.write(0, buffer, capacity, out_actual);
        });
  };

  ops.getsockopt = [](zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                      int16_t* out_code) {
    return getsockopt(SOCK_STREAM, io, level, optname, optval, optlen, out_code);
  };
  return ops;
}();

zx_status_t zxio_pipe_init(zxio_storage_t* storage, zx::socket socket, zx_info_socket_t info) {
  auto pipe = new (storage) zxio_pipe_t{
      .io = storage->io,
      .socket = std::move(socket),
  };
  zxio_init(&pipe->io,
            info.options & ZX_SOCKET_DATAGRAM ? &zxio_datagram_pipe_ops : &zxio_stream_pipe_ops);
  return ZX_OK;
}
