// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern int get_library_value();

int main() {
  if (get_library_value() != 100) {
    return 1;
  }
  return 0;
}
