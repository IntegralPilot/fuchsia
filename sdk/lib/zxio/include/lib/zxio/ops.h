// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_OPS_H_
#define LIB_ZXIO_OPS_H_

#include <lib/zxio/types.h>
#include <lib/zxio/zxio.h>
#include <stdarg.h>
#include <sys/socket.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// NOLINTBEGIN(modernize-use-using): This library uses typedefs to export a C interface.

// A table of operations for a zxio_t.
//
// Most of the functions that operate on a zxio_t call through this operations
// table to actually perform the operation. Use |zxio_init| to initialize a
// zxio_t with a custom operations table.
typedef struct zxio_ops {
  // Releases all resources held by |io|. No further ops may be called after invoking |destroy|.
  void (*destroy)(zxio_t* io);

  // See `zxio_close`.
  zx_status_t (*close)(zxio_t* io);

  // After |release| returns, any further ops most not be called relative to |io|,
  // except |destroy|.
  zx_status_t (*release)(zxio_t* io, zx_handle_t* out_handle);

  zx_status_t (*borrow)(zxio_t* io, zx_handle_t* out_handle);

  // TODO(tamird/abarth): clarify the semantics of this operation. fdio currently relies on this to
  // implement POSIX-style dup() which expects the seek pointer to be preserved, but zxio_vmo_clone
  // does not currently produce those semantics.
  zx_status_t (*clone)(zxio_t* io, zx_handle_t* out_handle);
  void (*wait_begin)(zxio_t* io, zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                     zx_signals_t* out_zx_signals);
  void (*wait_end)(zxio_t* io, zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals);
  zx_status_t (*sync)(zxio_t* io);
  zx_status_t (*attr_get)(zxio_t* io, zxio_node_attributes_t* inout_attr);
  zx_status_t (*attr_set)(zxio_t* io, const zxio_node_attributes_t* attr);
  zx_status_t (*readv)(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                       zxio_flags_t flags, size_t* out_actual);
  zx_status_t (*readv_at)(zxio_t* io, zx_off_t offset, const zx_iovec_t* vector,
                          size_t vector_count, zxio_flags_t flags, size_t* out_actual);
  zx_status_t (*writev)(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                        zxio_flags_t flags, size_t* out_actual);
  zx_status_t (*writev_at)(zxio_t* io, zx_off_t offset, const zx_iovec_t* vector,
                           size_t vector_count, zxio_flags_t flags, size_t* out_actual);
  zx_status_t (*seek)(zxio_t* io, zxio_seek_origin_t start, int64_t offset, size_t* out_offset);
  zx_status_t (*truncate)(zxio_t* io, uint64_t length);
  zx_status_t (*flags_get)(zxio_t* io, uint64_t* out_flags);
  zx_status_t (*flags_set)(zxio_t* io, uint64_t flags);
  zx_status_t (*vmo_get)(zxio_t* io, zxio_vmo_flags_t flags, zx_handle_t* out_vmo);
  zx_status_t (*on_mapped)(zxio_t* io, void* ptr);
  zx_status_t (*get_read_buffer_available)(zxio_t* io, size_t* out_available);
  zx_status_t (*shutdown)(zxio_t* io, zxio_shutdown_options_t options, int16_t* out_code);
  zx_status_t (*unlink)(zxio_t* io, const char* name, size_t name_len, int flags);
  zx_status_t (*token_get)(zxio_t* io, zx_handle_t* out_token);
  zx_status_t (*rename)(zxio_t* io, const char* old_path, size_t old_path_len,
                        zx_handle_t dst_token, const char* new_path, size_t new_path_len);
  zx_status_t (*link)(zxio_t* io, const char* src_path, size_t src_path_len, zx_handle_t dst_token,
                      const char* dst_path, size_t dst_path_len);
  zx_status_t (*link_into)(zxio_t* object, zx_handle_t dst_directory_token, const char* dst_path,
                           size_t dst_path_len);
  zx_status_t (*dirent_iterator_init)(zxio_t* io, zxio_dirent_iterator_t* iterator);
  zx_status_t (*dirent_iterator_next)(zxio_t* io, zxio_dirent_iterator_t* iterator,
                                      zxio_dirent_t* inout_entry);
  zx_status_t (*dirent_iterator_rewind)(zxio_t* io, zxio_dirent_iterator_t* iterator);
  void (*dirent_iterator_destroy)(zxio_t* io, zxio_dirent_iterator_t* iterator);
  zx_status_t (*isatty)(zxio_t* io, bool* tty);
  zx_status_t (*get_window_size)(zxio_t* io, uint32_t* width, uint32_t* height);
  zx_status_t (*set_window_size)(zxio_t* io, uint32_t width, uint32_t height);
  zx_status_t (*advisory_lock)(zxio_t* io, struct advisory_lock_req* req);
  zx_status_t (*watch_directory)(zxio_t* io, zxio_watch_directory_cb cb, zx_time_t deadline,
                                 void* context);
  zx_status_t (*bind)(zxio_t* io, const struct sockaddr* addr, socklen_t addrlen,
                      int16_t* out_code);
  zx_status_t (*connect)(zxio_t* io, const struct sockaddr* addr, socklen_t addrlen,
                         int16_t* out_code);
  zx_status_t (*listen)(zxio_t* io, int backlog, int16_t* out_code);
  zx_status_t (*accept)(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                        zxio_storage_t* out_storage, int16_t* out_code);
  zx_status_t (*getsockname)(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                             int16_t* out_code);
  zx_status_t (*getpeername)(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                             int16_t* out_code);
  zx_status_t (*getsockopt)(zxio_t* io, int level, int optname, void* optval, socklen_t* optlen,
                            int16_t* out_code);
  zx_status_t (*setsockopt)(zxio_t* io, int level, int optname, const void* optval,
                            socklen_t optlen, zx_handle_t access_token, int16_t* out_code);
  zx_status_t (*recvmsg)(zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                         int16_t* out_code);
  zx_status_t (*sendmsg)(zxio_t* io, const struct msghdr* msg, int flags, size_t* out_actual,
                         int16_t* out_code);
  zx_status_t (*ioctl)(zxio_t* io, int request, int16_t* out_code, va_list va);
  zx_status_t (*read_link)(zxio_t* io, const uint8_t** out_target, size_t* out_target_len);
  zx_status_t (*create_symlink)(zxio_t* io, const char* name, size_t name_len,
                                const uint8_t* target, size_t target_len, zxio_storage_t* storage);
  zx_status_t (*xattr_list)(zxio_t* io,
                            void (*callback)(void* context, const uint8_t* name, size_t name_len),
                            void* context);
  zx_status_t (*xattr_get)(zxio_t* io, const uint8_t* name, size_t name_len,
                           zx_status_t (*callback)(void* context, zxio_xattr_data_t data),
                           void* context);
  zx_status_t (*xattr_set)(zxio_t* io, const uint8_t* name, size_t name_len, const uint8_t* value,
                           size_t value_len, zxio_xattr_set_mode_t mode);
  zx_status_t (*xattr_remove)(zxio_t* io, const uint8_t* name, size_t name_len);
  zx_status_t (*open)(zxio_t* directory, const char* path, size_t path_len, zxio_open_flags_t flags,
                      const zxio_open_options_t* options, zxio_storage_t* storage);
  zx_status_t (*allocate)(zxio_t* io, uint64_t offset, uint64_t len,
                          const zxio_allocate_mode_t mode);
  zx_status_t (*enable_verity)(zxio_t* io, const zxio_fsverity_descriptor_t* descriptor);
} zxio_ops_t;

// Initialize a |zxio_t| object with the given |ops| table.
void zxio_init(zxio_t* io, const zxio_ops_t* ops);

// Get the ops table used by the given |zxio_t| object.
const zxio_ops_t* zxio_get_ops(zxio_t* io);

// NOLINTEND(modernize-use-using)

__END_CDECLS

#endif  // LIB_ZXIO_OPS_H_
