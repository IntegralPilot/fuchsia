// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/log-zircon.h"

#include <lib/zircon-internal/unique-backtrace.h>
#include <lib/zx/debuglog.h>
#include <lib/zx/result.h>
#include <lib/zx/socket.h>
#include <zircon/processargs.h>
#include <zircon/syscalls/object.h>

#include <cassert>

namespace ld {
namespace {

constexpr zx_signals_t kSocketWait = ZX_SOCKET_WRITABLE | ZX_SOCKET_PEER_CLOSED;

void DebuglogWrite(const zx::debuglog& debuglog, std::string_view str) {
  while (!str.empty()) {
    // Scan up to one past the maximum chunk size, so as to both use the full
    // buffer and omit the trailing newline when the buffer is exactly full.
    constexpr size_t kMaxScan = ld::Log::kBufferSize + 1;
    std::string_view chunk = str.substr(0, std::min(kMaxScan, str.size()));

    if (size_t nl = chunk.find('\n'); nl != std::string_view::npos) {
      // Write only a single line at a time so each line gets tagged.
      // Elide the trailing newline.
      chunk = chunk.substr(0, nl);
      str.remove_prefix(nl + 1);
    } else {
      // There was no newline, so write as much as fits in one packet.
      if (chunk.size() > ld::Log::kBufferSize) {
        assert(chunk.size() == ld::Log::kBufferSize + 1);
        chunk.remove_suffix(1);
      }
      str = str.substr(chunk.size());
    }

    zx_status_t status = debuglog.write(0, chunk.data(), chunk.size());
    if (status != ZX_OK) {
      CRASH_WITH_UNIQUE_BACKTRACE();
    }
  }
}

zx::result<size_t> SocketWrite(const zx::socket& socket, std::string_view str) {
  zx_status_t status;
  do {
    size_t wrote = 0;
    status = socket.write(0, str.data(), str.size(), &wrote);
    if (status == ZX_OK) {
      return zx::ok(wrote);
    }
    if (status == ZX_ERR_SHOULD_WAIT) {
      zx_signals_t pending = 0;
      status = socket.wait_one(kSocketWait, zx::time::infinite(), &pending);
    }
  } while (status == ZX_OK);
  return zx::error(status);
}

int SocketWriteAll(const zx::socket& socket, std::string_view str) {
  int wrote = 0;
  while (!str.empty()) {
    auto result = SocketWrite(socket, str);
    if (result.is_error()) {
      return wrote == 0 ? -1 : wrote;
    }
    str.remove_prefix(result.value());
    wrote += static_cast<int>(result.value());
  }
  return wrote;
}

}  // namespace

// The formatted message from elfldltl::PrintfDiagnosticsReport should be a
// single line with no newline, but we tell Printf to add one (see below).
// If the whole line fits in the buffer, then this callback will only be made
// once.
int Log::operator()(std::string_view str) const {
  if (str.empty()) {
    return 0;
  }

  // If we have a debuglog handle, use that.  It's packet-oriented with each
  // message expected to be a single line without newlines.  So remove the
  // newline that was added.  If the line was too long, it will look like two
  // separate log lines.
  if (debuglog_) {
    DebuglogWrite(debuglog_, str);
  }

  if (socket_) {
    // We might instead (or also?) have a socket, where the messages are
    // easier to capture at the other end.  This is a stream-oriented socket
    // (aka an fdio pipe), where newlines are expected.
    return SocketWriteAll(socket_, str);
  }

  // If there's no socket, always report full success if anything got out.
  return debuglog_ ? static_cast<int>(str.size()) : -1;
}

void Log::TakeLogHandle(zx::handle handle) {
  assert(handle);
  zx_info_handle_basic_t info;
  zx_status_t status = handle.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) [[unlikely]] {
    CRASH_WITH_UNIQUE_BACKTRACE();
  }

  switch (info.type) {
    case ZX_OBJ_TYPE_DEBUGLOG:
      debuglog_ = zx::debuglog{handle.release()};
      break;
    case ZX_OBJ_TYPE_SOCKET:
      socket_ = zx::socket{handle.release()};
      break;
  }
}

}  // namespace ld
