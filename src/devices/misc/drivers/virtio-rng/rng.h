// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_MISC_DRIVERS_VIRTIO_RNG_RNG_H_
#define SRC_DEVICES_MISC_DRIVERS_VIRTIO_RNG_RNG_H_

#include <lib/dma-buffer/buffer.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>
#include <stdlib.h>
#include <zircon/compiler.h>

#include <memory>

namespace virtio {

class Ring;

class RngDevice : public Device {
 public:
  RngDevice(zx::bti bti, std::unique_ptr<Backend> backend);
  virtual ~RngDevice();

  zx_status_t Init() override;

  void IrqRingUpdate() override;
  void IrqConfigChange() override;
  const char* tag() const override { return "virtio-rng"; }

 protected:
 private:
  // TODO(https://fxbug.dev/42098992): The kernel should trigger entropy requests, instead of
  // relying on this userspace thread to push entropy whenever it wants to. As a temporary hack,
  // this thread pushes entropy to the kernel every 300 seconds instead.

  // the entry point for the entropy seeding thread
  static int SeedThreadEntry(void* arg);

  // the method called by SeedThreadEntry() to actually launch a request
  zx_status_t Request();

  // the thread that seeds the system CPRNG periodically
  thrd_t seed_thread_;

  // the virtio ring
  static constexpr uint16_t kRingIndex = 0;
  static constexpr uint16_t kRingSize = 1;
  Ring vring_{this};

  // the buffer used to receive entropy
  static constexpr size_t kBufferSize = ZX_CPRNG_ADD_ENTROPY_MAX_LEN;
  std::unique_ptr<dma_buffer::ContiguousBuffer> buf_;
};

class RngDriver : public fdf::DriverBase2 {
 public:
  static constexpr char kDriverName[] = "virtio-rng";

  RngDriver();
  ~RngDriver() override = default;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  std::unique_ptr<RngDevice> device_;
};

}  // namespace virtio

#endif  // SRC_DEVICES_MISC_DRIVERS_VIRTIO_RNG_RNG_H_
