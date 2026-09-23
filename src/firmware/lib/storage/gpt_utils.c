// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/storage/gpt_utils.h>

static bool CheckAdd(size_t lhs, size_t rhs, size_t* out) {
  *out = lhs + rhs;
  return *out > rhs;
}

static bool CheckMul(size_t lhs, size_t rhs, size_t* out) {
  *out = lhs * rhs;
  return (lhs == 0) || (*out / lhs == rhs);
}

// Dependency of third_party/vboot_reference code
VbError_t VbExDiskRead(VbExDiskHandle_t handle, uint64_t lba_start, uint64_t lba_count,
                       void* buffer) {
  FuchsiaFirmwareStorage* ops = (FuchsiaFirmwareStorage*)handle;
  return FuchsiaFirmwareStorageRead(ops, lba_start * ops->block_size, lba_count * ops->block_size,
                                    buffer)
             ? VBERROR_SUCCESS
             : VBERROR_UNKNOWN;
}

// Dependency of third_party/vboot_reference code
VbError_t VbExDiskWrite(VbExDiskHandle_t handle, uint64_t lba_start, uint64_t lba_count,
                        const void* buffer) {
  FuchsiaFirmwareStorage* ops = (FuchsiaFirmwareStorage*)handle;
  return FuchsiaFirmwareStorageWriteConst(ops, lba_start * ops->block_size,
                                          lba_count * ops->block_size, buffer)
             ? VBERROR_SUCCESS
             : VBERROR_UNKNOWN;
}

uint8_t VbExOverrideGptEntryPriority(const GptEntry* e) { return 0; }

static bool InitFuchsiaFirmwareStorageGptData(FuchsiaFirmwareStorage* ops, GptData* data) {
  data->sector_bytes = (uint32_t)ops->block_size;
  data->streaming_drive_sectors = ops->total_blocks;
  data->gpt_drive_sectors = ops->total_blocks;

  // TODO(b/262617680): This requires malloc. Consider changing to a sysdeps like vboot_gpt_malloc()
  if (AllocAndReadGptData(ops, data)) {
    return false;
  }

  if (GptInit(data) != GPT_SUCCESS) {
    (void)WriteAndFreeGptData(ops, data);
    return false;
  }

  return true;
}

bool FuchsiaFirmwareStorageFreeGptData(FuchsiaFirmwareStorage* ops, GptData* data) {
  return WriteAndFreeGptData(ops, data) == GPT_SUCCESS;
}

bool FuchsiaFirmwareStorageSyncGpt(FuchsiaFirmwareStorage* ops, GptData* out_gpt_data) {
  GptData data;
  return InitFuchsiaFirmwareStorageGptData(ops, &data) &&
         FuchsiaFirmwareStorageFreeGptData(ops, &data) &&
         InitFuchsiaFirmwareStorageGptData(ops, out_gpt_data);
}

static bool FindGptEntry(const GptData* data, const char* name, GptEntry* entry) {
  size_t len = strlen(name);
  if ((len + 1) > sizeof(entry->name)) {
    return false;
  }

  // A simplified utf8 vs utf16 comparison
  GptHeader header;
  memcpy(&header, data->primary_header, sizeof(header));
  bool matched = false;
  for (size_t i = 0; !matched && i < header.number_of_entries; i++) {
    memcpy(entry, data->primary_entries + i * sizeof(GptEntry), sizeof(GptEntry));
    matched = true;
    for (size_t j = 0; matched && j <= len; j++) {
      matched = entry->name[j] == (j < len ? name[j] : 0);
    }
  }

  return matched;
}

bool FuchsiaFirmwareStorageGetPartitionSize(FuchsiaFirmwareStorage* ops, const GptData* data,
                                            const char* name, size_t* out) {
  GptEntry entry;
  if (!FindGptEntry(data, name, &entry)) {
    return false;
  }
  *out = (entry.ending_lba - entry.starting_lba + 1) * ops->block_size;
  return true;
}

// Check and convert offset within a partition to absolute offset of the ops.
static bool CheckAndComputePartitionAbsRange(FuchsiaFirmwareStorage* ops, const GptData* data,
                                             const char* name, size_t* offset, size_t size) {
  GptEntry entry;
  if (!FindGptEntry(data, name, &entry)) {
    firmware_storage_log("Failed to find partition: %s\n", name);
    return false;
  }

  size_t partition_abs_offset = 0;
  if (!CheckMul(entry.starting_lba, ops->block_size, &partition_abs_offset)) {
    return false;
  }

  size_t partition_abs_end_blk = 0;
  if (!CheckAdd(entry.ending_lba, 1, &partition_abs_end_blk)) {
    return false;
  }

  size_t partition_abs_end = 0;
  if (!CheckMul(partition_abs_end_blk, ops->block_size, &partition_abs_end)) {
    return false;
  }

  size_t abs_offset = 0;
  if (!CheckAdd(partition_abs_offset, *offset, &abs_offset)) {
    return false;
  }

  size_t abs_end = 0;
  if (!CheckAdd(abs_offset, size, &abs_end)) {
    return false;
  }

  if (abs_end > partition_abs_end) {
    firmware_storage_log("offset + size overflows partition\n");
    return false;
  }

  *offset = abs_offset;
  return true;
}

bool FuchsiaFirmwareStorageGptWrite(FuchsiaFirmwareStorage* ops, const GptData* data,
                                    const char* name, size_t offset, size_t size, void* src) {
  return CheckAndComputePartitionAbsRange(ops, data, name, &offset, size) &&
         FuchsiaFirmwareStorageWrite(ops, offset, size, src);
}

bool FuchsiaFirmwareStorageGptWriteConst(FuchsiaFirmwareStorage* ops, const GptData* data,
                                         const char* name, size_t offset, size_t size,
                                         const void* src) {
  return CheckAndComputePartitionAbsRange(ops, data, name, &offset, size) &&
         FuchsiaFirmwareStorageWriteConst(ops, offset, size, src);
}

bool FuchsiaFirmwareStorageGptRead(FuchsiaFirmwareStorage* ops, const GptData* data,
                                   const char* name, size_t offset, size_t size, void* dst) {
  return CheckAndComputePartitionAbsRange(ops, data, name, &offset, size) &&
         FuchsiaFirmwareStorageRead(ops, offset, size, dst);
}
