// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <string.h>
#include <unistd.h>

#include <cerrno>
#include <cstdio>
#include <cstdlib>

int main(int argc, char** argv) {
  if (argc == 0) {
    exit(EXIT_FAILURE);
  }

  if (argc < 2) {
    goto usage;
  }

  if (strcmp(argv[1], "close_fd") == 0 && argc == 3) {
    int fd = atoi(argv[2]);
    if (close(atoi(argv[2])) != 0) {
      fprintf(stderr, "Failed to close FD %d: %d (%s)\n", fd, errno, strerror(errno));
      exit(EXIT_FAILURE);
    }
    return 0;
  }

  if (strcmp(argv[1], "set_cwd") == 0 && argc == 3) {
    if (chdir(argv[2]) != 0) {
      fprintf(stderr, "Failed to chdir to %s: %d (%s)\n", argv[2], errno, strerror(errno));
      exit(EXIT_FAILURE);
    }
    return 0;
  }

usage:
  fprintf(stderr, "Usage: %s <command> [<arg>]\n", argv[0]);
  fprintf(stderr, "commands:\n");
  fprintf(stderr, "\tclose_fd: close the specified FD and exit.\n");
  exit(EXIT_FAILURE);
}
