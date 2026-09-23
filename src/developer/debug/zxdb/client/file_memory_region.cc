// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/file_memory_region.h"

namespace zxdb {

unwinder::Error FileMemoryRegion::ReadBytes(uint64_t addr, uint64_t size, void* dst) {
  if (addr < load_address_) {
    return unwinder::Error("out of boundary");
  }
  fseek(file_.get(), static_cast<int64_t>(addr - load_address_), SEEK_SET);
  if (fread(dst, 1, size, file_.get()) != size) {
    return unwinder::Error("short read");
  }
  return unwinder::Success();
}

}  // namespace zxdb
