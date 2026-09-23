// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdio.h>

int fputc(int c, FILE* f) {
  const unsigned char uc = static_cast<unsigned char>(c);
  return f->Write({reinterpret_cast<const char*>(&uc), 1}) == 1 ? uc : -1;
}

[[gnu::alias("fputc")]] int putc(int c, FILE* f);
