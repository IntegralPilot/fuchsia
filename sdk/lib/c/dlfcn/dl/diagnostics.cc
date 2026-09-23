// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "diagnostics.h"

#include <cstdarg>

namespace dl {

// This is effectively identical to Error::Printf but has to be written out
// again because varargs.  Since error_ is the first member, it should actually
// be folded with Error::Printf by ICF.
void DiagnosticsReport::Printf(const char* format, ...) {
  va_list args;
  va_start(args, format);
  error_.Printf(format, args);
  va_end(args);
}

}  // namespace dl
