/* Copyright 2020 The Fuchsia Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

#ifndef SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_TEST_MOCK_ZIRCON_BOOT_OPS_H_
#define SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_TEST_MOCK_ZIRCON_BOOT_OPS_H_

#include <lib/zbi/zbi.h>
#include <lib/zircon_boot/zircon_boot.h>
#include <lib/zx/result.h>

#include <functional>
#include <optional>
#include <span>
#include <string>
#include <unordered_map>
#include <vector>

class MockZirconBootOps {
 public:
  enum class LockStatus {
    kLocked,
    kUnlocked,
  };

  // The amount of extra ZBI space get_kernel_load_buffer() op must be able to
  // reserve on top of the requested size to account for add_zbi_items() items.
  static constexpr size_t kExtraZbiItemsCapacity = 0x1000;

  MockZirconBootOps() = default;

  // Basic ops
  zx::result<> ReadFromPartition(const char* part, size_t offset, size_t size, void* out);
  zx::result<> WriteToPartition(const char* part, size_t offset, size_t size, const void* payload);
  zx::result<size_t> GetPartitionSize(const char* part);
  void Boot(zbi_header_t* image, size_t capacity);
  std::optional<AbrSlotIndex> GetBootedSlot() const { return booted_slot_; }
  const std::vector<uint8_t>& GetBootedImage() const { return booted_image_; }
  AbrOps GetAbrOps();
  void SetAddDeviceZbiItemsMethod(
      std::function<bool(zbi_header_t*, size_t, const AbrSlotIndex*)> method);

  // Adds a partition to the fake disk.
  void AddPartition(const char* name, size_t size);
  // Removes an existing partition, which will cause a test failure on any
  // further read or write access. Useful to ensure code-under-test never
  // touches the given partition past a certain point.
  void RemovePartition(const char* name);
  // Removes all partitions, any further disk access will cause test failure.
  void RemoveAllPartitions();

  // Firmware ABR related
  AbrSlotIndex GetFirmwareSlot() { return firmware_slot_; }
  void SetFirmwareSlot(AbrSlotIndex slot) { firmware_slot_ = slot; }
  void Reboot(bool force_recovery);

  // Verified boot related.
  void WriteRollbackIndex(size_t location, uint64_t rollback_index);
  zx::result<uint64_t> ReadRollbackIndex(size_t location) const;
  LockStatus GetDeviceLockStatus() { return device_locked_status_; }
  void SetDeviceLockStatus(LockStatus status) { device_locked_status_ = status; }
  AvbAtxPermanentAttributes GetPermanentAttributes();
  void SetPermanentAttributes(const AvbAtxPermanentAttributes& permanent_attribute);
  ZirconBootOps GetZirconBootOps();
  ZirconBootOps GetZirconBootOpsWithAvb();
  void SetRandomData(const std::vector<uint8_t>& data) { random_data_ = data; }

  // ZirconBootOps callback to provide the kernel load buffer.
  uint8_t* GetKernelLoadBuffer(size_t* size);
  // Resizes the kernel load buffer; must be called before loading.
  void SetKernelLoadBufferSize(size_t size);
  // Returns the kernel load buffer for test examination.
  const std::vector<uint8_t>& GetKernelLoadBuffer() const { return load_buffer_; }

 private:
  std::unordered_map<std::string, std::vector<uint8_t>> partitions_;
  std::unordered_map<size_t, uint64_t> rollback_index_;
  std::unordered_map<std::string, std::vector<uint8_t>> persistent_value_;
  LockStatus device_locked_status_ = LockStatus::kLocked;
  AbrSlotIndex firmware_slot_;
  std::vector<uint8_t> booted_image_;
  std::optional<AbrSlotIndex> booted_slot_;
  std::vector<uint8_t> load_buffer_;
  std::function<bool(zbi_header_t*, size_t, const AbrSlotIndex*)> add_zbi_items_;
  AvbAtxPermanentAttributes permanent_attributes_;
  std::vector<uint8_t> random_data_;

  zx::result<std::span<uint8_t>> GetPartitionSpan(const char* name, size_t offset, size_t size);

  // For assigning to ZirconBootOps
  static bool ReadFromPartition(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                                void* dst, size_t* read_size);
  static bool WriteToPartition(ZirconBootOps* ops, const char* part, size_t offset, size_t size,
                               const void* src, size_t* write_size);
  static bool FirmwareCanBootKernelSlot(ZirconBootOps* ops, AbrSlotIndex kernel_slot, bool* out);
  static void Reboot(ZirconBootOps* ops, bool force_recovery);
  static void Boot(ZirconBootOps* ops, zbi_header_t* image, size_t capacity);
  static bool AddDeviceZbiItems(ZirconBootOps* zb_ops, zbi_header_t* image, size_t capacity,
                                const AbrSlotIndex* slot);

  // For assigning to ZirconVBootOps
  static bool GetPartitionSize(ZirconBootOps* ops, const char* part, size_t* out);
  static bool ReadRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                uint64_t* out_rollback_index);
  static bool WriteRollbackIndex(ZirconBootOps* ops, size_t rollback_index_location,
                                 uint64_t rollback_index);
  static bool ReadIsDeviceLocked(ZirconBootOps* ops, bool* out_is_locked);
  static bool ReadPermanentAttributes(ZirconBootOps* ops, AvbAtxPermanentAttributes* attribute);
  static bool ReadPermanentAttributesHash(ZirconBootOps* ops, uint8_t hash[AVB_SHA256_DIGEST_SIZE]);
  static uint8_t* GetKernelLoadBuffer(ZirconBootOps* ops, size_t* size);
  static bool GetRandom(ZirconBootOps* ops, size_t num_bytes, uint8_t* output);
};

#endif  // SRC_FIRMWARE_LIB_ZIRCON_BOOT_INCLUDE_LIB_ZIRCON_BOOT_TEST_MOCK_ZIRCON_BOOT_OPS_H_
