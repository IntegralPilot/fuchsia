// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main() {
  int fd = open("/dev/booted", O_RDWR);
  if (fd < 0) {
    abort();
  }
  char buf[1];
  memset(buf, 0xff, sizeof(buf));

  ssize_t count = read(fd, buf, 1);
  if (count < 0) {
    abort();
  }
  if (count != 1) {
    abort();
  }
  if (buf[0] != 0) {
    abort();
  }

  count = write(fd, "1", 1);
  if (count < 0) {
    abort();
  }
  if (count != 1) {
    abort();
  }

  count = read(fd, buf, 1);
  if (count < 0) {
    abort();
  }
  if (count != 1) {
    abort();
  }
  if (buf[0] != 1) {
    abort();
  }
}
