// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This program is used to test the process_builder library's handling of
// statically linked PIE executables.
//
// It uses normal (static) libc startup to find another channel handle with
// type PA_USER0, and then reads a message from that channel and echos it back
// on the same channel. The test uses this echo to confirm that the process was
// loaded correctly.

#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

int main() {
  zx_handle_t user_chan = zx_take_startup_handle(PA_HND(PA_USER0, 0));
  if (user_chan == ZX_HANDLE_INVALID) {
    return 1;
  }

  // Read a message from the PA_USER0 channel and echo it back. Note that
  // ZX_ERR_SHOULD_WAIT isn't handled here; the test should make sure to write
  // to the channel before starting us.
  char buffer[128];
  uint32_t actual_bytes, actual_handles;
  zx_status_t status = zx_channel_read(user_chan, 0, buffer, nullptr, sizeof(buffer), 0,
                                       &actual_bytes, &actual_handles);
  if (status != ZX_OK) {
    return status;
  }

  // Write the same message back and exit.
  return zx_channel_write(user_chan, 0, buffer, actual_bytes, nullptr, 0);
}
