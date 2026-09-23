// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VFS_CPP_PSEUDO_DIR_H_
#define LIB_VFS_CPP_PSEUDO_DIR_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/vfs/cpp/node.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <map>
#include <memory>
#include <mutex>
#include <string>

namespace vfs {

// A pseudo-directory is a directory-like object whose entries are constructed by a program at
// runtime. The client can lookup, enumerate, and watch these directory entries but it cannot
// create, remove, or rename them.
//
// This class is thread-safe.
class PseudoDir final : public Node {
 public:
  PseudoDir() : Node(CreateDirectory()) {}

  ~PseudoDir() override {
    // We must close all connections to the nodes this directory owns before destroying them, since
    // some nodes have state which cannot be owned by the connections.
    vfs_internal_node_shutdown(handle_);
  }

  // Serve a new connection to this pseudo-directory on `server_end` using specified `flags`.
  //
  // This method must only be used with a single-threaded asynchronous dispatcher. If `dispatcher`
  // is `nullptr`, the current thread's default dispatcher will be used via
  // `async_get_default_dispatcher`. The same `dispatcher` must be used if multiple connections are
  // served for the same node, otherwise `ZX_ERR_INVALID_ARGS` will be returned.
  zx_status_t Serve(fuchsia_io::Flags flags, fidl::ServerEnd<fuchsia_io::Directory> server_end,
                    async_dispatcher_t* dispatcher = nullptr) const {
    if (flags & (fuchsia_io::wire::kMaskKnownProtocols ^ fuchsia_io::Flags::kProtocolDirectory)) {
      return ZX_ERR_INVALID_ARGS;  // Only the directory protocol is allowed with this signature.
    }
    return ServeInternal(flags | fuchsia_io::Flags::kProtocolDirectory, server_end.TakeChannel(),
                         dispatcher);
  }

  // Adds a directory entry associating the given `name` with `vn`. The same node may be added
  // multiple times with different names. Returns `ZX_ERR_ALREADY_EXISTS` if there is already a node
  // associated with `name`.
  zx_status_t AddSharedEntry(std::string name, std::shared_ptr<Node> vn) {
    std::lock_guard guard(mutex_);
    if (node_map_.find(name) != node_map_.cend()) {
      return ZX_ERR_ALREADY_EXISTS;
    }
    if (zx_status_t status = vfs_internal_directory_add(handle(), vn->handle(), name.c_str());
        status != ZX_OK) {
      return status;
    }
    node_map_.emplace(std::move(name), std::move(vn));
    return ZX_OK;
  }

  // Adds a directory entry associating the given `name` with `vn`. Returns `ZX_ERR_ALREADY_EXISTS`
  // if there is already a node associated with `name`.
  zx_status_t AddEntry(std::string name, std::unique_ptr<Node> vn) {
    return AddSharedEntry(std::move(name), std::move(vn));
  }

  // Removes a directory entry with the given `name`, closing any active connections to the node
  // in the process. Returns `ZX_ERR_NOT_FOUND` there is no node with `name`.
  zx_status_t RemoveEntry(const std::string& name) { return RemoveEntryImpl(name, nullptr); }

  // Removes a directory entry with the given `name` that matches `node`, closing any active
  // connections to the node in the process. Returns `ZX_ERR_NOT_FOUND` there is no node that
  // matches both `name` and `node`.
  zx_status_t RemoveEntry(const std::string& name, const Node* node) {
    return RemoveEntryImpl(name, node);
  }

  // Checks if directory is empty. Use caution if modifying this directory from multiple threads.
  bool IsEmpty() const {
    std::lock_guard guard(mutex_);
    return node_map_.empty();
  }

  // Finds and returns a node matching `name` as `out_node`. This directory maintains ownership
  // of `out_node`. Returns `ZX_ERR_NOT_FOUND` if there is no node with `name`.
  zx_status_t Lookup(std::string_view name, Node** out_node) const {
    std::lock_guard guard(mutex_);
    if (auto node_it = node_map_.find(name); node_it != node_map_.cend()) {
      *out_node = node_it->second.get();
      return ZX_OK;
    }
    return ZX_ERR_NOT_FOUND;
  }

 private:
  static vfs_internal_node_t* CreateDirectory() {
    vfs_internal_node_t* dir;
    ZX_ASSERT(vfs_internal_directory_create(&dir) == ZX_OK);
    return dir;
  }

  zx_status_t RemoveEntryImpl(const std::string& name, const Node* node) {
    std::lock_guard guard(mutex_);
    if (auto found = node_map_.find(name); found != node_map_.cend()) {
      if (node && node != found->second.get()) {
        return ZX_ERR_NOT_FOUND;
      }
      if (zx_status_t status = vfs_internal_directory_remove(handle(), name.c_str());
          status != ZX_OK) {
        return status;
      }
      node_map_.erase(found);
      return ZX_OK;
    }
    return ZX_ERR_NOT_FOUND;
  }

  mutable std::mutex mutex_;

  // *NOTE*: Due to the SDK VFS `Lookup()` semantics, we need to maintain a strong reference to the
  // nodes added to this directory. `Lookup()` returns a `vfs::Node*` which callers downcast to the
  // concrete node type. The underlying `vfs_internal_node_t` type has no concept of the `vfs::Node`
  // type, so we must store them here to allow safe downcasting.
  std::map<std::string, std::shared_ptr<Node>, std::less<>> node_map_ __TA_GUARDED(mutex_);
};
}  // namespace vfs

#endif  // LIB_VFS_CPP_PSEUDO_DIR_H_
