// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_LOG_ZIRCON_H_
#define LIB_LD_LOG_ZIRCON_H_

#include <lib/zx/debuglog.h>
#include <lib/zx/socket.h>
#include <zircon/syscalls/log.h>

#include <string_view>

namespace ld {

// This is a callable object that gets output to a debuglog and/or socket.
// It should be called with a single full log message ending in a newline.
class Log {
 public:
  // This is a recommended maximum buffer size to use for log messages.
  // A message longer than this can be split across multiple lines.
  static constexpr size_t kBufferSize = ZX_LOG_RECORD_DATA_MAX;

  // Returns the number of chars written, or -1.
  int operator()(std::string_view str) const;

  // Returns true if there is any point in calling the output function.
  // Formatting output can be skipped if it won't go anywhere.
  explicit operator bool() const { return debuglog_ || socket_; }

  zx::debuglog set_debuglog(zx::debuglog log) { return std::exchange(debuglog_, std::move(log)); }

  zx::socket set_socket(zx::socket socket) { return std::exchange(socket_, std::move(socket)); }

  // Consume a log handle as transferred by the bootstrap protocol.  This does
  // set_debuglog() or set_socket() if the handle is of a recognized type.
  void TakeLogHandle(zx::handle handle);

 private:
  zx::debuglog debuglog_;
  zx::socket socket_;
};

}  // namespace ld

#endif  // LIB_LD_LOG_ZIRCON_H_
