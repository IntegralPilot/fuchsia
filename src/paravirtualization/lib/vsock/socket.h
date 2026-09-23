// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PARAVIRTUALIZATION_LIB_VSOCK_SOCKET_H_
#define SRC_PARAVIRTUALIZATION_LIB_VSOCK_SOCKET_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// This creates a new stream socket in the vsock family as if this call was made:
//   socket(AF_VSOCK, SOCK_STREAM, 0);
zx_status_t create_virtio_stream_socket(int* out_fd);

// This returns the local cid of the system.
zx_status_t get_local_cid(uint32_t* out_local_cid);

__END_CDECLS

#endif  // SRC_PARAVIRTUALIZATION_LIB_VSOCK_SOCKET_H_
