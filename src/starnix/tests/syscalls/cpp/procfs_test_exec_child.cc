// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unistd.h>

int main(void) {
  // Signal to the test that the program has started.
  char buf[] = "poke";
  write(STDOUT_FILENO, buf, sizeof buf);

  // Wait for the test to signal that the program can exit.
  read(STDIN_FILENO, buf, sizeof buf);

  return 0;
}
