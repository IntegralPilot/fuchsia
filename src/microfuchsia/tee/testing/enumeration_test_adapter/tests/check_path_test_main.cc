// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>

int main(int argc, char** argv) {
  if (argc != 2) {
    fprintf(stderr, "wrong number of args: %d\n", argc);
    abort();
  }

  struct stat buf{};
  if (stat(argv[1], &buf) != 0) {
    perror("stat");
    abort();
  }
  return 0;
}
