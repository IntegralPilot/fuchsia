// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UI_SCENIC_CPP_BUFFER_COLLECTION_IMPORT_EXPORT_TOKENS_H_
#define LIB_UI_SCENIC_CPP_BUFFER_COLLECTION_IMPORT_EXPORT_TOKENS_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>

namespace allocation {

// Convenience function which allows clients to easily create a valid |BufferCollectionExportToken|
// / |BufferCollectionImportToken| pair for use between Allocator and Flatland.
struct BufferCollectionImportExportTokens {
  static BufferCollectionImportExportTokens New();
  fuchsia::ui::composition::BufferCollectionImportToken DuplicateImportToken();

  fuchsia::ui::composition::BufferCollectionExportToken export_token;
  fuchsia::ui::composition::BufferCollectionImportToken import_token;
};

namespace cpp {

struct BufferCollectionImportExportTokens {
  static BufferCollectionImportExportTokens New();
  fuchsia_ui_composition::BufferCollectionImportToken DuplicateImportToken();

  fuchsia_ui_composition::BufferCollectionExportToken export_token;
  fuchsia_ui_composition::BufferCollectionImportToken import_token;
};

}  // namespace cpp

}  // namespace allocation

#endif  // LIB_UI_SCENIC_CPP_BUFFER_COLLECTION_IMPORT_EXPORT_TOKENS_H_
