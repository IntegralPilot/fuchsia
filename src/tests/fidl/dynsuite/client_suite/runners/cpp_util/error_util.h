// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_FIDL_DYNSUITE_CLIENT_SUITE_RUNNERS_CPP_UTIL_ERROR_UTIL_H_
#define SRC_TESTS_FIDL_DYNSUITE_CLIENT_SUITE_RUNNERS_CPP_UTIL_ERROR_UTIL_H_

#include <fidl/fidl.clientsuite/cpp/fidl.h>
#include <lib/fidl/cpp/wire/status.h>

namespace clienttest_util {

inline fidl_clientsuite::FidlErrorKind ClassifyError(const fidl::Status& status) {
  ZX_ASSERT(!status.ok());
  auto reason = status.reason();
  if (reason == fidl::Reason::kCanceledDueToOtherError) {
    reason = status.underlying_reason().value();
  }
  switch (reason) {
    case fidl::Reason::kUnbind:
    case fidl::Reason::kClose:
    case fidl::Reason::kDispatcherError:
    case fidl::Reason::kTransportError:
    case fidl::Reason::kEncodeError:
      return fidl_clientsuite::FidlErrorKind::kOtherError;
    case fidl::Reason::kUnexpectedMessage:
      if (status.lossy_description() == ::fidl::internal::kErrorInvalidHeader) {
        return fidl_clientsuite::FidlErrorKind::kDecodingError;
      }
      return fidl_clientsuite::FidlErrorKind::kUnexpectedMessage;
    case fidl::Reason::kPeerClosedWhileReading:
      return fidl_clientsuite::FidlErrorKind::kChannelPeerClosed;
    case fidl::Reason::kDecodeError:
      return fidl_clientsuite::FidlErrorKind::kDecodingError;
    case fidl::Reason::kUnknownMethod:
      return fidl_clientsuite::FidlErrorKind::kUnknownMethod;
    default:
      auto description = status.FormatDescription();
      ZX_PANIC("clienttest_util::ClassifyError is missing a case for this error: %s",
               description.c_str());
  }
}

}  // namespace clienttest_util

#endif  // SRC_TESTS_FIDL_DYNSUITE_CLIENT_SUITE_RUNNERS_CPP_UTIL_ERROR_UTIL_H_
