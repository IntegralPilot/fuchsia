// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/features.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fit/nullable.h>
#include <zircon/compiler.h>

#include <array>
#include <iostream>
#include <string>
#include <string_view>

namespace fidl {

namespace internal {

const char* const kErrorInvalidHeader = "invalid header";
const char* const kErrorUnknownTxId = "unknown txid";
const char* const kErrorUnknownOrdinal = "unknown ordinal";
const char* const kErrorTransport = "underlying transport I/O error";
const char* const kErrorChannelUnbound = "unbound endpoint";
const char* const kErrorWaitOneFailed = "failed waiting to read from endpoint";
const char* const kErrorSyncEventBufferTooSmall =
    "received a larger message than allowed by the events";
const char* const kErrorSyncEventUnhandledTransitionalEvent = "unhandled transitional event";
const char* const kCallerAllocatedBufferTooSmall =
    "buffer provided to caller-allocating flavor is too small";
const char* const kUnknownMethod = "server did not recognize this method";
const char* const kUnsupportedFrameworkError =
    "server sent a framework_err value that is not supported";

bool IsFatalErrorUniversal(fidl::Reason error) {
  switch (error) {
    case Reason::__DoNotUse:
    case Reason::kCanceledDueToOtherError:
    case Reason::kUnknownMethod:
    case Reason::kPendingTwoWayCallPreventsUnbind:
      // These are not concrete errors.
      __builtin_abort();
    case Reason::kUnbind:
    case Reason::kClose:
    case Reason::kPeerClosedWhileReading:
    case Reason::kDispatcherError:
    case Reason::kTransportError:
      return true;
    case Reason::kUnexpectedMessage:
    case Reason::kEncodeError:
    case Reason::kDecodeError:
    case Reason::kAbandonedAsyncReply:
    case Reason::kReply:
      return false;
  }
}

fidl::Status ErrorFromUnbindInfo(fidl::UnbindInfo info) {
  // Depending on what kind of error caused teardown, we may want to propagate
  // the error to all other outstanding contexts.
  if (IsFatalErrorUniversal(info.reason())) {
    return info.ToError();
  }
  // These errors are specific to one call, whose corresponding context
  // would have been notified during |Dispatch| or making the call.
  return fidl::Status::Canceled(info);
}

fidl::OneWayStatus OneWayErrorFromUnbindInfo(fidl::UnbindInfo info) {
  fidl::Status error = ErrorFromUnbindInfo(info);
  if (error.is_peer_closed()) {
    return fidl::OneWayStatus{fidl::Status::Ok()};
  }
  return fidl::OneWayStatus{error};
}

}  // namespace internal

namespace {

constexpr const char* DescribeReason(Reason reason) {
  // The error descriptions are quite terse to save binary size.
  switch (reason) {
    case internal::kUninitializedReason:
      return nullptr;
    case Reason::kUnbind:
      return "user initiated unbind";
    case Reason::kClose:
      return "(server) user initiated close with epitaph";
    case Reason::kPendingTwoWayCallPreventsUnbind:
      return "pending two-way calls renders unbinding unsafe";
    case Reason::kCanceledDueToOtherError:
      return "being canceled by failure from another operation: ";
    case Reason::kPeerClosedWhileReading:
      return "peer closed";
    case Reason::kDispatcherError:
      return "dispatcher error";
    case Reason::kTransportError:
      return internal::kErrorTransport;
    case Reason::kEncodeError:
      return "encode error";
    case Reason::kDecodeError:
      return "decode error";
    case Reason::kUnexpectedMessage:
      return "unexpected message";
    case Reason::kUnknownMethod:
      return "unknown interaction";
    case Reason::kAbandonedAsyncReply:
      return "(server) async completer is discarded without a reply";
    case Reason::kReply:
      return "(server) reply failed";
  }
}

// A buffer of 256 bytes is sufficient for all tested results.
// If the description exceeds this length at runtime,
// the output will be truncated.
// We can increase the size if necessary.
using StatusFormattingBuffer = std::array<char, 256>;

}  // namespace

Status Status::Canceled(UnbindInfo cause) {
  Status status = cause.ToError();
  // Universal errors should be directly passed instead of tucked under a nested error.
  ZX_DEBUG_ASSERT(!internal::IsFatalErrorUniversal(status.reason_));
  return Status(ZX_ERR_CANCELED, Reason::kCanceledDueToOtherError, status.reason_, status.error_);
}

[[nodiscard]] std::string Status::FormatDescription() const {
  StatusFormattingBuffer buf;
  size_t length = FormatImpl(&*buf.begin(), sizeof(buf), /* from_unbind_info */ false);
  return std::string(&*buf.begin(), length);
}

[[nodiscard]] const char* Status::lossy_description() const {
  // If an error string was explicitly specified, use that.
  if (error_) {
    return error_;
  }
  // Otherwise, derive an error from |reason_|.
  return reason_description();
}

[[nodiscard]] const char* Status::reason_description() const { return DescribeReason(reason_); }

[[nodiscard]] const char* Status::underlying_reason_description() const {
  ZX_DEBUG_ASSERT(underlying_reason_ != Reason::kCanceledDueToOtherError);
  return DescribeReason(underlying_reason_);
}

size_t Status::FormatImpl(char* destination, size_t length, bool from_unbind_info) const {
  ZX_ASSERT(length > 0);
  int num_would_write = 0;

  // We use |snprintf| since it minimizes allocation and is faster
  // than output streams.
  if (!from_unbind_info && status_ == ZX_OK && reason_ == internal::kUninitializedReason) {
    num_would_write = snprintf(destination, length, "FIDL success");
  } else {
    const char* prelude = from_unbind_info ? "FIDL endpoint was unbound" : "FIDL operation failed";

    const char* status_meaning = [&] {
      switch (reason_) {
        // This reason may only appear in an |UnbindInfo|.
        case Reason::kClose:
          ZX_DEBUG_ASSERT(from_unbind_info);
          return "status of sending epitaph";
        case Reason::kPeerClosedWhileReading:
          if (status_ != ZX_ERR_PEER_CLOSED) {
            return "epitaph";
          }
          break;
        default:
          break;
      }
      return "status";
    }();

    const char* detail_prefix = error_ ? ", detail: " : "";
    const char* detail = error_ ? error_ : "";

    const char* underlying_description = underlying_reason_description();
    if (!underlying_description) {
      underlying_description = "";
    }

#if __FIDL_SUPPORT_HANDLES
    num_would_write = snprintf(destination, length, "%s due to %s%s, %s: %s (%d)%s%s", prelude,
                               reason_description(), underlying_description, status_meaning,
                               zx_status_get_string(status_), status_, detail_prefix, detail);
#else
    num_would_write =
        snprintf(destination, length, "%s due to %s%s, %s: %d%s%s", prelude, reason_description(),
                 underlying_description, status_meaning, status_, detail_prefix, detail);
#endif  // __FIDL_SUPPORT_HANDLES
  }

  ZX_ASSERT(num_would_write > 0);
  return static_cast<size_t>(num_would_write) >= length ? length - 1 : num_would_write;
}

std::ostream& operator<<(std::ostream& ostream, const Status& result) {
  StatusFormattingBuffer buf;
  size_t length = result.FormatImpl(&*buf.begin(), sizeof(buf), /* from_unbind_info */ false);
  ostream << std::string_view(&*buf.begin(), length);
  return ostream;
}

[[nodiscard]] std::string UnbindInfo::FormatDescription() const {
  StatusFormattingBuffer buf;
  size_t length = FormatImpl(&*buf.begin(), sizeof(buf), /* from_unbind_info */ true);
  return std::string(&*buf.begin(), length);
}

std::ostream& operator<<(std::ostream& ostream, const UnbindInfo& info) {
  StatusFormattingBuffer buf;
  size_t length = info.Status::FormatImpl(&*buf.begin(), sizeof(buf), /* from_unbind_info */ true);
  ostream << std::string_view(&*buf.begin(), length);
  return ostream;
}

size_t fidl::internal::DisplayError<fidl::Status>::Format(const fidl::Status& value,
                                                          char* destination, size_t capacity) {
  return value.FormatImpl(destination, capacity, false);
}

}  // namespace fidl
