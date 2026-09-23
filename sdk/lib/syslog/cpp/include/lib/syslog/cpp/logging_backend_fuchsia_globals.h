// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYSLOG_CPP_LOGGING_BACKEND_FUCHSIA_GLOBALS_H_
#define LIB_SYSLOG_CPP_LOGGING_BACKEND_FUCHSIA_GLOBALS_H_

#include <zircon/availability.h>
#include <zircon/types.h>

namespace fuchsia_logging {

struct RawLogSettings;

namespace internal {

// These functions are an internal contract between the Fuchsia logging backend and the logging
// state shared library. API users should not call these directly, but they need to be exported to
// allow for global state management of logs within a single process.

extern "C" {

// Returns the current thread's koid.
zx_koid_t FuchsiaLogGetCurrentThreadKoid();

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

class LogState;

// Acquires the state lock.
void FuchsiaLogAcquireState();

// Updates the log state, requires that the state lock is held.
void FuchsiaLogSetStateLocked(LogState* new_state);

// Releases the state lock.
void FuchsiaLogReleaseState();

// Returns the current log state.
LogState* FuchsiaLogGetStateLocked();

#else

class Logger;

// Initializes the global logger. This is safe to call multiple times.
void FuchsiaLogInitGlobalLogger(const RawLogSettings*);

// Returns the global minimum severity. This is separate for performance reasons.
uint8_t FuchsiaLogGetGlobalMinSeverity();

// Returns the global logger. This will create a logger with default settings if one does not
// already exist.
Logger* FuchsiaLogGetGlobalLogger(RawLogSettings (*get_default_settings)());

// Writes a log record to a logger. The buffer should be in the appropriate diagnostics record
// format.
zx_status_t FuchsiaLogWrite(const Logger* logger, const void* buffer, size_t len);

// Calls `callback` for each tag for the logger.
void FuchsiaLogForEachTag(const Logger* logger, void* context,
                          void (*callback)(void* context, const char* tag));

#endif

}  // extern "C"

}  // namespace internal
}  // namespace fuchsia_logging

#endif  // LIB_SYSLOG_CPP_LOGGING_BACKEND_FUCHSIA_GLOBALS_H_
