// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_LIB_STDOUT_TO_DEBUGLOG_CPP_STDOUT_TO_DEBUGLOG_H_
#define SRC_SYS_LIB_STDOUT_TO_DEBUGLOG_CPP_STDOUT_TO_DEBUGLOG_H_

#include <zircon/types.h>

namespace StdoutToDebuglog {

// Connect the stdout and stderr for this program to the kernel's debuglog. This
// This is used by some components in early boot that precede the archivist.
zx_status_t Init();

}  // namespace StdoutToDebuglog

#endif  // SRC_SYS_LIB_STDOUT_TO_DEBUGLOG_CPP_STDOUT_TO_DEBUGLOG_H_
