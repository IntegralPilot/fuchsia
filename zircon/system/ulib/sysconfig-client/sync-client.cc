// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// clang-format off
#include <lib/sysconfig/sync-client.h>
#include <lib/sysconfig/sysconfig-header.h>
// clang-format on

#include <fidl/fuchsia.hardware.skipblock/cpp/wire.h>
#include <lib/cksum.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <string.h>
#include <zircon/hw/gpt.h>
#include <zircon/status.h>

#include <array>

#include <fbl/algorithm.h>

namespace sysconfig {

namespace {

constexpr size_t kKilobyte = 1 << 10;
constexpr size_t kSysconfigSize = 60 * kKilobyte;
constexpr size_t kABRMetadtaSize = 4 * kKilobyte;
constexpr size_t kVerifiedBootMetadataSize = 64 * kKilobyte;

struct AstroSysconfigPartition {
  uint8_t sysconfig[kSysconfigSize];
  uint8_t abr_metadata[kABRMetadtaSize];
  uint8_t vb_metadata_a[kVerifiedBootMetadataSize];
  uint8_t vb_metadata_b[kVerifiedBootMetadataSize];
  uint8_t vb_metadata_r[kVerifiedBootMetadataSize];
} __PACKED;

static_assert(sizeof(AstroSysconfigPartition) == 256 * kKilobyte);

constexpr size_t kAstroSysconfigPartitionSize = sizeof(AstroSysconfigPartition);

const sysconfig_header kLegacyLayout = {
    .magic = SYSCONFIG_HEADER_MAGIC_ARRAY,
    .sysconfig_data = {offsetof(AstroSysconfigPartition, sysconfig), kSysconfigSize},
    .abr_metadata = {offsetof(AstroSysconfigPartition, abr_metadata), kABRMetadtaSize},
    .vb_metadata_a = {offsetof(AstroSysconfigPartition, vb_metadata_a), kVerifiedBootMetadataSize},
    .vb_metadata_b = {offsetof(AstroSysconfigPartition, vb_metadata_b), kVerifiedBootMetadataSize},
    .vb_metadata_r = {offsetof(AstroSysconfigPartition, vb_metadata_r), kVerifiedBootMetadataSize},
    .crc_value = 0,  // Don't care. The header is only a default returned by
                     // GetHeader() and should never be written to storage.
};

constexpr size_t kAstroPageSize = 4 * kKilobyte;

constexpr zx_vm_option_t kVmoRw = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;

zx::result<fidl::ClientEnd<skipblock::SkipBlock>> FindSysconfigPartition(
    fidl::UnownedClientEnd<fuchsia_io::Directory> dev) {
  zx::result directory = component::OpenDirectoryAt(dev, "class/skip-block");
  if (directory.is_error()) {
    return directory.take_error();
  }
  zx::result watch_result =
      device_watcher::WatchDirectoryForItems<zx::result<fidl::ClientEnd<skipblock::SkipBlock>>>(
          *directory,
          [&directory](std::string_view filename)
              -> std::optional<zx::result<fidl::ClientEnd<skipblock::SkipBlock>>> {
            zx::result client = component::ConnectAt<skipblock::SkipBlock>(*directory, filename);
            if (client.is_error()) {
              return client.take_error();
            }
            fidl::WireSyncClient skip_block(std::move(client.value()));
            const fidl::WireResult result = skip_block->GetPartitionInfo();
            if (zx_status_t status = result.ok() ? result.value().status : result.status();
                status != ZX_OK) {
              return zx::error(status);
            }
            const auto& response = result.value();
            const uint8_t type[] = GUID_SYS_CONFIG_VALUE;
            if (memcmp(response.partition_info.partition_guid.data(), type,
                       skipblock::wire::kGuidLen) != 0) {
              return {};
            }
            return zx::ok(skip_block.TakeClientEnd());
          });
  if (watch_result.is_error()) {
    return watch_result.take_error();
  }
  return std::move(watch_result.value());
}

sysconfig_subpartition GetSubpartitionInfo(const sysconfig_header& header,
                                           SyncClient::PartitionType partition) {
  switch (partition) {
    case SyncClient::PartitionType::kSysconfig:
      return header.sysconfig_data;
    case SyncClient::PartitionType::kABRMetadata:
      return header.abr_metadata;
    case SyncClient::PartitionType::kVerifiedBootMetadataA:
      return header.vb_metadata_a;
    case SyncClient::PartitionType::kVerifiedBootMetadataB:
      return header.vb_metadata_b;
    case SyncClient::PartitionType::kVerifiedBootMetadataR:
      return header.vb_metadata_r;
  }
  ZX_ASSERT(false);  // Unreachable.
}

struct PartitionTypeAndInfo {
  SyncClient::PartitionType id;
  uint64_t offset;
  uint64_t size;

  PartitionTypeAndInfo() = default;

  // Why not use a sysconfig_subpartition member?
  // Because sysconfig_subpartition is declared with "packed" attribute. This has been seen
  // to cause alignment issue on some platforms if used directly as member.
  PartitionTypeAndInfo(SyncClient::PartitionType part, sysconfig_subpartition info)
      : id(part), offset(info.offset), size(info.size) {}
};

using PartitionTypeAndInfoArray = std::array<PartitionTypeAndInfo, 5>;

PartitionTypeAndInfoArray SortSubpartitions(const sysconfig_header& header) {
  using PartitionType = SyncClient::PartitionType;
  PartitionTypeAndInfoArray ret = {{
      PartitionTypeAndInfo(PartitionType::kSysconfig, header.sysconfig_data),
      PartitionTypeAndInfo(PartitionType::kABRMetadata, header.abr_metadata),
      PartitionTypeAndInfo(PartitionType::kVerifiedBootMetadataA, header.vb_metadata_a),
      PartitionTypeAndInfo(PartitionType::kVerifiedBootMetadataB, header.vb_metadata_b),
      PartitionTypeAndInfo(PartitionType::kVerifiedBootMetadataR, header.vb_metadata_r),
  }};
  std::sort(ret.begin(), ret.end(),
            [](const auto& l, const auto& r) { return l.offset < r.offset; });
  return ret;
}

// Rotate <mem> to the left by <rotate> unit
void RotateMemoryLeft(uint8_t* mem, size_t len, size_t rotate) {
  /**
   * Example: rotate "123456789" 3 units to the left, into "456789123"
   *
   * Step 1: reverse the entire array -> "987654321"
   * Step 2: reverse the first (9 - 3) units -> "456789321"
   * Step 3: reverse the last 3 units -> "456789123"
   */
  std::reverse(mem, mem + len);
  std::reverse(mem, mem + len - rotate);
  std::reverse(mem + len - rotate, mem + len);
}

struct PartitionTransformInfo {
  PartitionTypeAndInfoArray current;
  PartitionTypeAndInfoArray target_sorted;

  PartitionTransformInfo(const sysconfig_header& current_header,
                         const sysconfig_header& target_header) {
    current = SortSubpartitions(current_header);
    target_sorted = SortSubpartitions(target_header);
  }

  PartitionTypeAndInfo GetCurrentInfo(SyncClient::PartitionType id) {
    auto res = std::find_if(current.begin(), current.end(),
                            [id](const auto& ele) { return ele.id == id; });
    ZX_ASSERT(res != current.end());
    return *res;
  }

  PartitionTypeAndInfo GetTargetInfo(SyncClient::PartitionType id) {
    auto res = std::find_if(target_sorted.begin(), target_sorted.end(),
                            [id](const auto& ele) { return ele.id == id; });
    ZX_ASSERT(res != target_sorted.end());
    return *res;
  }
};

// Update sysconfig layout according to a new header in place.
void UpdateSysconfigLayout(void* start, size_t len, const sysconfig_header& current_header,
                           const sysconfig_header& target_header) {
  /**
   * Example:
   * Existing layout: AAAAAABCCCCCCCC
   * Target layout:   XAACCCCCCCCBBBB ('X' means not belonging to any sub-partition)
   *
   * Step 1: Shrink partition A -> "AAXXXXBCCCCCCCC"
   * Step 2: Pack all sub-partitions to the right -> "XXXXAABCCCCCCCC"
   * Step 3: Reorder sub-partitions in packed region -> "XXXXAACCCCCCCCB"
   * Step 4: Move all sub-partitions to target offset in their order. Size of B increases
   * naturally. -> "XAACCCCCCCCBXXX"
   *
   * The average run-time of the algorithm is approximately 2ms in release build.
   */

  auto mem = static_cast<uint8_t*>(start);
  PartitionTransformInfo info(current_header, target_header);

  // Step 1: Shrink sub-partitions sizes to target.
  for (auto& current : info.current) {
    current.size = std::min(current.size, info.GetTargetInfo(current.id).size);
  }

  // Step 2: Pack all sub-partitions to the right.
  size_t move_right_copy_index = len;
  for (auto iter = info.current.rbegin(); iter != info.current.rend(); iter++) {
    move_right_copy_index -= iter->size;
    memmove(&mem[move_right_copy_index], &mem[iter->offset], iter->size);
    iter->offset = move_right_copy_index;
  }

  // Step 3: Order sub-partitions according to the target header, by rotating the memory.
  size_t seg_offset = move_right_copy_index;
  for (auto& target : info.target_sorted) {
    PartitionTypeAndInfo target_part_in_current = info.GetCurrentInfo(target.id);
    // Rotate this sub-partition to the first position in the segment.
    uint8_t* seg_start = &mem[seg_offset];
    size_t seg_len = len - seg_offset;
    size_t rotate = target_part_in_current.offset - seg_offset;
    if (rotate != 0) {
      RotateMemoryLeft(seg_start, seg_len, rotate);
      // Update offset after rotation.
      for (auto& current : info.current) {
        if (current.offset >= seg_offset) {
          current.offset = (current.offset - seg_offset + seg_len - rotate) % seg_len + seg_offset;
        }
      }
    }
    seg_offset += target_part_in_current.size;
  }

  // Step 4: Move sub-partitions to their target offsets. Sizes increase naturally.
  size_t end_of_prev_part = 0;
  for (auto& target : info.target_sorted) {
    PartitionTypeAndInfo target_part_in_current = info.GetCurrentInfo(target.id);
    memset(&mem[end_of_prev_part], 0xff, target.offset - end_of_prev_part);
    if (target.offset != target_part_in_current.offset) {
      memmove(&mem[target.offset], &mem[target_part_in_current.offset],
              target_part_in_current.size);
    }
    // Note: We set end_or_prev_part to be target offset + old size so that
    // if target size > old size, next iteration will set the expanded memory to 0xff.
    end_of_prev_part = target.offset + target_part_in_current.size;
  }
  // Set the remaining part after all sub-partitions of 0xff, if there is any.
  memset(&mem[end_of_prev_part], 0xff, len - end_of_prev_part);
}

}  // namespace

zx::result<SyncClient> SyncClient::Create(fidl::UnownedClientEnd<fuchsia_io::Directory> dev) {
  zx::result skip_block = FindSysconfigPartition(dev);
  if (skip_block.is_error()) {
    return skip_block.take_error();
  }
  return zx::ok(SyncClient(fidl::WireSyncClient(std::move(skip_block.value()))));
}

SyncClient SyncClient::Create(fidl::ClientEnd<skipblock::SkipBlock> skip_block) {
  return SyncClient(fidl::WireSyncClient(std::move(skip_block)));
}

zx::result<std::reference_wrapper<const sysconfig_header>> SyncClient::GetHeader() {
  if (!header_.has_value()) {
    if (zx_status_t status = LoadFromStorage(); status != ZX_OK) {
      fprintf(stderr, "ReadHeader: Failed to read header from storage. %s.\n",
              zx_status_get_string(status));
      return zx::error(status);
    }

    sysconfig_header& header = header_.emplace();
    memcpy(&header, read_mapper_.start(), sizeof(sysconfig_header));
    if (!sysconfig_header_valid(&header, kAstroPageSize, kAstroSysconfigPartitionSize)) {
      fprintf(stderr, "GetHeader: Falling back to default header.\n");
      memcpy(&header, &kLegacyLayout, sizeof(sysconfig_header));
    }
  }
  return zx::ok(std::reference_wrapper(header_.value()));
}

zx_status_t SyncClient::WritePartition(PartitionType partition, const zx::vmo& vmo,
                                       zx_off_t vmo_offset) {
  zx::result header = GetHeader();
  if (header.is_error()) {
    // In case there is actually a valid header in the storage, but just that we fail to read due to
    // transient error, refuse to perform any write to avoid compromising the partition, bootloader
    // etc.
    fprintf(stderr,
            "WritePartition: error while reading for header. Refuse to perform any write. %s\n",
            header.status_string());
    return header.error_value();
  }

  auto partition_info = GetSubpartitionInfo(*header, partition);
  return Write(partition_info.offset, partition_info.size, vmo, vmo_offset);
}

zx_status_t SyncClient::Write(size_t offset, size_t len, const zx::vmo& vmo, zx_off_t vmo_offset,
                              skipblock::wire::WriteBytesMode mode) {
  zx::vmo dup;
  if (zx_status_t status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); status != ZX_OK) {
    return status;
  }
  skipblock::wire::WriteBytesOperation operation = {
      .vmo = std::move(dup),
      .vmo_offset = vmo_offset,
      .offset = offset,
      .size = len,
      .mode = mode,
  };
  printf("sysconfig: ADDING ERASE CYCLE TO SYSCONFIG\n");
  auto result = skip_block_->WriteBytes(std::move(operation));
  return result.ok() ? result.value().status : result.status();
}

zx_status_t SyncClient::WriteBytesWithoutErase(size_t offset, size_t len, const zx::vmo& vmo,
                                               zx_off_t vmo_offset) {
  zx::vmo dup;
  if (zx_status_t status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); status != ZX_OK) {
    return status;
  }
  skipblock::wire::WriteBytesOperation operation = {
      .vmo = std::move(dup),
      .vmo_offset = vmo_offset,
      .offset = offset,
      .size = len,
      // The option doesn't have effect, but not setting it to a valid enum value will result
      // in fidl errors.
      .mode = skipblock::wire::WriteBytesMode::kReadModifyEraseWrite,
  };
  auto result = skip_block_->WriteBytesWithoutErase(std::move(operation));
  return result.ok() ? result.value().status : result.status();
}

zx_status_t SyncClient::InitializeReadMapper() {
  auto result = skip_block_->GetPartitionInfo();
  zx_status_t status = result.ok() ? result.value().status : result.status();
  if (status != ZX_OK) {
    return status;
  }
  const uint64_t block_size = result.value().partition_info.block_size_bytes;

  return read_mapper_.CreateAndMap(fbl::round_up(block_size, zx_system_get_page_size()),
                                   "sysconfig read");
}

zx_status_t SyncClient::ReadPartition(PartitionType partition, const zx::vmo& vmo,
                                      zx_off_t vmo_offset) {
  zx::result header = GetHeader();
  if (header.is_error()) {
    fprintf(stderr, "ReadPartition: error while reading for header. %s\n", header.status_string());
    return header.error_value();
  }

  auto partition_info = GetSubpartitionInfo(*header, partition);
  return Read(partition_info.offset, partition_info.size, vmo, vmo_offset);
}

zx_status_t SyncClient::Read(size_t offset, size_t len, const zx::vmo& vmo, zx_off_t vmo_offset) {
  if (auto status = LoadFromStorage(); status != ZX_OK) {
    fprintf(stderr, "Failed to read content from storage. %s\n", zx_status_get_string(status));
    return status;
  }
  return vmo.write(reinterpret_cast<uint8_t*>(read_mapper_.start()) + offset, vmo_offset, len);
}

zx_status_t SyncClient::LoadFromStorage() {
  // Lazily create read mapper.
  if (read_mapper_.start() == nullptr) {
    zx_status_t status = InitializeReadMapper();
    if (status != ZX_OK) {
      return status;
    }
  }

  zx::vmo dup;
  if (zx_status_t status = read_mapper_.vmo().duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
      status != ZX_OK) {
    return status;
  }
  skipblock::wire::ReadWriteOperation operation = {
      .vmo = std::move(dup),
      .vmo_offset = 0,
      .block = 0,
      .block_count = 1,
  };
  auto result = skip_block_->Read(std::move(operation));
  zx_status_t status = result.ok() ? result.value().status : result.status();
  if (status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}

zx_status_t SyncClient::GetPartitionSize(PartitionType partition, size_t* out) {
  zx::result header = GetHeader();
  if (header.is_error()) {
    fprintf(stderr, "GetPartitionSize: error while reading for header. %s\n",
            header.status_string());
    return header.error_value();
  }
  *out = GetSubpartitionInfo(*header, partition).size;
  return ZX_OK;
}

zx_status_t SyncClient::GetPartitionOffset(PartitionType partition, size_t* out) {
  zx::result header = GetHeader();
  if (header.is_error()) {
    fprintf(stderr, "GetPartitionOffset: error while reading for header. %s\n",
            header.status_string());
    return header.error_value();
  }
  *out = GetSubpartitionInfo(*header, partition).offset;
  return ZX_OK;
}

zx_status_t SyncClient::UpdateLayout(sysconfig_header target_header) {
  zx::result result = GetHeader();
  if (result.is_error()) {
    fprintf(stderr, "UpdateLayout: Failed to read current result. %s\n", result.status_string());
    return result.error_value();
  }
  const sysconfig_header& header = result.value();

  if (sysconfig_header_equal(&target_header, &header)) {
    fprintf(stderr,
            "UpdateLayout: Already organized according to the specified layout. Skipping.\n");
    return ZX_OK;
  }

  update_sysconfig_header_magic_and_crc(&target_header);

  // Refuse to update to an invalid header in the first place
  if (!sysconfig_header_valid(&target_header, kAstroPageSize, kAstroSysconfigPartitionSize)) {
    fprintf(stderr, "UpdateLayout: Header is invalid. Refuse to update\n");
    return ZX_ERR_INVALID_ARGS;
  }

  // Read the entire partition in to read_mapper_
  if (zx_status_t status = LoadFromStorage(); status != ZX_OK) {
    fprintf(stderr, "UpdateLayout: Failed to load from storage. %s\n",
            zx_status_get_string(status));
    return status;
  }

  UpdateSysconfigLayout(read_mapper_.start(), read_mapper_.size(), header, target_header);

  // Write the header, if it is not the legacy one.
  if (!sysconfig_header_equal(&target_header, &kLegacyLayout)) {
    if (zx_status_t status = read_mapper_.vmo().write(&target_header, 0, sizeof(target_header));
        status != ZX_OK) {
      fprintf(stderr, "failed to write header to vmo. %s\n", zx_status_get_string(status));
      return status;
    }
  }

  if (zx_status_t status = Write(0, kAstroSysconfigPartitionSize, read_mapper_.vmo(), 0);
      status != ZX_OK) {
    fprintf(stderr, "UpdateLayout: failed to write to storage. %s\n", zx_status_get_string(status));
    return status;
  }

  header_ = target_header;
  return ZX_OK;
}

zx_status_t SyncClientBuffered::GetPartitionSize(PartitionType partition, size_t* size) {
  return client_.GetPartitionSize(partition, size);
}

zx_status_t SyncClientBuffered::GetPartitionOffset(PartitionType partition, size_t* size) {
  return client_.GetPartitionOffset(partition, size);
}

uint32_t SyncClientBuffered::PartitionTypeToCacheMask(PartitionType partition) {
  switch (partition) {
    case PartitionType::kSysconfig:
      return CacheBitMask::kSysconfig;
    case PartitionType::kABRMetadata:
      return CacheBitMask::kAbrMetadata;
    case PartitionType::kVerifiedBootMetadataA:
      return CacheBitMask::kVbmetaA;
    case PartitionType::kVerifiedBootMetadataB:
      return CacheBitMask::kVbmetaB;
    case PartitionType::kVerifiedBootMetadataR:
      return CacheBitMask::kVbmetaR;
  }
  ZX_ASSERT_MSG(false, "Unknown partition type %d\n", static_cast<int>(partition));
}

bool SyncClientBuffered::IsCacheEmpty(PartitionType partition) {
  return (cache_modified_flag_ & PartitionTypeToCacheMask(partition)) == 0;
}

void SyncClientBuffered::MarkCacheNonEmpty(PartitionType partition) {
  cache_modified_flag_ |= PartitionTypeToCacheMask(partition);
}

zx_status_t SyncClientBuffered::CreateCache() {
  if (cache_.vmo()) {
    return ZX_OK;
  }

  if (auto status = cache_.CreateAndMap(kAstroSysconfigPartitionSize, "sysconfig cache", kVmoRw);
      status != ZX_OK) {
    fprintf(stderr, "failed to create cache. %s\n", zx_status_get_string(status));
    return status;
  }

  if (auto status = client_.Read(0, kAstroSysconfigPartitionSize, cache_.vmo(), 0);
      status != ZX_OK) {
    fprintf(stderr, "failed to initialize cache content. %s\n", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

void SyncClientBuffered::InvalidateCache() {
  cache_modified_flag_ = 0;
  cache_.Reset();
}

bool SyncClientBuffered::IsAllCacheEmpty() { return cache_modified_flag_ == 0; }

zx_status_t SyncClientBuffered::WritePartition(PartitionType partition, const zx::vmo& vmo,
                                               zx_off_t vmo_offset) {
  return WriteCache(partition, vmo, vmo_offset);
}

zx_status_t SyncClientBuffered::WriteCache(PartitionType partition, const zx::vmo& vmo,
                                           zx_off_t vmo_offset) {
  if (auto status = CreateCache(); status != ZX_OK) {
    return status;
  }

  size_t size;
  uint8_t* start;
  if (auto status = GetSubpartitionCacheAddrSize(partition, &start, &size); status != ZX_OK) {
    return status;
  }

  if (auto status = vmo.read(start, vmo_offset, size); status != ZX_OK) {
    fprintf(stderr, "WriteCache: Failed to write to cache. %s\n", zx_status_get_string(status));
    return status;
  }
  MarkCacheNonEmpty(partition);

  return ZX_OK;
}

zx_status_t SyncClientBuffered::ReadPartition(PartitionType partition, const zx::vmo& vmo,
                                              zx_off_t vmo_offset) {
  return IsCacheEmpty(partition) ? client_.ReadPartition(partition, vmo, vmo_offset)
                                 : ReadCache(partition, vmo, vmo_offset);
}

zx_status_t SyncClientBuffered::ReadCache(PartitionType partition, const zx::vmo& vmo,
                                          zx_off_t vmo_offset) {
  size_t size;
  uint8_t* start;
  if (auto status = GetSubpartitionCacheAddrSize(partition, &start, &size); status != ZX_OK) {
    return status;
  }

  if (auto status = vmo.write(start, vmo_offset, size); status != ZX_OK) {
    fprintf(stderr, "ReadCache::Failed to read from cached %d, %s\n", static_cast<int>(partition),
            zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

zx_status_t SyncClientBuffered::Flush() {
  if (IsAllCacheEmpty()) {
    return ZX_OK;
  }

  if (auto status = client_.Write(0, kAstroSysconfigPartitionSize, cache_.vmo(), 0);
      status != ZX_OK) {
    fprintf(stderr, "Failed to flush write. %s\n", zx_status_get_string(status));
    return status;
  }

  InvalidateCache();
  return ZX_OK;
}

zx_status_t SyncClientBuffered::GetSubpartitionCacheAddrSize(PartitionType partition,
                                                             uint8_t** start, size_t* size) {
  zx::result header = client_.GetHeader();
  if (header.is_error()) {
    return header.error_value();
  }
  auto info = GetSubpartitionInfo(header.value(), partition);
  *start = static_cast<uint8_t*>(cache_.start()) + info.offset;
  *size = info.size;
  return ZX_OK;
}

const uint8_t* SyncClientBuffered::GetCacheBuffer(PartitionType partition) {
  size_t size;
  uint8_t* start;
  if (auto status = GetSubpartitionCacheAddrSize(partition, &start, &size); status != ZX_OK) {
    return nullptr;
  }
  return start;
}

zx_status_t SyncClientBuffered::UpdateLayout(sysconfig_header target_header) {
  if (auto status = Flush(); status != ZX_OK) {
    return status;
  }

  return client_.UpdateLayout(target_header);
}

// One example layout that supports abr wear-leveling.
// Comparing with legacy layout, abr metadata is put at the end, extended to 10 pages
// sysconfig_data shrunken to 5 pages.
// The first page is reserved for header.
static constexpr sysconfig_header kLayoutForWearLeveling = {
    .magic = SYSCONFIG_HEADER_MAGIC_ARRAY,
    .sysconfig_data = {4 * kKilobyte, 20 * kKilobyte},
    .abr_metadata = {216 * kKilobyte, 40 * kKilobyte},
    .vb_metadata_a = {24 * kKilobyte, kVerifiedBootMetadataSize},
    .vb_metadata_b = {88 * kKilobyte, kVerifiedBootMetadataSize},
    .vb_metadata_r = {152 * kKilobyte, kVerifiedBootMetadataSize},
    .crc_value = 0x16713db5,
};

const sysconfig_header& SyncClientAbrWearLeveling::GetAbrWearLevelingSupportedLayout() {
  return kLayoutForWearLeveling;
}

zx_status_t SyncClientAbrWearLeveling::ReadPartition(PartitionType partition, const zx::vmo& vmo,
                                                     zx_off_t vmo_offset) {
  return IsCacheEmpty(partition) && partition == PartitionType::kABRMetadata
             ? ReadLatestAbrMetadataFromStorage(vmo, vmo_offset)
             : SyncClientBuffered::ReadPartition(partition, vmo, vmo_offset);
}

bool SyncClientAbrWearLeveling::IsOnlyAbrMetadataModified() {
  return !IsCacheEmpty(PartitionType::kABRMetadata) && IsCacheEmpty(PartitionType::kSysconfig) &&
         IsCacheEmpty(PartitionType::kVerifiedBootMetadataA) &&
         IsCacheEmpty(PartitionType::kVerifiedBootMetadataB) &&
         IsCacheEmpty(PartitionType::kVerifiedBootMetadataR);
}

zx_status_t SyncClientAbrWearLeveling::ReadLatestAbrMetadataFromStorage(const zx::vmo& vmo,
                                                                        zx_off_t vmo_offset) {
  abr_metadata_ext latest;
  if (auto status = FindLatestAbrMetadataFromStorage(&latest); status != ZX_OK) {
    return status;
  }
  return vmo.write(&latest, vmo_offset, sizeof(abr_metadata_ext));
}

zx_status_t SyncClientAbrWearLeveling::FindLatestAbrMetadataFromStorage(abr_metadata_ext* out) {
  zx::result result = client_.GetHeader();
  if (result.is_error()) {
    return result.error_value();
  }
  const sysconfig_header& header = result.value();

  if (auto status = client_.LoadFromStorage(); status != ZX_OK) {
    return status;
  }
  auto abr_start = static_cast<uint8_t*>(client_.read_mapper_.start()) + header.abr_metadata.offset;

  ZX_ASSERT(out != nullptr);
  if (layout_support_wear_leveling(&header, kAstroPageSize)) {
    find_latest_abr_metadata_page(&header, abr_start, kAstroPageSize, out);
  } else {
    memcpy(out, abr_start, sizeof(abr_metadata_ext));
  }

  return ZX_OK;
}

zx_status_t SyncClientAbrWearLeveling::Flush() {
  if (IsAllCacheEmpty()) {
    return ZX_OK;
  }

  zx::result result = client_.GetHeader();
  if (result.is_error()) {
    return result.error_value();
  }
  const sysconfig_header& header = result.value();

  if (!layout_support_wear_leveling(&header, kAstroPageSize)) {
    return SyncClientBuffered::Flush();
  }

  // Try if we can only perform an abr metadata append.
  if (FlushAppendAbrMetadata(&header) != ZX_OK) {
    // If appending is not applicable, flush all valid cached data to memory.
    // An erase will be introduced.
    if (auto status = FlushReset(&header); status != ZX_OK) {
      return status;
    }
  }

  InvalidateCache();
  return ZX_OK;
}

zx_status_t SyncClientAbrWearLeveling::ValidateAbrMetadataInStorage(
    const abr_metadata_ext* expected) {
  abr_metadata_ext latest;
  if (auto status = FindLatestAbrMetadataFromStorage(&latest); status != ZX_OK) {
    fprintf(stderr, "Failed while reading abr data from storage for validation.\n");
    return status;
  }
  return memcmp(&latest, expected, sizeof(abr_metadata_ext)) ? ZX_ERR_IO_DATA_INTEGRITY : ZX_OK;
}

zx_status_t SyncClientAbrWearLeveling::FlushAppendAbrMetadata(const sysconfig_header* header) {
  if (!IsOnlyAbrMetadataModified()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (auto status = client_.LoadFromStorage(); status != ZX_OK) {
    return status;
  }
  auto& mapper = client_.read_mapper_;
  auto abr_start = static_cast<const uint8_t*>(mapper.start()) + header->abr_metadata.offset;

  // Find an empty page to write
  uint64_t page_write_index;
  if (!find_empty_page_for_wear_leveling(header, abr_start, kAstroPageSize, &page_write_index)) {
    return ZX_ERR_INTERNAL;
  }

  // Read the abr metadata from cache, update magic and write back to cache.
  // Note: Although cache has the same layout as sysconfig partition, writing abr metadata to
  // cache does not use abr wear-leveling. The new data is just written to the start of the abr
  // metadata sub-partition in cache.
  abr_metadata_ext abr_data;
  uint8_t* cache_abr_start = static_cast<uint8_t*>(cache_.start()) + header->abr_metadata.offset;
  memcpy(&abr_data, cache_abr_start, sizeof(abr_metadata_ext));
  set_abr_metadata_ext_magic(&abr_data);
  memcpy(cache_abr_start, &abr_data, sizeof(abr_metadata_ext));

  // Perform a write without erase.
  size_t offset =
      header->abr_metadata.offset + static_cast<size_t>(page_write_index) * kAstroPageSize;
  if (auto status = client_.WriteBytesWithoutErase(offset, kAstroPageSize, cache_.vmo(),
                                                   header->abr_metadata.offset);
      status != ZX_OK) {
    fprintf(stderr, "Failed to append abr metadata to persistent storage. %s\n",
            zx_status_get_string(status));
    return status;
  }

  // find_empty_page_for_wear_leveling() check page emptiness by verifying that all bytes are
  // 0xff. However this may not always be safe as a page that has been deliberately written all 0xff
  // may not be available to write. Thus to be safe, here we double-check that the data can be read
  // back correctly. Returns error if validation fails. This will lead Flush() to fall back to
  // FlushReset().
  if (auto status = ValidateAbrMetadataInStorage(&abr_data); status != ZX_OK) {
    fprintf(stderr, "Validation failed for abr metadata ext write. %s\n",
            zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

zx_status_t SyncClientAbrWearLeveling::FlushReset(const sysconfig_header* header) {
  // 1. Write the abr data to the first page of in the sub-partition
  // 2. Set the rest pages of the abr sub-partition to empty (write 0xff).

  auto abr_start = static_cast<uint8_t*>(cache_.start()) + header->abr_metadata.offset;
  abr_metadata_ext abr_data;
  // Find the latest abr meta, either from the cache if it is modified, or from the storage.
  if (IsCacheEmpty(PartitionType::kABRMetadata)) {
    find_latest_abr_metadata_page(header, abr_start, kAstroPageSize, &abr_data);
  } else {
    memcpy(&abr_data, abr_start, sizeof(abr_metadata_ext));
  }
  // Set magic.
  set_abr_metadata_ext_magic(&abr_data);
  // Put the latest abr data to the first page.
  memcpy(abr_start, &abr_data, sizeof(abr_metadata_ext));

  // Write data to persistent storage.
  size_t write_size = kAstroSysconfigPartitionSize - (header->abr_metadata.size - kAstroPageSize);
  if (auto status = client_.Write(0, write_size, cache_.vmo(), 0,
                                  skipblock::wire::WriteBytesMode::kEraseWrite);
      status != ZX_OK) {
    fprintf(stderr, "Failed to flush write. %s\n", zx_status_get_string(status));
    return status;
  }

  erase_count_++;
  return ZX_OK;
}

}  // namespace sysconfig

uint32_t sysconfig_header_crc32(uint32_t crc, const uint8_t* buf, size_t len) {
  return crc32(crc, buf, len);
}
