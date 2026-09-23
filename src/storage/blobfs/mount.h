// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_MOUNT_H_
#define SRC_STORAGE_BLOBFS_MOUNT_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>

#include "src/storage/blobfs/compression/external_decompressor.h"

namespace blobfs {

enum class Writability {
  // Do not write to persistent storage under any circumstances whatsoever.
  ReadOnlyDisk,
  // Do not allow users of the filesystem to mutate filesystem state. This state allows the journal
  // to replay while initializing writeback.
  ReadOnlyFilesystem,
  // Permit all operations.
  Writable,
};

// Toggles that may be set on blobfs during initialization.
struct MountOptions {
  Writability writability = Writability::Writable;
  bool verbose = false;

  // Used to establish fidl connections to the DecompressorCreator instead of the default
  // implementation that will perform an |fdio_service_connect| with the given channel.
  DecompressorCreatorConnector* decompression_connector = nullptr;

  int paging_threads = 2;
#ifndef NDEBUG
  bool fsck_at_end_of_every_transaction = false;
#endif
};

struct ComponentOptions {
  int pager_threads = 1;
};

// Start blobfs as a component. Begin serving requests on the provided |root|. Initially it starts
// the filesystem in an unconfigured state, only serving the fuchsia.fs.Startup protocol. Once
// fuchsia.fs.Startup/Start is called with the block device and mount options, the filesystem is
// started with that configuration and begins serving requests to other protocols, including the
// actual root of the filesystem at /root.
//
// Also expects a lifecycle server end over which to serve fuchsia.process.lifecycle/Lifecycle for
// shutting down the blobfs component.
//
// This function blocks until the filesystem terminates.
zx::result<> StartComponent(ComponentOptions options, fidl::ServerEnd<fuchsia_io::Directory> root,
                            fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> lifecycle);

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_MOUNT_H_
