// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_CPP_UDP_SOCKET_PRIVATE_H_
#define LIB_ZXIO_CPP_UDP_SOCKET_PRIVATE_H_

#include <fidl/fuchsia.posix.socket/cpp/wire.h>

namespace zxio {

inline constexpr size_t kMetadataSizeSegmentSize = 8;

// Size occupied by the prelude bytes in a Tx message.
inline constexpr size_t kTxUdpPreludeSize =
    fidl::MaxSizeInChannel<fuchsia_posix_socket::wire::SendMsgMeta,
                           fidl::MessageDirection::kSending>() +
    kMetadataSizeSegmentSize;

// Size occupied by the prelude bytes in an Rx message.
inline constexpr size_t kRxUdpPreludeSize =
    fidl::MaxSizeInChannel<fuchsia_posix_socket::wire::RecvMsgMeta,
                           fidl::MessageDirection::kSending>() +
    kMetadataSizeSegmentSize;

}  // namespace zxio

#endif  // LIB_ZXIO_CPP_UDP_SOCKET_PRIVATE_H_
