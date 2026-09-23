// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

// This program creates one anonymous memory mapping then sleeps,
// such that the test can validate memory attribution reporting.
// When it receives a SIGINT signal, it exits.
int main(void) {
  const size_t num_pages_to_allocate = 4200;
  const size_t page_size = sysconf(_SC_PAGESIZE);

  void* addr = NULL;
  const size_t size = num_pages_to_allocate * page_size;
  const int prot = PROT_READ | PROT_WRITE;
  const int flags = MAP_SHARED | MAP_ANONYMOUS;
  const int fd = -1;
  const off_t offset = 0;

  addr = mmap(addr, size, prot, flags, fd, offset);
  if (addr == MAP_FAILED) {
    exit(EXIT_FAILURE);
  }

  // Populate those pages.
  for (size_t i = 0; i < size; i++) {
    ((volatile char*)addr)[i] = (char)(1);
  }

  fprintf(stdout, "mmap_anonymous_then_sleep did mmap\n");
  fflush(stdout);

  while (1) {
    if (sleep(10) != 0 && errno == EINTR) {
      break;
    }
  }

  return 0;
}
