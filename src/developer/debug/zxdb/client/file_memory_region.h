// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_FILE_MEMORY_REGION_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_FILE_MEMORY_REGION_H_

#include "src/lib/unwinder/memory.h"

namespace zxdb {

// Memory region backed by a file, e.g., .text and .rodata of a module.
class FileMemoryRegion : public unwinder::Memory {
 public:
  FileMemoryRegion(uint64_t load_address, const std::string& path)
      : load_address_(load_address), file_(fopen(path.c_str(), "rb"), fclose) {}
  unwinder::Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override;

 private:
  uint64_t load_address_;
  std::unique_ptr<FILE, decltype(&fclose)> file_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_FILE_MEMORY_REGION_H_
