// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SOCKET_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SOCKET_DISPATCHER_H_

#include <lib/object-constants.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zx/result.h>
#include <stdint.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class SocketDispatcher;

DECLARE_PEERED_DISPATCHER_RUST_PROTOS(SocketDispatcher, rust_socket_dispatcher)

extern "C" {
zx_status_t cpp_socket_dispatcher_create(
    void* holder, uint32_t flags, ffi::Uninitialized<KernelHandle<SocketDispatcher>>* handle_out);

size_t rust_socket_dispatcher_get_read_threshold(const SocketDispatcher* disp);
zx_status_t rust_socket_dispatcher_set_read_threshold(const SocketDispatcher* disp, size_t value);
size_t rust_socket_dispatcher_get_write_threshold(const SocketDispatcher* disp);
zx_status_t rust_socket_dispatcher_set_write_threshold(const SocketDispatcher* disp, size_t value);
zx_info_socket_t rust_socket_dispatcher_get_info(const SocketDispatcher* disp);
}  // extern "C"

class SocketDispatcher final : public Dispatcher {
 public:
  SocketDispatcher(void* holder, uint32_t flags);
  ~SocketDispatcher() final;

  DECLARE_PEERED_DISPATCHER_RUST_METHODS(rust_socket_dispatcher, ZX_OBJ_TYPE_SOCKET, true)

  // Property methods.
  size_t GetReadThreshold() const { return rust_socket_dispatcher_get_read_threshold(this); }
  zx_status_t SetReadThreshold(size_t value) const {
    return rust_socket_dispatcher_set_read_threshold(this, value);
  }
  size_t GetWriteThreshold() const { return rust_socket_dispatcher_get_write_threshold(this); }
  zx_status_t SetWriteThreshold(size_t value) const {
    return rust_socket_dispatcher_set_write_threshold(this, value);
  }

  zx_info_socket_t GetInfo() const { return rust_socket_dispatcher_get_info(this); }

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  OpaqueStorage<kSocketDispatcherStateSize, kSocketDispatcherStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_SOCKET_DISPATCHER_H_
