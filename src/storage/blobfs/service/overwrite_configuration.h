// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_SERVICE_OVERWRITE_CONFIGURATION_H_
#define SRC_STORAGE_BLOBFS_SERVICE_OVERWRITE_CONFIGURATION_H_

#include <fidl/fuchsia.storage.blobfs/cpp/wire.h>
#include <lib/async/dispatcher.h>

#include "src/storage/blobfs/blobfs.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {

class OverwriteConfigurationService
    : public fidl::WireServer<fuchsia_storage_blobfs::OverwriteConfiguration>,
      public fs::Service {
 public:
  OverwriteConfigurationService(async_dispatcher_t* dispatcher, Blobfs& blobfs);

  void Set(SetRequestView request, SetCompleter::Sync& completer) final;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_storage_blobfs::OverwriteConfiguration> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) final;

 private:
  Blobfs& blobfs_;
  fidl::ServerBindingGroup<fuchsia_storage_blobfs::OverwriteConfiguration> bindings_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_SERVICE_OVERWRITE_CONFIGURATION_H_
