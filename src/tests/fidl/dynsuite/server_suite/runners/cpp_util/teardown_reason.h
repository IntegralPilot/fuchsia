
// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_RUNNERS_CPP_UTIL_TEARDOWN_REASON_H_
#define SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_RUNNERS_CPP_UTIL_TEARDOWN_REASON_H_

#include <fidl/fidl.serversuite/cpp/common_types.h>
#include <lib/fidl/cpp/wire/coding_errors.h>
#include <lib/fidl/cpp/wire/status.h>

namespace cpp_util {

inline fidl_serversuite::TeardownReason GetTeardownReason(const fidl::UnbindInfo& info) {
  switch (info.reason()) {
    case fidl::Reason::kPeerClosedWhileReading:
      return fidl_serversuite::TeardownReason::kPeerClosed;
    case fidl::Reason::kUnbind:
    case fidl::Reason::kClose:
      return fidl_serversuite::TeardownReason::kVoluntaryShutdown;
    case fidl::Reason::kUnexpectedMessage:
      if (info.status() == ZX_ERR_PROTOCOL_NOT_SUPPORTED ||
          info.lossy_description() == fidl::internal::kCodingErrorInvalidWireFormatMetadata) {
        return fidl_serversuite::TeardownReason::kIncompatibleFormat;
      }
      if (info.lossy_description() == fidl::internal::kErrorInvalidHeader) {
        return fidl_serversuite::TeardownReason::kDecodeFailure;
      }
      return fidl_serversuite::TeardownReason::kUnexpectedMessage;
    case fidl::Reason::kEncodeError:
      if (info.lossy_description() == fidl::internal::kCodingErrorTooManyHandlesConsumed) {
        return fidl_serversuite::TeardownReason::kWriteFailure;
      }
      return fidl_serversuite::TeardownReason::kEncodeFailure;
    case fidl::Reason::kDecodeError:
      if (info.lossy_description() == fidl::internal::kCodingErrorDoesNotSupportV1Envelopes ||
          info.lossy_description() == fidl::internal::kCodingErrorUnsupportedWireFormatVersion) {
        return fidl_serversuite::TeardownReason::kIncompatibleFormat;
      }
      return fidl_serversuite::TeardownReason::kDecodeFailure;
    case fidl::Reason::kTransportError:
      return fidl_serversuite::TeardownReason::kWriteFailure;
    case fidl::Reason::kDispatcherError:
      ZX_PANIC("GetTeardownReason: this error should not happen in tests: %s",
               info.FormatDescription().c_str());
    default:
      ZX_PANIC("GetTeardownReason: missing a case for this error: %s",
               info.FormatDescription().c_str());
  }
}

}  // namespace cpp_util

#endif  // SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_RUNNERS_CPP_UTIL_TEARDOWN_REASON_H_
