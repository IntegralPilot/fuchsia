// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_SYNCIO_STUB_MISSING_INCLUDES_H_
#define SRC_STARNIX_LIB_SYNCIO_STUB_MISSING_INCLUDES_H_

// Adding includes that are not detected by rust-bindings because they are
// defined using functions

#include <lib/zxio/bsdsocket.h>
#include <lib/zxio/types.h>

// generate.py will remove __bindgen_missing_ from the start of constant names.
#define C(t, x) const t __bindgen_missing_##x = x

C(zxio_shutdown_options_t, ZXIO_SHUTDOWN_OPTIONS_READ);
C(zxio_shutdown_options_t, ZXIO_SHUTDOWN_OPTIONS_WRITE);
C(zxio_node_protocols_t, ZXIO_NODE_PROTOCOL_NONE);
C(zxio_node_protocols_t, ZXIO_NODE_PROTOCOL_CONNECTOR);
C(zxio_node_protocols_t, ZXIO_NODE_PROTOCOL_DIRECTORY);
C(zxio_node_protocols_t, ZXIO_NODE_PROTOCOL_FILE);
C(zxio_node_protocols_t, ZXIO_NODE_PROTOCOL_SYMLINK);
C(zxio_seek_origin_t, ZXIO_SEEK_ORIGIN_START);
C(zxio_seek_origin_t, ZXIO_SEEK_ORIGIN_CURRENT);
C(zxio_seek_origin_t, ZXIO_SEEK_ORIGIN_END);
C(zxio_allocate_mode_t, ZXIO_ALLOCATE_KEEP_SIZE);
C(zxio_allocate_mode_t, ZXIO_ALLOCATE_UNSHARE_RANGE);
C(zxio_allocate_mode_t, ZXIO_ALLOCATE_PUNCH_HOLE);
C(zxio_allocate_mode_t, ZXIO_ALLOCATE_COLLAPSE_RANGE);
C(zxio_allocate_mode_t, ZXIO_ALLOCATE_ZERO_RANGE);
C(zxio_allocate_mode_t, ZXIO_ALLOCATE_INSERT_RANGE);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_NONE);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_NODE);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_DIR);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_SERVICE);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_FILE);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_TTY);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_VMO);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_DEBUGLOG);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_PIPE);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_STREAM_SOCKET);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_RAW_SOCKET);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_PACKET_SOCKET);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET);
C(zxio_object_type_t, ZXIO_OBJECT_TYPE_SYMLINK);
C(zxio_selinux_context_state_t, ZXIO_SELINUX_CONTEXT_STATE_DATA);
C(zxio_selinux_context_state_t, ZXIO_SELINUX_CONTEXT_STATE_USE_XATTRS);
C(zxio_socket_mark_domain_t, ZXIO_SOCKET_MARK_DOMAIN_1);
C(zxio_socket_mark_domain_t, ZXIO_SOCKET_MARK_DOMAIN_2);

#endif  // SRC_STARNIX_LIB_SYNCIO_STUB_MISSING_INCLUDES_H_
