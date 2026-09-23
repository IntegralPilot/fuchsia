// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>
#include <stdlib.h>

#include <string>

int main(int argc, char** argv) {
  int kmsg_fd = open("/dev/kmsg", O_WRONLY);
  std::string message("Hello, Starnix logs!");
  write(kmsg_fd, message.c_str(), message.length());

  // Generate a coredump so there's a tombstone in inspect with our process koid.
  abort();
}
