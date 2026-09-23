// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_ALLOCATION_ALLOCATOR_H_
#define SRC_UI_SCENIC_LIB_ALLOCATION_ALLOCATOR_H_

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sys/cpp/component_context.h>

#include <memory>
#include <unordered_map>

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"

namespace allocation {

// This class implements Allocator service which allows allocation of BufferCollections which can be
// used in multiple Flatland/Gfx sessions simultaneously.
class Allocator : public fidl::Server<fuchsia_ui_composition::Allocator> {
 public:
  Allocator(async_dispatcher_t* dispatcher, sys::ComponentContext* app_context,
            const std::vector<std::shared_ptr<BufferCollectionImporter>>&
                default_buffer_collection_importers,
            const std::vector<std::shared_ptr<BufferCollectionImporter>>&
                screenshot_buffer_collection_importers,
            fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator,
            inspect::Node inspect_node = inspect::Node());
  ~Allocator() override;

  // |fuchsia_ui_composition::Allocator|
  void RegisterBufferCollection(RegisterBufferCollectionRequest& request,
                                RegisterBufferCollectionCompleter::Sync& completer) override;
  void RegisterBufferCollection(
      fuchsia_ui_composition::RegisterBufferCollectionArgs args,
      fit::function<void(fit::result<fuchsia_ui_composition::RegisterBufferCollectionError>)>
          completer);

 private:
  struct ParsedArgs {
    zx_koid_t koid;
    fuchsia_ui_composition::RegisterBufferCollectionUsages buffer_collection_usages;
    fuchsia_ui_composition::BufferCollectionExportToken export_token;
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token;
  };

  using Importers = std::vector<std::pair<BufferCollectionImporter&, BufferCollectionUsage>>;
  using BufferCollectionTokens =
      std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>;
  using Completer =
      fit::function<void(fit::result<fuchsia_ui_composition::RegisterBufferCollectionError>)>;

  // Parses the FIDL struct, validating the arguments. Logs an error and returns std::nullopt on
  // failure.
  static std::optional<ParsedArgs> ParseArgs(
      fuchsia_ui_composition::RegisterBufferCollectionArgs args);
  // Returns a list of references to all importers to be used for a buffer collection with |usages|.
  Importers GetImporters(fuchsia_ui_composition::RegisterBufferCollectionUsages usages) const;
  void DuplicateBufferCollectionToken(ParsedArgs parsed_args, Completer completer);
  // Import a BufferCollection that has already been validated via
  // `fuchsia.sysmem2/Allocator.ValidateBufferCollectionToken()`. This is necessary because Scenic
  // cannot trust clients to provide valid sysmem tokens.
  void ImportBufferCollection(ParsedArgs parsed_args, Importers importers,
                              BufferCollectionTokens tokens, Completer completer);
  void ReleaseBufferCollection(GlobalBufferCollectionId collection_id);

  // Update inspect values.
  void IncrementRegisteredBufferCollections();
  void IncrementReleasedBufferCollections();
  void IncrementFailedBufferCollections();

  // The FIDL bindings for this Allocator instance, which reference |this| as the implementation and
  // run on |dispatcher_|.
  fidl::ServerBindingGroup<fuchsia_ui_composition::Allocator> bindings_;

  // Used to import Flatland buffer collections and images to external services that Flatland does
  // not have knowledge of. Each importer is used for a different service.
  std::vector<std::shared_ptr<BufferCollectionImporter>> default_buffer_collection_importers_;

  // Used to import buffer collections for screenshot purposes.
  std::vector<std::shared_ptr<BufferCollectionImporter>> screenshot_buffer_collection_importers_;

  // A Sysmem allocator to facilitate buffer allocation with the Renderer.
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;

  // Keep track of buffer collection Ids for garbage collection.
  std::unordered_map<GlobalBufferCollectionId,
                     fuchsia_ui_composition::RegisterBufferCollectionUsages>
      buffer_collections_;

  inspect::Node inspect_node_;
  inspect::UintProperty inspect_registered_buffer_collections_;
  inspect::UintProperty inspect_released_buffer_collections_;
  inspect::UintProperty inspect_failed_buffer_collections_;
  inspect::UintProperty inspect_outstanding_buffer_collections_;

  // Should be last.
  fxl::WeakPtrFactory<Allocator> weak_factory_;

  // Dispatcher where this class runs on. Currently points to scenic main thread's dispatcher.
  async::Executor executor_;
};

}  // namespace allocation

#endif  // SRC_UI_SCENIC_LIB_ALLOCATION_ALLOCATOR_H_
