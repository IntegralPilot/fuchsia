// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdio.h>

int fputs(const char* s, FILE* f) {
  ktl::string_view str(s);
  return f->Write(str) == static_cast<int>(str.size()) ? 0 : -1;
}
