// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/volume_image/fvm/fvm_unpack.h"

#include <fcntl.h>
#include <lib/fpromise/result.h>
#include <sys/types.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

#include <fbl/unique_fd.h>

#include "src/storage/fvm/format.h"
#include "src/storage/fvm/metadata.h"
#include "src/storage/volume_image/fvm/fvm_metadata.h"
#include "src/storage/volume_image/utils/fd_writer.h"
#include "src/storage/volume_image/utils/reader.h"
#include "src/storage/volume_image/utils/writer.h"

namespace storage::volume_image {

namespace {
// A simple wrapper to hold onto the copying buffer while copying slices from the fvm image
// to their new blk files.
class SliceDistributor {
 public:
  SliceDistributor(const Reader& reader, const fvm::Metadata& metadata)
      : reader_(reader), buffer_(metadata.GetHeader().slice_size), metadata_(metadata) {}

  fpromise::result<void, std::string> WriteSlice(uint64_t pslice, Writer* writer, uint64_t vslice) {
    if (auto result = reader_.Read(metadata_.GetHeader().GetSliceDataOffset(pslice), buffer_);
        result.is_error()) {
      return result.take_error_result();
    }
    if (auto result = writer->Write(vslice * metadata_.GetHeader().slice_size, buffer_);
        result.is_error()) {
      return result.take_error_result();
    }
    return fpromise::ok();
  }

 private:
  const Reader& reader_;
  std::vector<uint8_t> buffer_;
  const fvm::Metadata& metadata_;
};
}  // namespace

namespace internal {

fpromise::result<void, std::string> UnpackRawFvmPartitions(
    const Reader& image, const fvm::Metadata& metadata,
    const std::vector<std::unique_ptr<Writer>>& out_files) {
  SliceDistributor distributor(image, metadata);
  for (uint64_t pslice = 0; pslice <= metadata.GetHeader().GetAllocationTableUsedEntryCount();
       ++pslice) {
    const fvm::SliceEntry& slice = metadata.GetSliceEntry(pslice);
    if (!slice.IsAllocated()) {
      continue;
    }
    const uint64_t partition = slice.VPartition();
    const uint64_t vslice = slice.VSlice();
    // Skip partitions that we didn't ask to write out.
    if (partition >= out_files.size() || !out_files[partition]) {
      continue;
    }
    if (auto result = distributor.WriteSlice(pslice, out_files[partition].get(), vslice);
        result.is_error()) {
      return fpromise::error("Failed to copy slice " + std::to_string(pslice) + " to vslice " +
                             std::to_string(vslice) + " of partition id " +
                             std::to_string(partition) + ": " + result.take_error());
    }
  }

  return fpromise::ok();
}

std::vector<std::optional<std::string>> DisambiguateNames(
    const std::vector<std::optional<std::string>>& names) {
  std::vector<std::optional<std::string>> deduped(names.size());
  std::unordered_map<std::string, size_t> counts;
  for (size_t i = 0; i < names.size(); ++i) {
    if (!names[i].has_value()) {
      continue;
    }
    std::string current = names[i].value();
    // Disallow path separators to keep files local, and dashes since they'll be used to
    // differentiate duplicates.
    const std::array bad_chars{'-', '/', '\\'};
    std::ranges::replace_if(
        current, [&bad_chars](char c) { return std::ranges::contains(bad_chars, c); }, '_');
    if (current == "." || current == "..") {
      current = std::string(current.size(), '_');
    }
    size_t dupe_number = 0;
    auto entry = counts.find(current);
    if (entry == counts.end()) {
      counts[current] = 0;
    } else {
      dupe_number = entry->second + 1;
      counts[current] = dupe_number;
    }
    if (dupe_number > 0 || current.empty()) {
      deduped[i] = std::move(current) + "-" + std::to_string(dupe_number);
    } else {
      deduped[i] = std::move(current);
    }
  }
  return deduped;
}

}  // namespace internal

fpromise::result<void, std::string> UnpackRawFvm(const Reader& image,
                                                 const std::string& out_path_prefix) {
  auto metadata_or = FvmGetMetadata(image);
  if (metadata_or.is_error()) {
    return metadata_or.take_error_result();
  }
  const auto& metadata = metadata_or.take_value();

  // Verify that the file is big enough before proceeding. The reads use pread which will silently
  // read past the end of the file.
  if (metadata.GetHeader().fvm_partition_size != image.length()) {
    std::cerr << "Warning: The image file is " << image.length() << " bytes. Expected "
              << metadata.GetHeader().fvm_partition_size << " bytes." << std::endl;
  }

  // Find all used partitions
  size_t num_partitions = metadata.GetHeader().GetPartitionTableEntryCount();
  std::vector<std::optional<std::string>> names(num_partitions + 1);
  for (uint64_t i = 1; i <= num_partitions; ++i) {
    const fvm::VPartitionEntry& partition = metadata.GetPartitionEntry(i);
    if (partition.IsFree()) {
      continue;
    }
    std::cout << "Partition \"" << partition.name() << "\" has reserved " << partition.slices
              << " slices for " << partition.slices * metadata.GetHeader().slice_size << " bytes."
              << std::endl;
    names[i] = partition.name();
  }

  // Open an output file for each partition
  std::vector<std::optional<std::string>> out_names = internal::DisambiguateNames(names);
  std::vector<std::unique_ptr<Writer>> writers(num_partitions + 1);
  for (size_t i = 0; i < out_names.size(); ++i) {
    if (!out_names[i].has_value()) {
      continue;
    }
    std::string path = out_path_prefix + out_names[i].value();
    fbl::unique_fd fd(open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, S_IRUSR | S_IWUSR));
    if (!fd.is_valid()) {
      return fpromise::error("Failed to open '" + path + "' for writing");
    }
    writers[i] = std::make_unique<FdWriter>(std::move(fd));
  }

  return internal::UnpackRawFvmPartitions(image, metadata, writers);
}

}  // namespace storage::volume_image
