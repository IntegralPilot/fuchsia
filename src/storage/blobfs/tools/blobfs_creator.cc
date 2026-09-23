// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/tools/blobfs_creator.h"

#include <fcntl.h>
#include <inttypes.h>
#include <lib/zx/result.h>
#include <string.h>
#include <sys/types.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <thread>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <fbl/unique_fd.h>

#include "src/lib/chunked-compression/multithreaded-chunked-compressor.h"
#include "src/lib/digest/digest.h"
#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/fsck_host.h"
#include "src/storage/blobfs/host.h"
#include "src/storage/blobfs/iterator/node_populator.h"
#include "src/storage/lib/host/common.h"

namespace {

constexpr uint32_t kDefaultConcurrency = 4;

void WriteBlobInfoToJson(std::ofstream& file, const blobfs::BlobInfo& blob) {
  std::filesystem::path path =
      std::filesystem::relative(std::filesystem::canonical(blob.GetSrcFilePath()));
  const auto& blob_layout = blob.GetBlobLayout();
  uint64_t total_size = blob_layout.TotalBlockCount() * blobfs::kBlobfsBlockSize;
  file << "  {\n";
  file << "    \"source_path\": " << path << ",\n";
  file << "    \"merkle\": \"" << blob.GetDigest().ToString() << "\",\n";
  file << "    \"bytes\": " << blob_layout.FileSize() << ",\n";
  file << "    \"size\": " << total_size << ",\n";
  file << "    \"file_size\": " << blob_layout.FileSize() << ",\n";
  file << "    \"compressed_file_size\": " << blob_layout.DataSizeUpperBound() << ",\n";
  file << "    \"merkle_tree_size\": " << blob_layout.MerkleTreeSize() << ",\n";
  file << "    \"used_space_in_blobfs\": " << total_size << "\n";
  file << "  }";
}

zx::result<> RecordBlobs(const std::filesystem::path& path,
                         std::map<digest::Digest, blobfs::BlobInfo>& blobs) {
  std::ofstream file(path);
  if (!file.is_open()) {
    fprintf(stderr, "Failed to open: %s\n", path.c_str());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  file << "[\n";
  bool is_first_blob = true;
  for (const auto& [digest, blob] : blobs) {
    if (is_first_blob) {
      is_first_blob = false;
    } else {
      file << ",\n";
    }
    WriteBlobInfoToJson(file, blob);
  }
  file << "]\n";
  file.close();
  if (file.fail()) {
    fprintf(stderr, "Writing to %s failed\n", path.c_str());
    return zx::error(ZX_ERR_IO);
  }
  return zx::ok();
}

zx::result<> CreateBlobfsWithBlobs(fbl::unique_fd fd,
                                   const std::map<digest::Digest, blobfs::BlobInfo>& blobs) {
  std::unique_ptr<blobfs::Blobfs> blobfs;
  if (zx_status_t status = blobfs_create(&blobfs, std::move(fd)); status != ZX_OK) {
    return zx::error(status);
  }
  for (const auto& [digest, blob] : blobs) {
    if (zx::result status = blobfs->AddBlob(blob); status.is_error()) {
      fprintf(stderr, "Failed to add blob '%s': %d\n", blob.GetSrcFilePath().c_str(),
              status.status_value());
      return status;
    }
  }
  return zx::ok();
}

}  // namespace

zx_status_t BlobfsCreator::Usage() {
  zx_status_t status = FsCreator::Usage();

  fprintf(stderr, "\nblobfs specific options:\n");
  fprintf(stderr,
          "\t--deprecated_padded_format\tFormat blobfs using the deprecated format that uses more "
          "space.\n"
          "Valid for the commands: mkfs and create.\n");
  // Additional information about manifest format.
  fprintf(stderr, "\nEach manifest line must adhere to one of the following formats:\n");
  fprintf(stderr, "\t'dst/path=src/path'\n");
  fprintf(stderr, "\t'dst/path'\n");
  fprintf(stderr, "with one dst/src pair or single dst per line.\n");

  fprintf(stderr, "\nblobfs specific commands:\n");
  fprintf(stderr, "\texport [IMAGE] [PATH]\n");
  fprintf(stderr,
          "\nExports each blob in IMAGE to the directory in PATH. If PATH does not exist, will "
          "attempt to "
          "create it.\n");
  fprintf(stderr,
          "\nEach blob exported to PATH is named after their merkle root, and the contents match "
          "what IMAGE has.\n");
  return status;
}

bool BlobfsCreator::IsCommandValid(Command command) {
  switch (command) {
    case Command::kMkfs:
    case Command::kFsck:
    case Command::kUsedDataSize:
    case Command::kUsedInodes:
    case Command::kUsedSize:
    case Command::kAdd:
      return true;
    default:
      return false;
  }
}

bool BlobfsCreator::IsOptionValid(Option option) {
  // TODO(planders): Add offset and length support to blobfs.
  switch (option) {
    case Option::kDepfile:
    case Option::kReadonly:
    case Option::kCompress:
    case Option::kJsonOutput:
    case Option::kHelp:
      return true;
    default:
      return false;
  }
}

bool BlobfsCreator::IsArgumentValid(Argument argument) {
  switch (argument) {
    case Argument::kManifest:
    case Argument::kBlob:
      return true;
    default:
      return false;
  }
}

zx_status_t BlobfsCreator::ProcessManifestLine(FILE* manifest, const char* dir_path) {
  char src[PATH_MAX];
  src[0] = '\0';
  char dst[PATH_MAX];
  dst[0] = '\0';

  zx_status_t status;
  if ((status = ParseManifestLine(manifest, dir_path, src, dst)) != ZX_OK) {
    return status;
  }

  if (!strlen(src)) {
    fprintf(stderr, "Manifest line must specify source file\n");
    return ZX_ERR_INVALID_ARGS;
  }

  blob_list_.push_back(src);
  return ZX_OK;
}

zx_status_t BlobfsCreator::ProcessCustom(int argc, char** argv, uint8_t* processed) {
  if (strcmp(argv[0], "--blob") == 0) {
    constexpr uint8_t required_args = 2;
    if (argc < required_args) {
      fprintf(stderr, "Not enough arguments for %s\n", argv[0]);
      return ZX_ERR_INVALID_ARGS;
    }
    blob_list_.push_back(argv[1]);
    *processed = required_args;
    return ZX_OK;
  }

  if (strcmp(argv[0], "--deprecated_padded_format") == 0) {
    if (GetCommand() != Command::kMkfs) {
      fprintf(stderr, "%s is only valid for mkfs and create\n", argv[0]);
      return ZX_ERR_INVALID_ARGS;
    }
    blob_layout_format_ = blobfs::BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart;
    *processed = 1;
    return ZX_OK;
  }

  fprintf(stderr, "Argument not found: %s\n", argv[0]);
  return ZX_ERR_INVALID_ARGS;
}

zx::result<blobfs::BlobInfo> BlobfsCreator::ProcessBlobToBlobInfo(
    const std::filesystem::path& path,
    std::optional<chunked_compression::MultithreadedChunkedCompressor>& compressor) {
  if (zx_status_t res = AppendDepfile(path.c_str()); res != ZX_OK) {
    return zx::error(res);
  }
  fbl::unique_fd data_fd(open(path.c_str(), O_RDONLY, 0644));
  if (!data_fd.is_valid()) {
    fprintf(stderr, "Failed to open: %s\n", path.c_str());
    return zx::error(ZX_ERR_BAD_PATH);
  }
  zx::result<blobfs::BlobInfo> blob_info =
      compressor.has_value()
          ? blobfs::BlobInfo::CreateCompressed(data_fd.get(), blob_layout_format_, path,
                                               *compressor)
          : blobfs::BlobInfo::CreateUncompressed(data_fd.get(), blob_layout_format_, path);
  if (blob_info.is_error()) {
    fprintf(stderr, "Error here: %d\n", blob_info.error_value());
    return blob_info;
  }
  return blob_info;
}

zx_status_t BlobfsCreator::CalculateRequiredSize(off_t* out) {
  std::vector<std::thread> threads;
  std::atomic<uint32_t> blob_index = 0;
  uint32_t n_threads = std::thread::hardware_concurrency();
  if (!n_threads) {
    n_threads = kDefaultConcurrency;
  }
  std::optional<chunked_compression::MultithreadedChunkedCompressor> compressor =
      ShouldCompress()
          ? std::optional<chunked_compression::MultithreadedChunkedCompressor>(n_threads)
          : std::nullopt;
  // Accessing this with relaxed memory ordering across threads. It doesn't matter much if we do
  // a little more work than we should, eventual consistency is fine.
  std::atomic<zx_status_t> status = ZX_OK;
  for (uint32_t j = n_threads; j > 0; j--) {
    threads.emplace_back([&] {
      while (true) {
        {
          if (status.load(std::memory_order_relaxed) != ZX_OK) {
            return;
          }
        }

        uint32_t i = blob_index.fetch_add(1);
        if (i >= blob_list_.size()) {
          break;
        }
        zx::result<blobfs::BlobInfo> info_or = ProcessBlobToBlobInfo(blob_list_[i], compressor);
        if (info_or.is_error()) {
          status.store(info_or.status_value(), std::memory_order_relaxed);
          return;
        }
        blobfs::BlobInfo blob_info = std::move(info_or.value());

        std::lock_guard l(blob_info_lock_);
        blob_info_list_.insert_or_assign(blob_info.GetDigest(), std::move(blob_info));
      }
    });
  }

  for (auto& thread : threads) {
    thread.join();
  }

  if (zx_status_t end_status = status.load(std::memory_order_relaxed); end_status != ZX_OK) {
    return end_status;
  }

  uint64_t required_node_count = 0;
  for (const auto& [digest, blob_info] : blob_info_list_) {
    uint64_t block_count = blob_info.GetBlobLayout().TotalBlockCount();
    data_blocks_ += block_count;
    uint64_t extent_count =
        fbl::round_up(block_count, blobfs::Extent::kBlockCountMax) / blobfs::Extent::kBlockCountMax;
    ZX_ASSERT_MSG(extent_count < blobfs::kMaxExtentsPerBlob,
                  "Number of extents " PRIu64 "exceeds format limit of " PRIu64
                  " extents per blob.");
    required_node_count += blobfs::NodePopulator::NodeCountForExtents(extent_count);
  }

  required_inodes_ = std::max(blobfs::kBlobfsDefaultInodeCount, required_node_count);

  blobfs::Superblock info;
  // Initialize enough of |info| to be able to compute the number of bytes the image will occupy.
  info.inode_count = required_inodes_;
  info.data_block_count = data_blocks_;
  info.journal_block_count = blobfs::kMinimumJournalBlocks;
  *out = static_cast<off_t>(blobfs::TotalBlocks(info) * blobfs::kBlobfsBlockSize);
  return ZX_OK;
}

zx_status_t BlobfsCreator::Mkfs() {
  uint64_t block_count;
  if (blobfs::GetBlockCount(fd_.get(), &block_count)) {
    fprintf(stderr, "blobfs: cannot find end of underlying device\n");
    return ZX_ERR_IO;
  }

  int r = blobfs::Mkfs(fd_.get(), block_count,
                       {.blob_layout_format = blob_layout_format_, .num_inodes = required_inodes_});

  if (r >= 0 && !blob_list_.empty()) {
    zx_status_t status;
    if ((status = Add()) != ZX_OK) {
      return status;
    }
  }
  return r;
}

zx_status_t BlobfsCreator::Fsck() {
  zx_status_t status;
  std::unique_ptr<blobfs::Blobfs> vn;
  if ((status = blobfs::blobfs_create(&vn, std::move(fd_))) < 0) {
    return status;
  }

  return blobfs::Fsck(vn.get());
}

zx_status_t BlobfsCreator::UsedDataSize() {
  zx_status_t status;
  uint64_t size;
  if ((status = blobfs::UsedDataSize(fd_, &size)) != ZX_OK) {
    return status;
  }

  printf("%" PRIu64 "\n", size);
  return ZX_OK;
}

zx_status_t BlobfsCreator::UsedInodes() {
  zx_status_t status;
  uint64_t used_inodes;
  if ((status = blobfs::UsedInodes(fd_, &used_inodes)) != ZX_OK) {
    return status;
  }

  printf("%" PRIu64 "\n", used_inodes);
  return ZX_OK;
}

zx_status_t BlobfsCreator::UsedSize() {
  zx_status_t status;
  uint64_t size;
  if ((status = blobfs::UsedSize(fd_, &size)) != ZX_OK) {
    return status;
  }

  printf("%" PRIu64 "\n", size);
  return ZX_OK;
}

zx_status_t BlobfsCreator::Add() {
  if (blob_list_.empty()) {
    fprintf(stderr, "Adding a blob requires an additional file argument\n");
    return Usage();
  }

  if (zx::result status = CreateBlobfsWithBlobs(std::move(fd_), blob_info_list_);
      status.is_error()) {
    return status.status_value();
  }

  if (json_output_path().has_value()) {
    if (zx::result status = RecordBlobs(*json_output_path(), blob_info_list_); status.is_error()) {
      return status.status_value();
    }
  }

  return ZX_OK;
}
