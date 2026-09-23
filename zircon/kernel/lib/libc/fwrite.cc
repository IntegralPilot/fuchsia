// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdio.h>

size_t fwrite(const void* buf, size_t size, size_t n, FILE* f) {
  ktl::string_view str{reinterpret_cast<const char*>(buf), size * n};
  int wrote = f->Write(str);
  return wrote > 0 ? static_cast<size_t>(wrote) : 0;
}
