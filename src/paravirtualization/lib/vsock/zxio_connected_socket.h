// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PARAVIRTUALIZATION_LIB_VSOCK_ZXIO_CONNECTED_SOCKET_H_
#define SRC_PARAVIRTUALIZATION_LIB_VSOCK_ZXIO_CONNECTED_SOCKET_H_

#include <fidl/fuchsia.vsock/cpp/fidl.h>
#include <lib/zx/socket.h>
#include <lib/zxio/types.h>

#include <vector>

#include "src/paravirtualization/lib/vsock/vm_sockets.h"

namespace vsock {

class ZxioConnectedSocket {
 public:
  ZxioConnectedSocket(zx::socket socket,
                      fidl::ClientEnd<fuchsia_vsock::Connection> connection_client_end,
                      struct sockaddr_vm& peer_addr);

  zx::socket& socket() { return socket_; }
  const struct sockaddr_vm& peer_addr() const { return *peer_addr_; }

 private:
  zxio_t io_;
  zx::socket socket_;
  fidl::ClientEnd<fuchsia_vsock::Connection> connection_client_end_;
  std::unique_ptr<struct sockaddr_vm> peer_addr_;
};

}  // namespace vsock

#endif  // SRC_PARAVIRTUALIZATION_LIB_VSOCK_ZXIO_CONNECTED_SOCKET_H_
