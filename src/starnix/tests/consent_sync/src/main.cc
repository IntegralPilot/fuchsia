// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fcntl.h>
#include <unistd.h>

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>

#define CONSENT_PATH "/dev/consent"

int main() {
  int fd = open(CONSENT_PATH, O_RDWR);
  if (fd < 0) {
    perror("open");
    return 1;
  }

  // Test writing "1"
  if (write(fd, "1", 1) != 1) {
    perror("write 1");
    close(fd);
    return 1;
  }

  char buf[16];
  if (lseek(fd, 0, SEEK_SET) < 0) {
    perror("lseek");
    close(fd);
    return 1;
  }

  ssize_t n = read(fd, buf, sizeof(buf) - 1);
  if (n < 0) {
    perror("read");
    close(fd);
    return 1;
  }
  buf[n] = '\0';

  if (strcmp(buf, "1\n") != 0) {
    fprintf(stderr, "Expected '1\\n', got '%s'\n", buf);
    close(fd);
    return 1;
  }

  // Test writing "0"
  if (write(fd, "0", 1) != 1) {
    perror("write 0");
    close(fd);
    return 1;
  }

  if (lseek(fd, 0, SEEK_SET) < 0) {
    perror("lseek");
    close(fd);
    return 1;
  }

  n = read(fd, buf, sizeof(buf) - 1);
  if (n < 0) {
    perror("read");
    close(fd);
    return 1;
  }
  buf[n] = '\0';

  if (strcmp(buf, "0\n") != 0) {
    fprintf(stderr, "Expected '0\\n', got '%s'\n", buf);
    close(fd);
    return 1;
  }

  printf("PASS: consent_sync integration test\n");
  close(fd);
  return 0;
}
