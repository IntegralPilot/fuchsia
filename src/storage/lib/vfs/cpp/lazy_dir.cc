// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/lazy_dir.h"

#include <dirent.h>
#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <string_view>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs {

namespace {
int CompareLazyDirPtrs(const void* a, const void* b) {
  auto a_id = static_cast<const LazyDir::LazyEntry*>(a)->id;
  auto b_id = static_cast<const LazyDir::LazyEntry*>(b)->id;
  if (a_id == b_id) {
    return 0;
  }
  return a_id < b_id ? -1 : 1;
}
}  // namespace

LazyDir::LazyDir() = default;
LazyDir::~LazyDir() = default;

fio::NodeProtocolKinds LazyDir::GetProtocols() const { return fio::NodeProtocolKinds::kDirectory; }

zx_status_t LazyDir::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out_vnode) {
  LazyEntryVector entries;
  GetContents(&entries);
  for (const auto& entry : entries) {
    if (name.compare(entry.name) == 0) {
      return GetFile(out_vnode, entry.id, entry.name);
    }
  }
  return ZX_ERR_NOT_FOUND;
}

zx_status_t LazyDir::Readdir(VdirCookie* cookie, void* dirents, size_t len, size_t* out_actual) {
  LazyEntryVector entries;
  GetContents(&entries);
  qsort(entries.data(), entries.size(), sizeof(LazyEntry), CompareLazyDirPtrs);

  fs::DirentFiller df(dirents, len);

  // The cookie's pointer is used as a bool to indicate whether "." should be output. nullptr
  // indicates that "." should be output. Any non-nullptr value indicates that "." was already
  // output.
  if (cookie->p == nullptr) {
    if (!df.Next(".", fio::DirentType::kDirectory, fio::kInoUnknown)) {
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    cookie->p = this;
  }

  for (auto it = std::lower_bound(entries.begin(), entries.end(), cookie->n,
                                  [](const LazyEntry& a, uint64_t b_id) { return a.id < b_id; });
       it < entries.end(); ++it) {
    if (cookie->n >= it->id) {
      continue;
    }
    const uint8_t d_type = IFTODT(it->type);
    if (!df.Next(it->name, fio::DirentType{d_type}, fio::kInoUnknown)) {
      *out_actual = df.BytesFilled();
      return *out_actual == 0 ? ZX_ERR_BUFFER_TOO_SMALL : ZX_OK;
    }
    cookie->n = it->id;
  }
  *out_actual = df.BytesFilled();
  return ZX_OK;
}

}  // namespace fs
