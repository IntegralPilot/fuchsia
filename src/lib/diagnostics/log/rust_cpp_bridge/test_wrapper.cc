// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/syslog/cpp/macros.h"

extern "C" {
void emit_trace_log_for_testing() { FX_LOGS(TRACE) << "TRACE TEST MESSAGE FROM C++"; }
void emit_debug_log_for_testing() { FX_LOGS(DEBUG) << "DEBUG TEST MESSAGE FROM C++"; }
void emit_info_log_for_testing() { FX_LOGS(INFO) << "INFO TEST MESSAGE FROM C++"; }
void emit_warning_log_for_testing() { FX_LOGS(WARNING) << "WARNING TEST MESSAGE FROM C++"; }
void emit_error_log_for_testing() { FX_LOGS(ERROR) << "ERROR TEST MESSAGE FROM C++"; }
}
