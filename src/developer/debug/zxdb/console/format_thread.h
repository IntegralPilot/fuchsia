// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FORMAT_THREAD_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FORMAT_THREAD_H_

#include <optional>

#include "src/developer/debug/zxdb/format/output_buffer.h"

namespace zxdb {

class ConsoleContext;
class Thread;
struct StopInfo;

// Returns an OutputBuffer with context about |thread| (and |info|, if provided). The thread will be
// examined to check for exceptions even if |info| is not passed here. Source code information is
// always included, with fallback to assembly if symbols are not available. Use
// |override_show_exception_info| to suppress adding the exception information to the returned
// buffer, even if present.
OutputBuffer FormatThreadStop(ConsoleContext* context, const Thread* thread,
                              std::optional<StopInfo> info, bool override_show_exception_info);

// Returns an OutputBuffer with a short one-line description of |thread|. No source or exception
// information will be printed, even if available.
OutputBuffer FormatThreadConcise(const ConsoleContext* context, const Thread* thread);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_FORMAT_THREAD_H_
