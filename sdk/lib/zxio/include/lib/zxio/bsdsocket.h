// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_BSDSOCKET_H_
#define LIB_ZXIO_BSDSOCKET_H_

#include <lib/zxio/types.h>
#include <lib/zxio/zxio.h>
#include <sys/socket.h>
#include <zircon/types.h>

__BEGIN_CDECLS

#define ZXIO_EXPORT __EXPORT

typedef zx_status_t (*zxio_service_connector)(const char* service_name,
                                              zx_handle_t* provider_handle);

// Creates a socket. Expects |service_connector| to yield a borrowed handle to the respective
// socket provider service. |allocator| is expected to allocate storage for a zxio_t object.
// On success, |*out_context| will point to the object allocated by |allocator|.
ZXIO_EXPORT zx_status_t zxio_socket(zxio_service_connector service_connector, int domain, int type,
                                    int protocol, zxio_storage_alloc allocator, void** out_context,
                                    int16_t* out_code);

// Binds the socket referred to in |io| to the address specified by |addr|.
ZXIO_EXPORT zx_status_t zxio_bind(zxio_t* io, const struct sockaddr* addr, socklen_t addrlen,
                                  int16_t* out_code);

// Connects the socket referred to in |io| to the address specified by |addr|.
ZXIO_EXPORT zx_status_t zxio_connect(zxio_t* io, const struct sockaddr* addr, socklen_t addrlen,
                                     int16_t* out_code);

// Marks the socket referred to in |io| as listening.
ZXIO_EXPORT zx_status_t zxio_listen(zxio_t* io, int backlog, int16_t* out_code);

// Accepts the first pending connection request on the socket referred to in |io|.
// Writes up to |*addrlen| bytes of the remote peer's address to |*addr| and sets |*addrlen|
// to the size of the remote peer's address. |*out_storage| will contain a new, connected socket.
ZXIO_EXPORT zx_status_t zxio_accept(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                                    zxio_storage_t* out_storage, int16_t* out_code);

// Writes up to |*addrlen| bytes of the socket's address to |*addr| and sets |*addrlen|
// to the size of the socket's address.
ZXIO_EXPORT zx_status_t zxio_getsockname(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                                         int16_t* out_code);

// Writes up to |*addrlen| bytes of the remote peer's address to |*addr| and sets |*addrlen|
// to the size of the remote peer's address
ZXIO_EXPORT zx_status_t zxio_getpeername(zxio_t* io, struct sockaddr* addr, socklen_t* addrlen,
                                         int16_t* out_code);

// Writes up to |*optlen| bytes of the value of the socket option specified by |level| and
// |optname| to |*optval| and sets |*optlen| to the size of the socket option.
ZXIO_EXPORT zx_status_t zxio_getsockopt(zxio_t* io, int level, int optname, void* optval,
                                        socklen_t* optlen, int16_t* out_code);

// Reads up to |optlen| bytes from |*optval| into the value of the socket option specified
// by |level| and |optname|. |access_token| should be provided if the socket option needs an
// access token, otherwise it should be set to |ZX_HANDLE_INVALID|. See table below for the
// required access tokens:
//
//  Option                     | Required Access Token
//  ---------------------------|--------------------------------------------------------
//  (SOL_SOCKET, SO_REUSEPORT) | Sharing Domain Token of the calling process
//
// If |access_token| is provided, it is consumed independently of the result of the call.
ZXIO_EXPORT zx_status_t zxio_setsockopt(zxio_t* io, int level, int optname, const void* optval,
                                        socklen_t optlen, zx_handle_t access_token,
                                        int16_t* out_code);

// Receives a message from a socket and sets |*out_actual| to the total bytes received.
//
// |msg|, |msg->msg_name|, |msg->msg_control| and |msg->msg_iov| must always point to
// valid memory if not null (properly aligned and will not trigger faults if accessed).
// The memory pointed to by the |iovec|s found in |msg->msg_iov| is allowed to fault
// iff the library's |zxio_maybe_faultable_copy| method is overridden to a method
// that can handle such faults. If the default definition of |zxio_maybe_faultable_copy|
// is used, then |msg->msg_iov| must also not fault. Note that unexpected faults
// will cause a Zircon exception to be raised.
ZXIO_EXPORT zx_status_t zxio_recvmsg(zxio_t* io, struct msghdr* msg, int flags, size_t* out_actual,
                                     int16_t* out_code);

// Sends a message from a socket and sets |*out_actual| to the total bytes sent.
//
// |msg|, |msg->msg_name|, |msg->msg_control| and |msg->msg_iov| must always point to
// valid memory if not null (properly aligned and will not trigger faults if accessed).
// The memory pointed to by the |iovec|s found in |msg->msg_iov| is allowed to fault
// iff the library's |zxio_maybe_faultable_copy| method is overridden to a method
// that can handle such faults. If the default definition of |zxio_maybe_faultable_copy|
// is used, then |msg->msg_iov| must also not fault. Note that unexpected faults
// will cause a Zircon exception to be raised.
ZXIO_EXPORT zx_status_t zxio_sendmsg(zxio_t* io, const struct msghdr* msg, int flags,
                                     size_t* out_actual, int16_t* out_code);

// A Fuchsia-specific socket option to set socket marks.
#define SO_FUCHSIA_MARK 10000

typedef uint8_t zxio_socket_mark_domain_t;
// The first socket mark domain.
#define ZXIO_SOCKET_MARK_DOMAIN_1 ((zxio_socket_mark_domain_t)1u)
// The second socket mark domain.
#define ZXIO_SOCKET_MARK_DOMAIN_2 ((zxio_socket_mark_domain_t)2u)

// A fuchsia socket can have multiple optional socket marks. This structure represents
// a socket mark for a specified domain. If `is_present` is 0, it means the socket does
// not carry a mark for the given domain and `value` field is unspecified.
//
// When getting the socket mark, you need to provide the `domain` field and the other
// fields will be filled as a result.
// When setting the socket mark, you can set a mark for a domain with `is_present` to
// be true, or clear the mark for that domain with `is_present` to be false.
typedef struct zxio_socket_mark {
  uint32_t value;
  zxio_socket_mark_domain_t domain;
  bool is_present;
} zxio_socket_mark_t;

// Optional parameters for creating a socket.
typedef struct zxio_socket_creation_options {
  // The length of the array pointed by |marks|.
  size_t num_marks;
  // An array of |zxio_socket_mark_t|, these marks will be applied to the
  // created socket from first to last.
  zxio_socket_mark_t* marks;
  // An optional wake group to which to add the socket.
  //
  // `ZX_HANDLE_INVALID` is interpreted as no wake group.
  //
  // If specified, must be a handle to an event that was retrieved from the
  // netstack on creation of a wake group. It must have the `TRANSFER` and
  // `DUPLICATE` rights.
  zx_handle_t wake_group;
} zxio_socket_creation_options_t;

// Creates a socket with the optional creation |opts|. Expects |service_connector| to yield
// a borrowed handle to the respective socket provider service. |allocator| is expected to
// allocate storage for a zxio_t object. On success, |*out_context| will point to the object
// allocated by |allocator|.
ZXIO_EXPORT zx_status_t zxio_socket_with_options(zxio_service_connector service_connector,
                                                 int domain, int type, int protocol,
                                                 zxio_socket_creation_options_t opts,
                                                 zxio_storage_alloc allocator, void** out_context,
                                                 int16_t* out_code);

__END_CDECLS

#endif  // LIB_ZXIO_BSDSOCKET_H_
