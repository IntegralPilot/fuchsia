// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_GPT_H_
#define SRC_FIRMWARE_PAVER_GPT_H_

#include <fidl/fuchsia.storage.partitions/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>

#include <string_view>

#include <gpt/gpt.h>

#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/config.h"
#include "src/firmware/paver/device-partitioner.h"
#include "src/lib/uuid/uuid.h"

namespace paver {

// Used as a search key for `GptDevicePartitioner.FindPartition`.
struct GptPartitionMetadata {
  std::string name;
  uuid::Uuid type_guid;
  uuid::Uuid instance_guid;
};

// Useful for when a GPT table is available (e.g. x86 devices). Provides common
// utility functions.
class GptDevicePartitioner {
 public:
  using FilterCallback = fit::function<bool(const GptPartitionMetadata&)>;

  struct InitializeGptResult {
    std::unique_ptr<GptDevicePartitioner> gpt;
    bool initialize_partition_tables;
  };

  // Find and initialize a GPT based device by querying fshost for the system GPT.
  //
  // If the system GPT was not formatted correctly, attempts to format it by calling
  // fuchsia.fshost/Recovery.InitSystemPartitionTable.
  static zx::result<InitializeGptResult> InitializeGpt(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config);

  // Returns a connection to the first matching partition.
  zx::result<std::unique_ptr<BlockPartitionClient>> FindPartition(FilterCallback filter) const;

  // Returns a connection to all matching partitions.
  zx::result<std::vector<std::unique_ptr<BlockPartitionClient>>> FindAllPartitions(
      FilterCallback filter) const;

  struct PartitionInitSpec {
   public:
    std::string name;
    uuid::Uuid type;
    // If zero, a random GUID will be assigned
    uuid::Uuid instance;
    // If nonzero, the partition will be allocated at the specific offset (and initialization will
    // fail if this overlaps with other partitions or the GPT itself).  If the value is zero, the
    // partition is dynamically allocated.  Dynamically allocated partitions must precede all
    // fixed-offset partitions in the list passed to ResetPartitionTables, otherwise they might be
    // allocated over the desired range.
    uint64_t start_block = 0;
    // Zero indicates an empty partition table entry; other fields are ignored.
    uint64_t size_bytes = 0;
    uint64_t flags = 0;

    static PartitionInitSpec ForKnownPartition(Partition partition, PartitionScheme scheme,
                                               size_t size_bytes);
  };

  // Wipes the partition table and resets it to `partitions`.
  // See fuchsia.storage.partitions.PartitionsManager/ResetPartitionTables.
  zx::result<> ResetPartitionTables(std::vector<PartitionInitSpec> partitions) const;

  const paver::BlockDevices& devices() { return devices_; }

  fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root() { return svc_root_; }

 private:
  GptDevicePartitioner(BlockDevices devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
                       uint64_t block_count, uint32_t block_size)
      : devices_(std::move(devices)),
        block_count_(block_count),
        block_size_(block_size),
        svc_root_(component::MaybeClone(svc_root)) {}

  const paver::BlockDevices devices_;
  const uint64_t block_count_;
  const uint64_t block_size_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
};

// TODO(69527): Remove this and migrate usages to |utf16_to_utf8|
inline void utf16_to_cstring(char* dst, const uint8_t* src, size_t charcount) {
  while (charcount > 0) {
    *dst++ = *src;
    src += 2;
    charcount -= 2;
  }
}

inline bool FilterByType(const GptPartitionMetadata& part, const uuid::Uuid& type) {
  return type == part.type_guid;
}

bool FilterByName(const GptPartitionMetadata& part, std::string_view name);

bool FilterByTypeAndName(const GptPartitionMetadata& part, const uuid::Uuid& type,
                         std::string_view name);

inline bool IsFvmPartition(const GptPartitionMetadata& part) {
  return FilterByType(part, GUID_FVM_VALUE) ||
         FilterByTypeAndName(part, GPT_FVM_TYPE_GUID, GPT_FVM_NAME);
}

bool IsFuchsiaSystemPartition(const PaverConfig& config, const GptPartitionMetadata& part);

// Returns true if the spec partition is Zircon A/B/R.
inline bool IsZirconPartitionSpec(const PartitionSpec& spec) {
  return spec.partition == Partition::kZirconA || spec.partition == Partition::kZirconB ||
         spec.partition == Partition::kZirconR;
}

inline bool IsEfiSystemPartition(const GptPartitionMetadata& part) {
  // Check for EFI system partition 'fuchsia-esp' or 'bootloader'.
  // And for legacy "efi-system" partition name.
  return FilterByTypeAndName(part, GUID_BOOTLOADER_VALUE, GUID_BOOTLOADER_NAME) ||
         // TODO(b/400314846) ARM emulator can be run using UEFI. But it uses
         // a mix of names and types for bootloader partition
         // FilterByTypeAndName(part, GUID_EFI_VALUE, GUID_EFI_NAME) ||
         FilterByName(part, GUID_EFI_NAME) || FilterByName(part, "efi-system");
}

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_GPT_H_
