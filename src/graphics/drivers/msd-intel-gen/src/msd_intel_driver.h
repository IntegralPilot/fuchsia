// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_DRIVER_H
#define MSD_DRIVER_H

#include <lib/magma_service/msd.h>

#include <memory>

class MsdIntelDriver : public msd::Driver {
 public:
  // msd::Driver implementation.
  void MsdConfigure(uint32_t flags) override { configure_flags_ = flags; }
  std::unique_ptr<msd::Device> MsdCreateDevice(msd::DeviceHandle* device_handle) override;
  std::unique_ptr<msd::Buffer> MsdImportBuffer(zx::vmo vmo, uint64_t client_id) override;
  magma_status_t MsdImportSemaphore(zx::handle handle, uint64_t client_id, uint64_t flags,
                                    std::unique_ptr<msd::Semaphore>* out) override;

  uint32_t configure_flags() { return configure_flags_; }

 private:
  uint32_t configure_flags_ = 0;
};

#endif  // MSD_DRIVER_H
