// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PLATFORM_PORT_H
#define PLATFORM_PORT_H

#include <lib/magma/util/status.h>

#include <memory>

namespace magma {

class PlatformPort {
 public:
  static std::unique_ptr<PlatformPort> Create();

  virtual ~PlatformPort() {}

  // Closes the port. This will cause any thread blocked in Wait to return an error.
  virtual void Close() = 0;

  // Waits for a port to return a packet.
  // If a packet is available before the timeout expires, `key_out` will be set.
  // If the packet is an interrupt, then `*trigger_time_out` will be set to the hardware interrupt
  // timestamp.
  virtual Status Wait(uint64_t* key_out, uint64_t timeout_ms, uint64_t* trigger_time_out) = 0;

  Status Wait(uint64_t* key_out) { return Wait(key_out, UINT64_MAX); }
  Status Wait(uint64_t* key_out, uint64_t timeout_ms) { return Wait(key_out, timeout_ms, nullptr); }
};

}  // namespace magma

#endif  // PLATFORM_PORT_H
