// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/error.h"

#include <stdarg.h>
#include <stdio.h>

namespace unwinder {

// We cannot call fxl::StringVPrintf because fxl depends on syslog and inspector library cannot
// depend on syslog.
Error::Error(const char* fmt, ...) {
  // This should be sufficient enough for error messages.
  constexpr size_t kStackBufferSize = 1024u;
  char buf[kStackBufferSize];

  va_list ap;
  va_start(ap, fmt);
  vsnprintf(buf, kStackBufferSize, fmt, ap);
  va_end(ap);

  has_err_ = true;
  msg_ = buf;
}

}  // namespace unwinder
