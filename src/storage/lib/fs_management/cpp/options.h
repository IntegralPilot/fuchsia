// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_OPTIONS_H_
#define SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_OPTIONS_H_

#include <fidl/fuchsia.fs.startup/cpp/wire.h>
#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <cstdint>
#include <functional>
#include <optional>

namespace fs_management {

// Because MountOptions is used for abstracting away mounting single volume or multivolume
// filesystems this becomes a mixture of fuchsia_fs_startup::wire::StartOptions and
// fuchsia_fs_startup::wire::MountOptions.
struct MountOptions {
  bool readonly = false;
  bool verbose_mount = false;

  // Ensures that requests to the mountpoint will be propagated to the underlying FS
  bool wait_until_ready = true;

  // If set, run fsck after every transaction.
  bool fsck_after_every_transaction = false;

  // If set, a callable that connects and returns a handle to the crypt service.
  std::function<zx::result<fidl::ClientEnd<fuchsia_fxfs::Crypt>>()> crypt_client;

  // If set, this is passed in as a duration to provide profile recording and replay.
  std::optional<uint32_t> startup_profiling_seconds;

  // If set, the system will be requested to use inline hardware crypto instead of in-process
  // encryption.
  std::optional<bool> inline_crypto_enabled;

  // If set, the system will be requested to use barriers instead of checksums to ensure data
  // consistency with respect to the journal.
  std::optional<bool> barriers_enabled;

  // Generate a StartOptions fidl struct to pass the a fuchsia.fs.startup.Startup interface based
  // on this set of options.
  __EXPORT
  zx::result<fuchsia_fs_startup::wire::StartOptions> as_start_options(fidl::AnyArena& arena) const;
};

struct MkfsOptions {
  uint32_t fvm_data_slices = 1;
  bool verbose = false;

  // The number of sectors per cluster on a FAT file systems or zero for the default.
  uint16_t sectors_per_cluster = 0;

  // Set to use the deprecated padded blobfs format.
  bool deprecated_padded_blobfs_format = false;

  // The initial number of inodes to allocate space for. If 0, a default is used. Only supported
  // for blobfs.
  uint64_t num_inodes = 0;

  // Generate a FormatOptions fidl struct to pass the a fuchsia.fs.startup.Startup interface based
  // on this set of options.
  __EXPORT
  fuchsia_fs_startup::wire::FormatOptions as_format_options(fidl::AnyArena& arena) const;
};

struct FsckOptions {
  bool verbose = false;

  // At MOST one of the following '*_modify' flags may be true.
  bool never_modify = false;   // Fsck still looks for problems, but does not try to resolve them.
  bool always_modify = false;  // Fsck never asks to resolve problems; it will always do it.
  bool force = false;          // Force fsck to check the filesystem integrity, even if "clean".

  // Generate a CheckOptions fidl struct to pass the a fuchsia.fs.startup.Startup interface based
  // on this set of options.
  //
  // The current set of filesystems that support launching with fuchsia.fs.startup.Startup don't
  // support any check options so this doesn't currently do anything. This function is provided for
  // consistency.
  __EXPORT
  fuchsia_fs_startup::wire::CheckOptions as_check_options() const;
};

}  // namespace fs_management

#endif  // SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_OPTIONS_H_
