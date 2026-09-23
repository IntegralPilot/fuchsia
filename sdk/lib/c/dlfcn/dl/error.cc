// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "error.h"

#include <cstdarg>
#include <cstdio>

namespace dl {

// This has to be written twice as method and constructor because varargs, but
// they will probably be folded by ICF.
Error::Error(const char* format, ...) {
  va_list args;
  va_start(args, format);
  Printf(format, args);
  va_end(args);
}

void Error::Printf(const char* format, ...) {
  va_list args;
  va_start(args, format);
  Printf(format, args);
  va_end(args);
}

void Error::Printf(const char* format, va_list args) {
  // The object must be freshly default-constructed.
  assert(!buffer_);
  assert(size_ == kUnused);

  // vasprintf yields a malloc'd buffer on success.
  int n = vasprintf(&buffer_, format, args);

  if (n < 0) [[unlikely]] {
    // Leave the special marker that the object is "set" but there is no string
    // allocated because it couldn't be.
    buffer_ = nullptr;  // vasprintf may set it unpredictably on error.
    size_ = kAllocationFailure;
    return;
  }

  assert(n > 0);
  size_ = static_cast<size_t>(n);
}

}  // namespace dl
