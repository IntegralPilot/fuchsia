// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_MODULE_INDEXER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_MODULE_INDEXER_H_

#include <string>

#include "src/developer/debug/zxdb/symbols/dwo_info.h"
#include "src/developer/debug/zxdb/symbols/index.h"

namespace zxdb {

class DwarfBinary;
class Index;

struct ModuleIndexerResult {
  using DwoInfoVector = std::vector<std::unique_ptr<DwoInfo>>;

  ModuleIndexerResult(std::unique_ptr<Index>&& i, DwoInfoVector&& d)
      : index(std::move(i)), dwo_info(std::move(d)) {}

  // Index is not moveable so we need to allocate it on the heap so this value can be moved.
  std::unique_ptr<Index> index;

  // Info for each .dwo reference in the index that could be loaded. This array corresponds 1:1 with
  // index.dwo_refs(). If the file could not be loaded for a DWO, the pointer will be null.
  DwoInfoVector dwo_info;
};

// Index the given DwarfBinary, loading and indexing any .dwo files it references.
//
// Note the usage of the DwarfBinary doesn't really change anything but it causes a bunch of caching
// stuff on the LLVM objects that require a non-const reference.
ModuleIndexerResult IndexModule(fxl::WeakPtr<ModuleSymbols> module_symbols, DwarfBinary& binary,
                                const std::string& build_dir);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_MODULE_INDEXER_H_
