// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/zx/eventpair.h>
#include <zircon/assert.h>

namespace allocation {

BufferCollectionImportExportTokens BufferCollectionImportExportTokens::New() {
  BufferCollectionImportExportTokens ref_pair;
  zx_status_t status =
      zx::eventpair::create(0, &ref_pair.export_token.value, &ref_pair.import_token.value);
  ZX_ASSERT(status == ZX_OK);
  return ref_pair;
}

fuchsia::ui::composition::BufferCollectionImportToken
BufferCollectionImportExportTokens::DuplicateImportToken() {
  fuchsia::ui::composition::BufferCollectionImportToken import_dup;
  zx_status_t status = import_token.value.duplicate(ZX_RIGHT_SAME_RIGHTS, &import_dup.value);
  ZX_ASSERT(status == ZX_OK);
  return import_dup;
}

namespace cpp {

BufferCollectionImportExportTokens BufferCollectionImportExportTokens::New() {
  BufferCollectionImportExportTokens ref_pair;
  zx_status_t status =
      zx::eventpair::create(0, &ref_pair.export_token.value(), &ref_pair.import_token.value());
  ZX_ASSERT(status == ZX_OK);
  return ref_pair;
}

fuchsia_ui_composition::BufferCollectionImportToken
BufferCollectionImportExportTokens::DuplicateImportToken() {
  fuchsia_ui_composition::BufferCollectionImportToken import_dup;
  zx_status_t status = import_token.value().duplicate(ZX_RIGHT_SAME_RIGHTS, &import_dup.value());
  ZX_ASSERT(status == ZX_OK);
  return import_dup;
}

}  // namespace cpp

}  // namespace allocation
