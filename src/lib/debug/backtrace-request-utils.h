// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DEBUG_BACKTRACE_REQUEST_UTILS_H_
#define SRC_LIB_DEBUG_BACKTRACE_REQUEST_UTILS_H_

#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

// Returns true if the given exception type and general registers indicate
// an exception caused by backtrace_request().
bool is_backtrace_request(zx_excp_type_t excp_type, const zx_thread_state_general_regs_t* regs);

// The backtrace request is requesting for the calling thread, rather than all threads.
bool is_backtrace_request_current_thread(const zx_thread_state_general_regs_t* regs);

// The backtrace request is requesting for a specific thread by KOID.
bool is_backtrace_request_specific_thread(const zx_thread_state_general_regs_t* regs);

// Returns the KOID of the thread requested for backtrace.
uint64_t get_backtrace_request_koid(const zx_thread_state_general_regs_t* regs);

// Cleans up the backtrace request so that resuming |thread| will allow it to
// continue execution normally.
//
// This must only be called if is_backtrace_request() returned true, and only
// once per backtrace exception.
//
// |regs| may be modified by this function.
zx_status_t cleanup_backtrace_request(zx_handle_t thread, zx_thread_state_general_regs_t* regs);

#endif  // SRC_LIB_DEBUG_BACKTRACE_REQUEST_UTILS_H_
