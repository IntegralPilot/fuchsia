// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "enumerator.h"

#include <lib/magma/util/short_macros.h>

#include <cstddef>

namespace {
// Copied from the docs for [`fuchsia.io/Directory.ReadDirents`].
// __PACKED added so the structure can start at any address, because
// these elements are tightly packed in the byte stream.
struct dirent {
  // Describes the inode of the entry.
  uint64_t ino;
  // Describes the length of the dirent name in bytes.
  uint8_t size;
  // Describes the type of the entry. Aligned with the
  // POSIX d_type values. Use `DirentType` constants.
  uint8_t type;
  // Unterminated name of entry.
  char name[0];
} __PACKED;

// Validate the struct has no internal padding.
constexpr int kSizeofDirentBeforeName = 10;
static_assert(offsetof(struct dirent, name) == kSizeofDirentBeforeName);

}  // namespace

namespace magma {

fit::result<magma_status_t, uint32_t> Enumerator::Enumerate(uint32_t device_path_count,
                                                            uint32_t device_path_size,
                                                            char* device_paths_out) {
  uint32_t device_count = 0;
  char* path_ptr = device_paths_out;

  std::string device_namespace = device_namespace_;
  if (!device_namespace.ends_with("/")) {
    device_namespace += "/";
  }

  while (true) {
    // Continue reading a series of dir entries.
    auto result = client_->ReadDirents(fuchsia_io::wire::kMaxBuf);
    if (!result.ok()) {
      MAGMA_DMESSAGE("ReadDirents failed: %s", result.status_string());
      return fit::error(MAGMA_STATUS_INVALID_ARGS);
    }

    auto& response = result.value();
    if (zx_status_t status = response.s; status != ZX_OK) {
      MAGMA_DMESSAGE("ReadDirents got error response: %d", status);
      return fit::error(MAGMA_STATUS_INVALID_ARGS);
    }

    fidl::VectorView<uint8_t> dirents_buffer = response.dirents;
    if (dirents_buffer.size() == 0) {
      // No more entries.
      return fit::ok(device_count);
    }

    // Iterate through the series of entries.
    for (std::size_t offset = 0;;) {
      MAGMA_DASSERT(offset <= ZX_CHANNEL_MAX_MSG_BYTES);
      MAGMA_DASSERT(dirents_buffer.size() <= ZX_CHANNEL_MAX_MSG_BYTES);
      // offset and dirents_buffer.count() can be at most ZX_CHANNEL_MAX_MSG_BYTES,
      // so we don't have to worry about overflow.
      if (offset + kSizeofDirentBeforeName > dirents_buffer.size()) {
        break;
      }

      auto entry = reinterpret_cast<const struct dirent*>(&dirents_buffer.data()[offset]);
      {
        std::size_t entry_size = kSizeofDirentBeforeName + entry->size;
        assert(offset + entry_size <= dirents_buffer.size());
        offset += entry_size;
      }

      auto entry_name = std::string(entry->name, entry->size);

      MAGMA_DMESSAGE("Enumerate got entry: %s", entry_name.c_str());

      if (entry_name == "." || entry_name == "..") {
        continue;
      }

      if (device_count >= device_path_count) {
        return fit::error(MAGMA_STATUS_MEMORY_ERROR);
      }

      auto path = device_namespace + entry_name;

      bool is_dir = entry->type == static_cast<uint8_t>(fuchsia_io::wire::DirentType::kDirectory);
      if (is_dir) {
        path += "/device";
      }

      if (path.size() >= device_path_size) {
        return fit::error(MAGMA_STATUS_INVALID_ARGS);
      }

      strcpy(path_ptr, path.c_str());

      path_ptr += device_path_size;
      device_count += 1;
    }
  }

  return fit::ok(device_count);
}

}  // namespace magma
