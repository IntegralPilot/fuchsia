// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fdio/unsafe.h>
#include <stdio.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <iostream>

// A message we will send repeatedly to the /svc directory handle
struct ReadDirents {
  // Header
  uint32_t txid;
  uint8_t flags[3];
  uint8_t magic_number;
  uint64_t ordinal;
  // Body
  uint64_t size;
};

int main() {
  int svc_fd = open("/svc", O_DIRECTORY);
  if (svc_fd < 0) {
    perror("open(\"/svc\", O_DIRECTORY)");
    return errno;
  }
  fdio_t* svc_io = fdio_unsafe_fd_to_io(svc_fd);
  if (svc_io == nullptr) {
    std::cout << "Failed to get a fdio_t for /svc\n";
    return 1;
  }
  zx_handle_t svc_handle = fdio_unsafe_borrow_channel(svc_io);
  if (svc_handle == ZX_HANDLE_INVALID) {
    std::cout << "Failed to get channel handle for /svc\n";
    return 1;
  }

  // Initialize message
  ReadDirents message = {
      .txid = 1,
      .flags = {2, 0, 0},
      .magic_number = 1,
      .ordinal = 0x3582806bf27faa0a,
      .size = 8192,
  };
  int delay = 50;
  for (;;) {
    // Send the message
    std::cout << "sending message: " << message.txid << "\n";
    zx_status_t status = zx_channel_write(svc_handle, 0, &message, sizeof message, nullptr, 0);
    if (status != ZX_OK) {
      std::cout << "zx_channel_write failed: " << zx_status_get_string(status) << "\n";
      return 1;
    }
    // Increment the transaction id
    message.txid++;
    // Catch our breath
    usleep(delay);
  }
  return 0;
}
