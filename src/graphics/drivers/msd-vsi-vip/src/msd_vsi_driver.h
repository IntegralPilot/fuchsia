// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_VSI_DRIVER_H
#define MSD_VSI_DRIVER_H

#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

class MsdVsiDriver : public msd::Driver {
 public:
  MsdVsiDriver() : magic_(kMagic) {}

  virtual ~MsdVsiDriver() {}

  static MsdVsiDriver* cast(msd::Driver* driv) {
    DASSERT(driv);
    auto driver = static_cast<MsdVsiDriver*>(driv);
    DASSERT(driver->magic_ == kMagic);
    return driver;
  }

  static std::unique_ptr<MsdVsiDriver> Create();
  static void Destroy(MsdVsiDriver* drv);

  void MsdConfigure(uint32_t flags) override { configure_flags_ = flags; }
  std::unique_ptr<msd::Device> MsdCreateDevice(msd::DeviceHandle* device_handle) override;
  std::unique_ptr<msd::Buffer> MsdImportBuffer(zx::vmo vmo, uint64_t client_id) override;
  magma_status_t MsdImportSemaphore(zx::handle handle, uint64_t client_id, uint64_t flags,
                                    std::unique_ptr<msd::Semaphore>* out) override;

  uint32_t configure_flags() const { return configure_flags_; }

 private:
  uint32_t configure_flags_ = 0;
  static const uint32_t kMagic = 0x64726976;  //"driv"
  const uint32_t magic_;
};

#endif  // MSD_VSI_DRIVER_H
