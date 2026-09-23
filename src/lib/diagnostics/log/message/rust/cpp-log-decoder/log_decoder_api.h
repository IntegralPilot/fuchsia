// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DIAGNOSTICS_LOG_MESSAGE_RUST_CPP_LOG_DECODER_LOG_DECODER_API_H_
#define SRC_LIB_DIAGNOSTICS_LOG_MESSAGE_RUST_CPP_LOG_DECODER_LOG_DECODER_API_H_
#include <fuchsia/logger/cpp/fidl.h>
#include <lib/fpromise/result.h>

#include <string>
#include <string_view>
#include <vector>

#include "log_decoder.h"

namespace log_decoder {

/// Converts a decoded FFI `LogMessage` into a FIDL `fuchsia::logger::LogMessage`.
///
/// Converts timestamps, tags, severity, dropped log counts, and formats the message
/// along with any key-value pairs (`"[file(line)] message key=value ..."`).
fpromise::result<fuchsia::logger::LogMessage, std::string> ToFidlLogMessage(
    const LogMessage& message);

}  // namespace log_decoder

#endif  // SRC_LIB_DIAGNOSTICS_LOG_MESSAGE_RUST_CPP_LOG_DECODER_LOG_DECODER_API_H_
