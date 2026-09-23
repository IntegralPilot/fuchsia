// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/util.h"

#include <lib/syslog/cpp/macros.h>

namespace tracing {

std::ostream& operator<<(std::ostream& out, TransferStatus status) {
  switch (status) {
    case TransferStatus::kComplete:
      out << "complete";
      break;
    case TransferStatus::kProviderError:
      out << "provider error";
      break;
    case TransferStatus::kWriteError:
      out << "write error";
      break;
    case TransferStatus::kReceiverDead:
      out << "receiver dead";
      break;
  }

  return out;
}

std::ostream& operator<<(std::ostream& out, fuchsia_tracing::BufferDisposition disposition) {
  switch (disposition) {
    case fuchsia_tracing::BufferDisposition::kClearEntire:
      out << "clear-all";
      break;
    case fuchsia_tracing::BufferDisposition::kClearNondurable:
      out << "clear-nondurable";
      break;
    case fuchsia_tracing::BufferDisposition::kRetain:
      out << "retain";
      break;
    default:
      out << "unknown(" << static_cast<uint32_t>(disposition) << ")";
      break;
  }

  return out;
}

std::ostream& operator<<(std::ostream& out, fuchsia_tracing_controller::SessionState state) {
  switch (state) {
    case fuchsia_tracing_controller::SessionState::kReady:
      out << "ready";
      break;
    case fuchsia_tracing_controller::SessionState::kInitialized:
      out << "initialized";
      break;
    case fuchsia_tracing_controller::SessionState::kStarting:
      out << "starting";
      break;
    case fuchsia_tracing_controller::SessionState::kStarted:
      out << "started";
      break;
    case fuchsia_tracing_controller::SessionState::kStopping:
      out << "stopping";
      break;
    case fuchsia_tracing_controller::SessionState::kStopped:
      out << "stopped";
      break;
    case fuchsia_tracing_controller::SessionState::kTerminating:
      out << "terminating";
      break;
    default:
      out << "unknown(" << static_cast<uint32_t>(state) << ")";
      break;
  }

  return out;
}

}  // namespace tracing
