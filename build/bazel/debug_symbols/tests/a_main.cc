// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <ctype.h>
#include <stdio.h>

int main() {
  char buffer[16] = "Hello world!";
  for (char& ch : buffer) {
    ch = ::toupper(ch);
  }
  fprintf(stdout, "%s\n", buffer);
  return 0;
}
