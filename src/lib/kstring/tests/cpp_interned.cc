// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fxt/interned_category.h>

extern "C" {
const fxt::InternedString* get_cpp_hello_ptr() {
  using fxt::operator""_intern;
  return &"hello"_intern;
}

const fxt::InternedCategory* get_cpp_category_ptr() {
  using fxt::operator""_category;
  return &"hello_category"_category;
}
}
