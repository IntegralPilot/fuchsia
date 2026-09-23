// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_FAKE_BLOCK_DEVICE_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_FAKE_BLOCK_DEVICE_H_

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <functional>
#include <map>
#include <optional>

#include "lib/fidl/cpp/wire/internal/transport_channel.h"
#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/range/range.h"

namespace block_client {

// A fake device implementing (most of) the BlockDevice interface on top of an in-memory VMO
// representing the device. This allows clients of the BlockDevice interface to test against this
// fake in-process instead of relying on a real block device.
//
// This device also supports pausing processing FIFO transactions to allow tests to emulate slow
// devices or validate behavior in intermediate states.
//
// This class is thread-safe.
// This class is not movable or copyable.
class FakeBlockDevice : public BlockDevice {
 public:
  struct Config {
    uint64_t block_count = 0;
    uint32_t block_size = 0;
    bool supports_trim = false;
    uint32_t max_transfer_size = fuchsia_storage_block::wire::kMaxTransferUnbounded;
  };
  explicit FakeBlockDevice(const Config&);
  FakeBlockDevice(uint64_t block_count, uint32_t block_size)
      : FakeBlockDevice({block_count, block_size, false}) {}
  FakeBlockDevice(const FakeBlockDevice&) = delete;
  FakeBlockDevice& operator=(const FakeBlockDevice&) = delete;
  FakeBlockDevice(FakeBlockDevice&& other) = delete;
  FakeBlockDevice& operator=(FakeBlockDevice&& other) = delete;

  ~FakeBlockDevice() override = default;

  /// Returns a VMO child reference of the block device.
  zx::result<zx::vmo> VmoChildReference() const;

  // Sets a callback which will be invoked for each FIFO request that is received by the block
  // device. (If the FIFO request targets a VMO, |vmo| will be set as well.)
  // Note that if any request in a FIFO transaction fails, the transaction is immediately aborted.
  // In that case, the failing request will still be sent into the callback, but the other requests
  // in the transaction may or may not also be sent into the callback. (In practice, requests are
  // processed in order, so all requests after the first failing request wouldn't be processed.)
  // Not thread safe.  Should be called only when the device is not active.
  using Hook = std::function<zx_status_t(const BlockFifoRequest& request, const zx::vmo* vmo)>;
  void set_hook(Hook hook) { hook_ = std::move(hook); }

  // When paused, this device will make FIFO operations block until Resume() is called. The device
  // is in the Resume() state by default.
  void Pause();
  void Resume();

  // Sets the number of blocks which may be written to the block device. Once |limit| is reached,
  // all following operations will return ZX_ERR_IO.
  //
  // May be "std::nullopt" to allow an unlimited count of blocks.
  void SetWriteBlockLimit(uint64_t limit);

  // Turns off the "write block limit".
  void ResetWriteBlockLimit();
  uint64_t GetWriteBlockCount() const;
  void ResetBlockCounts();

  void SetInfoFlags(fuchsia_storage_block::wire::DeviceFlag flags);
  void SetBlockCount(uint64_t block_count);
  void SetBlockSize(uint32_t block_size);
  bool IsRegistered(vmoid_t vmoid) const;

  // Wipes the device to a zeroed state.
  void Wipe();

  zx::result<std::string> GetTopologicalPath() const override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx::result<> Rebind(std::string_view url_suffix) const override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx_status_t VolumeGetInfo(
      fuchsia_storage_block::wire::VolumeManagerInfo* out_manager_info,
      fuchsia_storage_block::wire::VolumeInfo* out_volume_info) const override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t VolumeQuerySlices(const uint64_t* slices, size_t slices_count,
                                fuchsia_storage_block::wire::VsliceRange* out_ranges,
                                size_t* out_ranges_count) const override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t VolumeExtend(uint64_t offset, uint64_t length) override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t VolumeShrink(uint64_t offset, uint64_t length) override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t FifoTransaction(BlockFifoRequest* requests,
                              size_t count) override __TA_NO_THREAD_SAFETY_ANALYSIS;
  zx_status_t BlockGetInfo(fuchsia_storage_block::wire::BlockInfo* out_info) const override;
  zx_status_t BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out_vmoid) final;

 protected:
  // Resizes the block device to be at least |new_size| bytes.
  void ResizeDeviceToAtLeast(uint64_t new_size);

 private:
  void AdjustBlockDeviceSizeLocked(uint64_t new_size) __TA_REQUIRES(lock_);

  mutable std::mutex lock_;

  // For handling paused_ waiters. Use BlockOnPaused() to wait on this.
  mutable std::condition_variable pause_condition_;

  bool paused_ __TA_GUARDED(lock_) = false;

  // The number of transactions which may occur before I/O errors are returned
  // to callers. If "nullopt", no limit is set.
  std::optional<uint64_t> write_block_limit_ __TA_GUARDED(lock_) = std::nullopt;
  uint64_t write_block_count_ __TA_GUARDED(lock_) = 0;

  uint64_t block_count_ __TA_GUARDED(lock_) = 0;
  uint32_t block_size_ __TA_GUARDED(lock_) = 0;
  fuchsia_storage_block::wire::DeviceFlag block_info_flags_ __TA_GUARDED(lock_) = {};
  uint32_t max_transfer_size_ __TA_GUARDED(lock_) = 0;
  std::map<vmoid_t, zx::vmo> vmos_ __TA_GUARDED(lock_);
  zx::vmo block_device_ __TA_GUARDED(lock_);
  Hook hook_;
};

// An extension of FakeBlockDevice that allows for testing on FVM devices.
//
// This class is thread-safe.
// This class is not movable or copyable.
class FakeFVMBlockDevice : public FakeBlockDevice {
 public:
  FakeFVMBlockDevice(uint64_t block_count, uint32_t block_size, uint64_t slice_size,
                     uint64_t slice_capacity);

  zx_status_t FifoTransaction(BlockFifoRequest* requests, size_t count) final;
  zx_status_t VolumeGetInfo(fuchsia_storage_block::wire::VolumeManagerInfo* out_manager_info,
                            fuchsia_storage_block::wire::VolumeInfo* out_volume_info) const final;
  zx_status_t VolumeQuerySlices(const uint64_t* slices, size_t slices_count,
                                fuchsia_storage_block::wire::VsliceRange* out_ranges,
                                size_t* out_ranges_count) const final;
  zx_status_t VolumeExtend(uint64_t offset, uint64_t length) final;
  zx_status_t VolumeShrink(uint64_t offset, uint64_t length) final;

 private:
  mutable std::mutex fvm_lock_;

  fuchsia_storage_block::wire::VolumeManagerInfo manager_info_ __TA_GUARDED(fvm_lock_) = {};
  fuchsia_storage_block::wire::VolumeInfo volume_info_ __TA_GUARDED(fvm_lock_) = {};

  // Start Slice -> Range.
  std::map<uint64_t, range::Range<uint64_t>> extents_ __TA_GUARDED(fvm_lock_);
};

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_FAKE_BLOCK_DEVICE_H_
