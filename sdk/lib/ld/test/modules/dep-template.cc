// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/compiler.h>

#include <initializer_list>

DECLS

extern "C" __EXPORT int64_t SYM() {
  int64_t total = VALUE;
  for (int64_t (*func)() : std::initializer_list<int64_t (*)()>{DEP_SYMS}) {
    total += func();
  }
  return total;
}
