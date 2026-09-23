// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PLATFORM_INTERRUPT_H
#define PLATFORM_INTERRUPT_H

#include <lib/magma/util/macros.h>

namespace magma {

class PlatformPort;

// Created from a PlatformPciDevice.
class PlatformInterrupt {
 public:
  PlatformInterrupt() {}

  virtual ~PlatformInterrupt() {}

  virtual uint64_t global_id() const = 0;
  virtual void Signal() = 0;
  virtual bool Wait() = 0;
  virtual void Complete() = 0;
  virtual void Ack() = 0;
  virtual bool Bind(PlatformPort* port, uint64_t key) = 0;
  virtual bool Unbind(PlatformPort* port) = 0;

  virtual uint64_t GetMicrosecondsSinceLastInterrupt() = 0;

 private:
  PlatformInterrupt(const PlatformInterrupt&) = delete;
  void operator=(const PlatformInterrupt&) = delete;
};

}  // namespace magma

#endif  // PLATFORM_INTERRUPT_H
