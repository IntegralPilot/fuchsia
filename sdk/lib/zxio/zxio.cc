// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.net.name/cpp/wire.h>
#include <fidl/fuchsia.net.resources/cpp/wire.h>
#include <fidl/fuchsia.net/cpp/wire.h>
#include <fidl/fuchsia.posix.socket.packet/cpp/wire.h>
#include <fidl/fuchsia.posix.socket.raw/cpp/wire.h>
#include <fidl/fuchsia.posix.socket/cpp/wire.h>
#include <ifaddrs.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/envelope.h>
#include <lib/zx/channel.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/result.h>
#include <lib/zxio/bsdsocket.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <lib/zxio/watcher.h>
#include <lib/zxio/zxio.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <optional>
#include <type_traits>
#include <variant>

#include <netpacket/packet.h>

#include "private.h"

namespace fnet = fuchsia_net;
namespace fsocket = fuchsia_posix_socket;
namespace frawsocket = fuchsia_posix_socket_raw;
namespace fpacketsocket = fuchsia_posix_socket_packet;

// The private fields of a |zxio_t| object.
//
// In |ops.h|, the |zxio_t| struct is defined as opaque. Clients of the zxio
// library are forbidden from relying upon the structure of |zxio_t| objects.
// To avoid temptation, the details of the structure are defined only in this
// implementation file and are not visible in the header.
using zxio_internal_t = struct zxio_internal {
  explicit zxio_internal(const zxio_ops_t* ops) : ops(ops) {}

  const zxio_ops_t* ops;

  // When adding fields to the |zxio_internal_t| struct, one may take from
  // the reserved bytes here to ensure that the ABI stays compatible.
  uint64_t reserved[3];
};

static_assert(sizeof(zxio_t) == sizeof(zxio_internal_t), "zxio_t should match zxio_internal_t");

// Converters from the public (opaque) types to the internal (implementation) types.
namespace {

zxio_internal_t* to_internal(zxio_t* io) { return reinterpret_cast<zxio_internal_t*>(io); }

const zxio_internal_t* to_internal(const zxio_t* io) {
  return reinterpret_cast<const zxio_internal_t*>(io);
}

static const zxio_ops_t* const kPoisoned = reinterpret_cast<zxio_ops_t*>(1);

}  // namespace

bool zxio_is_valid(const zxio_t* io) {
  if (io == nullptr) {
    return false;
  }
  const zxio_internal_t* zio = to_internal(io);
  return zio->ops != nullptr && zio->ops != kPoisoned;
}

void zxio_init(zxio_t* io, const zxio_ops_t* ops) { new (io) zxio_internal_t(ops); }

const zxio_ops_t* zxio_get_ops(zxio_t* io) {
  const zxio_internal_t* zio = to_internal(io);
  return zio->ops;
}

void zxio_destroy(zxio_t* io) {
  if (!zxio_is_valid(io)) {
    // Double destruction is undefined, but we allow ops to be nullptr, in which case we just
    // ignore.
    ZX_ASSERT(zxio_get_ops(io) != kPoisoned);
    return;
  }
  // We poison the object by creating a new object with ops set to kPoisoned. That will only work if
  // zxio_internal_t has a trivial destructor.
  static_assert(std::is_trivially_destructible<zxio_internal_t>::value,
                "zxio_internal_t must have trivial destructor");
  zxio_internal_t* zio = to_internal(io);
  zio->ops->destroy(io);
  // Poison the object. Double destruction is undefined behavior.
  zxio_init(io, kPoisoned);
}

zx_status_t zxio_close(zxio_t* io) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->close(io);
}

zx_status_t zxio_release(zxio_t* io, zx_handle_t* out_handle) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->release(io, out_handle);
}

zx_status_t zxio_borrow(zxio_t* io, zx_handle_t* out_handle) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->borrow(io, out_handle);
}

zx_status_t zxio_clone(zxio_t* io, zx_handle_t* out_handle) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->clone(io, out_handle);
}

zx_status_t zxio_wait_one(zxio_t* io, zxio_signals_t signals, zx_time_t deadline,
                          zxio_signals_t* out_observed) {
  if (!zxio_is_valid(io)) {
    *out_observed = ZXIO_SIGNAL_NONE;
    return ZX_ERR_BAD_HANDLE;
  }
  zx_handle_t handle = ZX_HANDLE_INVALID;
  zx_signals_t zx_signals = ZX_SIGNAL_NONE;
  zxio_wait_begin(io, signals, &handle, &zx_signals);
  if (handle == ZX_HANDLE_INVALID) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  zx_signals_t zx_observed = ZX_SIGNAL_NONE;
  zx_status_t status = zx_object_wait_one(handle, zx_signals, deadline, &zx_observed);
  if (status != ZX_OK) {
    return status;
  }
  zxio_wait_end(io, zx_signals, out_observed);
  return ZX_OK;
}

void zxio_wait_begin(zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                     zx_signals_t* out_zx_signals) {
  if (!zxio_is_valid(io)) {
    *out_handle = ZX_HANDLE_INVALID;
    *out_zx_signals = ZX_SIGNAL_NONE;
    return;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->wait_begin(io, zxio_signals, out_handle, out_zx_signals);
}

void zxio_wait_end(zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
  if (!zxio_is_valid(io)) {
    *out_zxio_signals = ZXIO_SIGNAL_NONE;
    return;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->wait_end(io, zx_signals, out_zxio_signals);
}

zx_status_t zxio_sync(zxio_t* io) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->sync(io);
}

zx_status_t zxio_attr_get(zxio_t* io, zxio_node_attributes_t* inout_attr) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (inout_attr->has.fsverity_root_hash && inout_attr->fsverity_root_hash == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->attr_get(io, inout_attr);
}

zx_status_t zxio_attr_set(zxio_t* io, const zxio_node_attributes_t* attr) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->attr_set(io, attr);
}

zx_status_t zxio_enable_verity(zxio_t* io, const zxio_fsverity_descriptor_t* descriptor) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->enable_verity(io, descriptor);
}

zx_status_t zxio_read(zxio_t* io, void* buffer, size_t capacity, zxio_flags_t flags,
                      size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  const zx_iovec_t vector = {
      .buffer = buffer,
      .capacity = capacity,
  };
  return zxio_readv(io, &vector, 1, flags, out_actual);
}

zx_status_t zxio_read_at(zxio_t* io, zx_off_t offset, void* buffer, size_t capacity,
                         zxio_flags_t flags, size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  const zx_iovec_t vector = {
      .buffer = buffer,
      .capacity = capacity,
  };
  return zxio_readv_at(io, offset, &vector, 1, flags, out_actual);
}

zx_status_t zxio_write(zxio_t* io, const void* buffer, size_t capacity, zxio_flags_t flags,
                       size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  const zx_iovec_t vector = {
      .buffer = const_cast<void*>(buffer),
      .capacity = capacity,
  };
  return zxio_writev(io, &vector, 1, flags, out_actual);
}

zx_status_t zxio_write_at(zxio_t* io, zx_off_t offset, const void* buffer, size_t capacity,
                          zxio_flags_t flags, size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  const zx_iovec_t vector = {
      .buffer = const_cast<void*>(buffer),
      .capacity = capacity,
  };
  return zxio_writev_at(io, offset, &vector, 1, flags, out_actual);
}

zx_status_t zxio_readv(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                       zxio_flags_t flags, size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->readv(io, vector, vector_count, flags, out_actual);
}

zx_status_t zxio_readv_at(zxio_t* io, zx_off_t offset, const zx_iovec_t* vector,
                          size_t vector_count, zxio_flags_t flags, size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->readv_at(io, offset, vector, vector_count, flags, out_actual);
}

zx_status_t zxio_writev(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                        zxio_flags_t flags, size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->writev(io, vector, vector_count, flags, out_actual);
}

zx_status_t zxio_writev_at(zxio_t* io, zx_off_t offset, const zx_iovec_t* vector,
                           size_t vector_count, zxio_flags_t flags, size_t* out_actual) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->writev_at(io, offset, vector, vector_count, flags, out_actual);
}

static_assert(ZX_STREAM_SEEK_ORIGIN_START == ZXIO_SEEK_ORIGIN_START, "ZXIO should match ZX");
static_assert(ZX_STREAM_SEEK_ORIGIN_CURRENT == ZXIO_SEEK_ORIGIN_CURRENT, "ZXIO should match ZX");
static_assert(ZX_STREAM_SEEK_ORIGIN_END == ZXIO_SEEK_ORIGIN_END, "ZXIO should match ZX");

zx_status_t zxio_seek(zxio_t* io, zxio_seek_origin_t start, int64_t offset, size_t* out_offset) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->seek(io, start, offset, out_offset);
}

zx_status_t zxio_truncate(zxio_t* io, uint64_t length) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->truncate(io, length);
}

zx_status_t zxio_flags_get(zxio_t* io, uint64_t* out_flags) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->flags_get(io, out_flags);
}

zx_status_t zxio_flags_set(zxio_t* io, uint64_t flags) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->flags_set(io, flags);
}

zx_status_t zxio_token_get(zxio_t* io, zx_handle_t* out_token) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->token_get(io, out_token);
}

zx_status_t zxio_vmo_get(zxio_t* io, zxio_vmo_flags_t flags, zx_handle_t* out_vmo) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->vmo_get(io, flags, out_vmo);
}

zx_status_t zxio_on_mapped(zxio_t* io, void* ptr) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->on_mapped(io, ptr);
}

zx_status_t zxio_get_read_buffer_available(zxio_t* io, size_t* out_available) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->get_read_buffer_available(io, out_available);
}

zx_status_t zxio_shutdown(zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->shutdown(io, options, out_code);
}

zx_status_t zxio_open(zxio_t* directory, const char* path, size_t path_len, zxio_open_flags_t flags,
                      const zxio_open_options_t* options, zxio_storage_t* storage) {
  if (!zxio_is_valid(directory)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(directory);
  return zio->ops->open(directory, path, path_len, flags, options, storage);
}

zx_status_t zxio_unlink(zxio_t* directory, const char* name, size_t name_len, int flags) {
  if (!zxio_is_valid(directory)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(directory);
  return zio->ops->unlink(directory, name, name_len, flags);
}

zx_status_t zxio_rename(zxio_t* old_directory, const char* old_path, size_t old_path_len,
                        zx_handle_t new_directory_token, const char* new_path,
                        size_t new_path_len) {
  if (!zxio_is_valid(old_directory)) {
    zx_handle_close(new_directory_token);
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(old_directory);
  return zio->ops->rename(old_directory, old_path, old_path_len, new_directory_token, new_path,
                          new_path_len);
}

zx_status_t zxio_link(zxio_t* src_directory, const char* src_path, size_t src_path_len,
                      zx_handle_t dst_directory_token, const char* dst_path, size_t dst_path_len) {
  if (!zxio_is_valid(src_directory)) {
    zx_handle_close(dst_directory_token);
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(src_directory);
  return zio->ops->link(src_directory, src_path, src_path_len, dst_directory_token, dst_path,
                        dst_path_len);
}

zx_status_t zxio_link_into(zxio_t* object, zx_handle_t dst_directory_token, const char* dst_path,
                           size_t dst_path_len) {
  if (!zxio_is_valid(object)) {
    zx_handle_close(dst_directory_token);
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(object);
  return zio->ops->link_into(object, dst_directory_token, dst_path, dst_path_len);
}

zx_status_t zxio_dirent_iterator_init(zxio_dirent_iterator_t* iterator, zxio_t* directory) {
  if (!zxio_is_valid(directory)) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (iterator == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  zxio_internal_t* zio = to_internal(directory);
  return zio->ops->dirent_iterator_init(directory, iterator);
}

zx_status_t zxio_dirent_iterator_next(zxio_dirent_iterator_t* iterator,
                                      zxio_dirent_t* inout_entry) {
  if (!zxio_is_valid(iterator->io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (inout_entry == nullptr || inout_entry->name == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  zxio_internal_t* zio = to_internal(iterator->io);
  return zio->ops->dirent_iterator_next(iterator->io, iterator, inout_entry);
}

zx_status_t zxio_dirent_iterator_rewind(zxio_dirent_iterator_t* iterator) {
  if (!zxio_is_valid(iterator->io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(iterator->io);
  return zio->ops->dirent_iterator_rewind(iterator->io, iterator);
}

void zxio_dirent_iterator_destroy(zxio_dirent_iterator_t* iterator) {
  if (!zxio_is_valid(iterator->io)) {
    return;
  }
  zxio_internal_t* zio = to_internal(iterator->io);
  zio->ops->dirent_iterator_destroy(iterator->io, iterator);
}

zx_status_t zxio_isatty(zxio_t* io, bool* tty) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->isatty(io, tty);
}

zx_status_t zxio_get_window_size(zxio_t* io, uint32_t* width, uint32_t* height) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (width == nullptr || height == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->get_window_size(io, width, height);
}

zx_status_t zxio_set_window_size(zxio_t* io, uint32_t width, uint32_t height) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->set_window_size(io, width, height);
}

zx_status_t zxio_ioctl(zxio_t* io, int request, int16_t* out_code, va_list va) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }

  zxio_internal_t* zio = to_internal(io);
  return zio->ops->ioctl(io, request, out_code, va);
}

zx_status_t zxio_watch_directory(zxio_t* directory, zxio_watch_directory_cb cb, zx_time_t deadline,
                                 void* context) {
  if (!zxio_is_valid(directory)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(directory);
  return zio->ops->watch_directory(directory, cb, deadline, context);
}

zx_status_t zxio_bind(zxio_t* io, const struct sockaddr* addr, socklen_t addrlen,
                      int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->bind(io, addr, addrlen, out_code);
}

zx_status_t zxio_connect(zxio_t* io, const struct sockaddr* addr, socklen_t addrlen,
                         int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->connect(io, addr, addrlen, out_code);
}

zx_status_t zxio_listen(zxio_t* io, int backlog, int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->listen(io, backlog, out_code);
}

zx_status_t zxio_accept(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                        zxio_storage_t* out_storage, int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->accept(io, addr, addrlen, out_storage, out_code);
}

zx_status_t zxio_getsockname(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                             int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->getsockname(io, addr, addrlen, out_code);
}

zx_status_t zxio_getpeername(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                             int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->getpeername(io, addr, addrlen, out_code);
}

zx_status_t zxio_getsockopt(zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                            int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }

  zxio_internal_t* zio = to_internal(io);
  return zio->ops->getsockopt(io, level, optname, optval, optlen, out_code);
}

zx_status_t zxio_setsockopt(zxio_t* io, int level, int optname, const void* optval,
                            socklen_t optlen, zx_handle_t access_token, int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    zx_handle_close(access_token);
    return ZX_ERR_BAD_HANDLE;
  }

  zxio_internal_t* zio = to_internal(io);
  return zio->ops->setsockopt(io, level, optname, optval, optlen, access_token, out_code);
}

zx_status_t zxio_recvmsg(zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                         int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }

  zxio_internal_t* zio = to_internal(io);
  return zio->ops->recvmsg(io, msg, flags, out_actual, out_code);
}

zx_status_t zxio_sendmsg(zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                         int16_t* out_code) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }

  zxio_internal_t* zio = to_internal(io);
  return zio->ops->sendmsg(io, msg, flags, out_actual, out_code);
}

template <typename T>
zx::result<fidl::UnownedClientEnd<T>> connect_socket_provider(
    zxio_service_connector service_connector) {
  zx_handle_t socket_provider_handle;
  zx_status_t status =
      service_connector(fidl::DiscoverableProtocolName<T>, &socket_provider_handle);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(fidl::UnownedClientEnd<T>(zx::unowned_channel(socket_provider_handle)));
}

namespace {
struct socket_creation_options {
  cpp20::span<zxio_socket_mark_t> marks;
  std::optional<zx::event> wake_group;
  socket_creation_options() = default;

  explicit socket_creation_options(zxio_socket_creation_options_t opts)
      : marks(opts.marks, opts.num_marks) {
    if (opts.wake_group != ZX_HANDLE_INVALID) {
      wake_group = zx::event(opts.wake_group);
    }
  }
};

// A class template to extract the client end from the FIDL response.
template <typename FidlMethod>
class WireResponse {};

template <>
struct WireResponse<fsocket::Provider::StreamSocket> {
  using ClientEnd = fidl::ClientEnd<fsocket::StreamSocket>;

  static ClientEnd FromResponse(fsocket::wire::ProviderStreamSocketResponse* response) {
    return std::move(response->s);
  }
};

template <>
struct WireResponse<fsocket::Provider::DatagramSocket> {
  using ClientEnd = std::variant<fidl::ClientEnd<fsocket::DatagramSocket>,
                                 fidl::ClientEnd<fsocket::SynchronousDatagramSocket>, zx_status_t>;

  static ClientEnd FromResponse(fsocket::wire::ProviderDatagramSocketResponse* response) {
    if (response->is_datagram_socket()) {
      return std::move(response->datagram_socket());
    }
    if (response->is_synchronous_datagram_socket()) {
      return std::move(response->synchronous_datagram_socket());
    }
    return ZX_ERR_IO;
  }
};

template <>
struct WireResponse<frawsocket::Provider::Socket> {
  using ClientEnd = fidl::ClientEnd<frawsocket::Socket>;

  static ClientEnd FromResponse(frawsocket::wire::ProviderSocketResponse* response) {
    return std::move(response->s);
  }
};

// TODO(https://fxbug.dev/384115233): Update after the API is stabilized.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
template <>
struct WireResponse<fsocket::Provider::StreamSocketWithOptions> {
  using ClientEnd = fidl::ClientEnd<fsocket::StreamSocket>;

  static ClientEnd FromResponse(fsocket::wire::ProviderStreamSocketWithOptionsResponse* response) {
    return std::move(response->s);
  }
};

template <>
struct WireResponse<fsocket::Provider::DatagramSocketWithOptions> {
  using ClientEnd = std::variant<fidl::ClientEnd<fsocket::DatagramSocket>,
                                 fidl::ClientEnd<fsocket::SynchronousDatagramSocket>, zx_status_t>;

  static ClientEnd FromResponse(
      fsocket::wire::ProviderDatagramSocketWithOptionsResponse* response) {
    if (response->is_datagram_socket()) {
      return std::move(response->datagram_socket());
    }
    if (response->is_synchronous_datagram_socket()) {
      return std::move(response->synchronous_datagram_socket());
    }
    return ZX_ERR_IO;
  }
};

template <>
struct WireResponse<frawsocket::Provider::SocketWithOptions> {
  using ClientEnd = fidl::ClientEnd<frawsocket::Socket>;

  static ClientEnd FromResponse(frawsocket::wire::ProviderSocketWithOptionsResponse* response) {
    return std::move(response->s);
  }
};
#endif

// Handles the wire result from socket provider and extracts client end channel it returns.
template <typename FidlMethod>
zx::result<fit::result<fuchsia_posix::Errno, typename WireResponse<FidlMethod>::ClientEnd>>
handle_wire_result(fidl::WireResult<FidlMethod> result) {
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return zx::ok(result->take_error());
  }
  auto client_end = WireResponse<FidlMethod>::FromResponse(result->value());
  return zx::ok(fit::ok(std::move(client_end)));
}

// Helper class to deal with variants.
template <class... Ts>
struct overloaded : Ts... {
  using Ts::operator()...;
};

template <class... Ts>
overloaded(Ts...) -> overloaded<Ts...>;

zx_status_t zxio_socket_inner(zxio_service_connector service_connector, int domain, int type,
                              int protocol, std::optional<socket_creation_options> opts,
                              zxio_storage_alloc allocator, void** out_context, int16_t* out_code) {
  zxio_storage_t* zxio_storage = nullptr;
  fsocket::wire::Domain sock_domain;
// TODO(https://fxbug.dev/384115233): Update after the API is stabilized.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  constexpr size_t kMaxSocketCreationOptionsArenaSize =
      fidl::MaxSizeInChannel<fsocket::wire::SocketCreationOptions,
                             fidl::MessageDirection::kSending>();
  // Set a sensible upper limit for how much stack space we're going to allow
  // using here to prevent deep stack usage in zxio/fdio. If this grows to
  // untenable sizes we might have to change strategies here.
  static_assert(kMaxSocketCreationOptionsArenaSize <= 128);

  if (opts.has_value()) {
    for (const auto& mark : opts->marks) {
      if (mark.domain != ZXIO_SOCKET_MARK_DOMAIN_1 && mark.domain != ZXIO_SOCKET_MARK_DOMAIN_2) {
        *out_code = EINVAL;
        return ZX_OK;
      }
    }
  }
  fidl::Arena<kMaxSocketCreationOptionsArenaSize> arena;
  // Must check that `opts` has value.
  auto creation_opts = [&arena, &opts]() -> fsocket::wire::SocketCreationOptions {
    auto fidl_marks = fnet::wire::Marks::Builder(arena);
    for (auto& mark : opts->marks) {
      switch (mark.domain) {
        case ZXIO_SOCKET_MARK_DOMAIN_1:
          if (mark.is_present) {
            fidl_marks.mark_1(mark.value);
          } else {
            fidl_marks.clear_mark_1();
          }
          break;
        case ZXIO_SOCKET_MARK_DOMAIN_2:
          if (mark.is_present) {
            fidl_marks.mark_2(mark.value);
          } else {
            fidl_marks.clear_mark_2();
          }
          break;
        default:
          // We did the validation before.
          __builtin_unreachable();
      }
    }
    auto builder = fsocket::wire::SocketCreationOptions::Builder(arena).marks(fidl_marks.Build());
    if (opts->wake_group.has_value()) {
      builder.group(
          fuchsia_net_resources::wire::WakeGroupToken{std::move(opts->wake_group.value())});
    }
    return builder.Build();
  };
#else
  if (opts.has_value()) {
    *out_code = EOPNOTSUPP;
    return ZX_OK;
  }
#endif
  switch (domain) {
    case AF_PACKET: {
      if ((protocol > std::numeric_limits<uint16_t>::max()) ||
          (protocol < std::numeric_limits<uint16_t>::min())) {
        return ZX_ERR_INVALID_ARGS;
      }

      fpacketsocket::wire::Kind kind;
      switch (type) {
        case SOCK_DGRAM:
          kind = fpacketsocket::wire::Kind::kNetwork;
          break;
        case SOCK_RAW:
          kind = fpacketsocket::wire::Kind::kLink;
          break;
        default:
          return ZX_ERR_INVALID_ARGS;
      }

      zx::result<fidl::UnownedClientEnd<fpacketsocket::Provider>> provider =
          connect_socket_provider<fpacketsocket::Provider>(service_connector);
      if (provider.is_error()) {
        return ZX_ERR_IO;
      }
      fidl::WireResult socket_result = fidl::WireCall(provider.value())->Socket(kind);
      if (!socket_result.ok()) {
        return socket_result.status();
      }
      if (socket_result->is_error()) {
        *out_code = static_cast<int16_t>(socket_result->error_value());
        return ZX_OK;
      }
      fidl::ClientEnd<fpacketsocket::Socket>& control = socket_result->value()->socket;
      fidl::WireResult result = fidl::WireCall(control)->Describe();
      if (!result.ok()) {
        return result.status();
      }
      fidl::WireResponse response = result.value();
      if (!response.has_event()) {
        return ZX_ERR_NOT_SUPPORTED;
      }
      if (zx_status_t status =
              allocator(ZXIO_OBJECT_TYPE_PACKET_SOCKET, &zxio_storage, out_context);
          status != ZX_OK || zxio_storage == nullptr) {
        return ZX_ERR_NO_MEMORY;
      }
      if (zx_status_t status = zxio_packet_socket_init(zxio_storage, std::move(response.event()),
                                                       std::move(control));
          status != ZX_OK) {
        return status;
      }
      const sockaddr_ll sll = {
          .sll_family = AF_PACKET,
          // NB: protocol is in network byte order.
          .sll_protocol = static_cast<uint16_t>(protocol),
      };

      if (sll.sll_protocol != 0) {
        // We successfully created the packet socket but the caller wants the
        // socket to be associated with some protocol so we do that now.
        if (zx_status_t status = zxio_bind(
                &zxio_storage->io, reinterpret_cast<const sockaddr*>(&sll), sizeof(sll), out_code);
            status != ZX_OK) {
          return status;
        }
      }
      *out_code = 0;
      return ZX_OK;
    }
    case AF_INET:
      sock_domain = fsocket::wire::Domain::kIpv4;
      break;
    case AF_INET6:
      sock_domain = fsocket::wire::Domain::kIpv6;
      break;
    default:
      return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
  }

  switch (type) {
    case SOCK_STREAM:
      switch (protocol) {
        case IPPROTO_IP:
        case IPPROTO_TCP: {
          zx::result<fidl::UnownedClientEnd<fsocket::Provider>> provider =
              connect_socket_provider<fsocket::Provider>(service_connector);
          if (provider.is_error()) {
            return ZX_ERR_IO;
          }
          auto socket_result =
// TODO(https://fxbug.dev/384115233): Update after the API is stabilized. Use if-else
// instead of the ternary operator.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
              opts.has_value()
                  ? handle_wire_result(
                        fidl::WireCall(provider.value())
                            ->StreamSocketWithOptions(sock_domain,
                                                      fsocket::wire::StreamSocketProtocol::kTcp,
                                                      creation_opts()))
                  :
#endif
                  handle_wire_result(
                      fidl::WireCall((provider.value()))
                          ->StreamSocket(sock_domain, fsocket::wire::StreamSocketProtocol::kTcp));
          if (socket_result.is_error()) {
            return socket_result.error_value();
          }
          if ((*socket_result).is_error()) {
            *out_code = static_cast<int16_t>((*socket_result).error_value());
            return ZX_OK;
          }
          fidl::ClientEnd<fsocket::StreamSocket>& control = (*socket_result).value();
          fidl::WireResult result = fidl::WireCall(control)->Describe();
          if (!result.ok()) {
            return result.status();
          }
          fidl::WireResponse response = result.value();
          if (!response.has_socket()) {
            return ZX_ERR_NOT_SUPPORTED;
          }
          zx_info_socket_t info;
          zx::socket& socket = response.socket();
          if (zx_status_t status =
                  socket.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr);
              status != ZX_OK) {
            return status;
          }
          if (zx_status_t status =
                  allocator(ZXIO_OBJECT_TYPE_STREAM_SOCKET, &zxio_storage, out_context);
              status != ZX_OK || zxio_storage == nullptr) {
            return ZX_ERR_NO_MEMORY;
          }
          if (zx_status_t status = zxio_stream_socket_init(
                  zxio_storage, std::move(response.socket()), info, false, std::move(control));
              status != ZX_OK) {
            return status;
          }

        } break;
        default:
          return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
      }
      break;
    case SOCK_DGRAM: {
      fsocket::wire::DatagramSocketProtocol proto;
      switch (protocol) {
        case IPPROTO_IP:
        case IPPROTO_UDP:
          proto = fsocket::wire::DatagramSocketProtocol::kUdp;
          break;
        case IPPROTO_ICMP:
          if (sock_domain != fsocket::wire::Domain::kIpv4) {
            return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
          }
          proto = fsocket::wire::DatagramSocketProtocol::kIcmpEcho;
          break;
        case IPPROTO_ICMPV6:
          if (sock_domain != fsocket::wire::Domain::kIpv6) {
            return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
          }
          proto = fsocket::wire::DatagramSocketProtocol::kIcmpEcho;
          break;
        default:
          return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
      }

      zx::result<fidl::UnownedClientEnd<fsocket::Provider>> provider =
          connect_socket_provider<fsocket::Provider>(service_connector);
      if (provider.is_error()) {
        return ZX_ERR_IO;
      }

      auto socket_result =
// TODO(https://fxbug.dev/384115233): Update after the API is stabilized. Use if-else
// instead of the ternary operator.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
          opts.has_value()
              ? handle_wire_result(
                    fidl::WireCall(provider.value())
                        ->DatagramSocketWithOptions(sock_domain, proto, creation_opts()))
              :
#endif
              handle_wire_result(
                  fidl::WireCall(provider.value())->DatagramSocket(sock_domain, proto));
      if (socket_result.is_error()) {
        return socket_result.error_value();
      }
      if ((*socket_result).is_error()) {
        *out_code = static_cast<int16_t>((*socket_result).error_value());
        return ZX_OK;
      }
      auto& response = (*socket_result).value();
      return std::visit(
          overloaded{
              [&](fidl::ClientEnd<fsocket::DatagramSocket>& control) {
                fidl::WireResult result = fidl::WireCall(control)->Describe();
                if (!result.ok()) {
                  return result.status();
                }
                fidl::WireResponse response = result.value();
                if (!response.has_socket()) {
                  return ZX_ERR_NOT_SUPPORTED;
                }
                if (!response.has_tx_meta_buf_size()) {
                  return ZX_ERR_NOT_SUPPORTED;
                }
                if (!response.has_rx_meta_buf_size()) {
                  return ZX_ERR_NOT_SUPPORTED;
                }
                if (!response.has_metadata_encoding_protocol_version() ||
                    response.metadata_encoding_protocol_version() !=
                        fsocket::UdpMetadataEncodingProtocolVersion::kZero) {
                  return ZX_ERR_NOT_SUPPORTED;
                }
                zx::socket& socket = response.socket();
                zx_info_socket_t info;
                if (zx_status_t status =
                        socket.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr);
                    status != ZX_OK) {
                  return status;
                }
                if (zx_status_t status =
                        allocator(ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET, &zxio_storage, out_context);
                    status != ZX_OK || zxio_storage == nullptr) {
                  return ZX_ERR_NO_MEMORY;
                }
                if (zx_status_t status =
                        zxio_datagram_socket_init(zxio_storage, std::move(socket), info,
                                                  {
                                                      response.tx_meta_buf_size(),
                                                      response.rx_meta_buf_size(),
                                                  },
                                                  std::move(control));
                    status != ZX_OK) {
                  return status;
                }
                *out_code = 0;
                return ZX_OK;
              },
              [&](fidl::ClientEnd<fsocket::SynchronousDatagramSocket>& control) {
                fidl::WireResult result = fidl::WireCall(control)->Describe();
                if (!result.ok()) {
                  return result.status();
                }
                fidl::WireResponse response = result.value();
                if (!response.has_event()) {
                  return ZX_ERR_NOT_SUPPORTED;
                }
                if (zx_status_t status = allocator(ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET,
                                                   &zxio_storage, out_context);
                    status != ZX_OK || zxio_storage == nullptr) {
                  return ZX_ERR_NO_MEMORY;
                }
                if (zx_status_t status = zxio_synchronous_datagram_socket_init(
                        zxio_storage, std::move(response.event()), std::move(control));
                    status != ZX_OK) {
                  return status;
                }
                *out_code = 0;
                return ZX_OK;
              },
              [](zx_status_t err) { return err; }},
          response);
    } break;
    case SOCK_RAW: {
      if (protocol == 0) {
        return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
      }
      if ((protocol > std::numeric_limits<uint8_t>::max()) ||
          (protocol < std::numeric_limits<uint8_t>::min())) {
        return ZX_ERR_INVALID_ARGS;
      }
      frawsocket::wire::ProtocolAssociation proto_assoc;
      uint8_t sock_protocol = static_cast<uint8_t>(protocol);
      // Sockets created with IPPROTO_RAW are only used to send packets as per
      // https://linux.die.net/man/7/raw,
      //
      //   A protocol of IPPROTO_RAW implies enabled IP_HDRINCL and is able to
      //   send any IP protocol that is specified in the passed header. Receiving
      //   of all IP protocols via IPPROTO_RAW is not possible using raw sockets.
      if (protocol == IPPROTO_RAW) {
        proto_assoc = frawsocket::wire::ProtocolAssociation::WithUnassociated({});
      } else {
        proto_assoc = frawsocket::wire::ProtocolAssociation::WithAssociated(sock_protocol);
      }

      zx::result<fidl::UnownedClientEnd<frawsocket::Provider>> provider =
          connect_socket_provider<frawsocket::Provider>(service_connector);
      if (provider.is_error()) {
        return ZX_ERR_IO;
      }
      auto socket_result =
// TODO(https://fxbug.dev/384115233): Update after the API is stabilized. Use if-else
// instead of the ternary operator.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
          opts.has_value() ? handle_wire_result(
                                 fidl::WireCall(provider.value())
                                     ->SocketWithOptions(sock_domain, proto_assoc, creation_opts()))
                           :
#endif
                           handle_wire_result(
                               fidl::WireCall(provider.value())->Socket(sock_domain, proto_assoc));
      if (socket_result.is_error()) {
        return socket_result.error_value();
      }
      if ((*socket_result).is_error()) {
        *out_code = static_cast<int16_t>((*socket_result).error_value());
        return ZX_OK;
      }
      fidl::ClientEnd<frawsocket::Socket>& control = (*socket_result).value();
      fidl::WireResult result = fidl::WireCall(control)->Describe();
      if (!result.ok()) {
        return result.status();
      }
      fidl::WireResponse response = result.value();
      if (!response.has_event()) {
        return ZX_ERR_NOT_SUPPORTED;
      }
      if (zx_status_t status = allocator(ZXIO_OBJECT_TYPE_RAW_SOCKET, &zxio_storage, out_context);
          status != ZX_OK || zxio_storage == nullptr) {
        return ZX_ERR_NO_MEMORY;
      }
      if (zx_status_t status =
              zxio_raw_socket_init(zxio_storage, std::move(response.event()), std::move(control));
          status != ZX_OK) {
        return status;
      }
    } break;
    default:
      return ZX_ERR_PROTOCOL_NOT_SUPPORTED;
  }

  *out_code = 0;
  return ZX_OK;
}
}  // namespace

zx_status_t zxio_socket_with_options(zxio_service_connector service_connector, int domain, int type,
                                     int protocol, zxio_socket_creation_options opts,
                                     zxio_storage_alloc allocator, void** out_context,
                                     int16_t* out_code) {
  return zxio_socket_inner(service_connector, domain, type, protocol, socket_creation_options(opts),
                           allocator, out_context, out_code);
}
zx_status_t zxio_socket(zxio_service_connector service_connector, int domain, int type,
                        int protocol, zxio_storage_alloc allocator, void** out_context,
                        int16_t* out_code) {
  return zxio_socket_inner(service_connector, domain, type, protocol, {}, allocator, out_context,
                           out_code);
}

zx_status_t zxio_read_link(zxio_t* io, const uint8_t** out_target, size_t* out_target_len) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->read_link(io, out_target, out_target_len);
}

zx_status_t zxio_create_symlink(zxio_t* io, const char* name, size_t name_len,
                                const uint8_t* target, size_t target_len, zxio_storage_t* storage) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->create_symlink(io, name, name_len, target, target_len, storage);
}

zx_status_t zxio_xattr_list(zxio_t* io,
                            void (*callback)(void* context, const uint8_t* name, size_t name_len),
                            void* context) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->xattr_list(io, callback, context);
}

zx_status_t zxio_xattr_get(zxio_t* io, const uint8_t* name, size_t name_len,
                           zx_status_t (*callback)(void* context, zxio_xattr_data_t data),
                           void* context) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->xattr_get(io, name, name_len, callback, context);
}

zx_status_t zxio_xattr_set(zxio_t* io, const uint8_t* name, size_t name_len, const uint8_t* value,
                           size_t value_len, zxio_xattr_set_mode_t mode) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->xattr_set(io, name, name_len, value, value_len, mode);
}

zx_status_t zxio_xattr_remove(zxio_t* io, const uint8_t* name, size_t name_len) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->xattr_remove(io, name, name_len);
}

zx_status_t zxio_allocate(zxio_t* io, uint64_t offset, uint64_t len, zxio_allocate_mode_t mode) {
  if (!zxio_is_valid(io)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zxio_internal_t* zio = to_internal(io);
  return zio->ops->allocate(io, offset, len, mode);
}
