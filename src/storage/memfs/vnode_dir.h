// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MEMFS_VNODE_DIR_H_
#define SRC_STORAGE_MEMFS_VNODE_DIR_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"
#include "src/storage/lib/vfs/cpp/watcher.h"
#include "src/storage/memfs/memfs.h"
#include "src/storage/memfs/vnode.h"

namespace memfs {

class VnodeDir final : public Vnode {
 public:
  explicit VnodeDir(Memfs& memfs);
  ~VnodeDir() override;

  fuchsia_io::NodeProtocolKinds GetProtocols() const final;

  zx_status_t Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) final;
  zx::result<fbl::RefPtr<fs::Vnode>> Create(std::string_view name, fs::CreationType vfs_type) final;

  // Create a vnode from a VMO.
  // Fails if the vnode already exists.
  // Passes the vmo to the Vnode; does not duplicate it.
  zx_status_t CreateFromVmo(std::string_view name, zx_handle_t vmo, zx_off_t off, zx_off_t len);

  // Use the watcher container to implement a directory watcher
  void Notify(std::string_view name, fuchsia_io::wire::WatchEvent event) final;
  zx_status_t WatchDir(fs::FuchsiaVfs* vfs, fuchsia_io::wire::WatchMask mask, uint32_t options,
                       fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) final;

 private:
  zx_status_t Readdir(fs::VdirCookie* cookie, void* dirents, size_t len, size_t* out_actual) final;

  // Resolves the question, "Can this directory create a child node with the name?"
  // Returns "ZX_OK" on success; otherwise explains failure with error message.
  zx_status_t CanCreate(std::string_view name) const;

  // Creates a dnode for the Vnode, attaches vnode to dnode, (if directory) attaches
  // dnode to vnode, and adds dnode to parent directory.
  zx_status_t AttachVnode(const fbl::RefPtr<memfs::Vnode>& vn, std::string_view name, bool isdir);

  zx_status_t Unlink(std::string_view name, bool must_be_dir) final;
  zx_status_t Rename(fbl::RefPtr<fs::Vnode> newdir, std::string_view oldname,
                     std::string_view newname, bool src_must_be_dir, bool dst_must_be_dir) final;
  zx_status_t Link(std::string_view name, fbl::RefPtr<fs::Vnode> target) final;
  zx::result<fs::VnodeAttributes> GetAttributes() const final;
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) final;

  Memfs& memfs_;
  fs::WatcherContainer watcher_;
};

}  // namespace memfs

#endif  // SRC_STORAGE_MEMFS_VNODE_DIR_H_
