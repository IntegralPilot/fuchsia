// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "local-vnode.h"

#include <lib/zx/channel.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/zxio.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/string_buffer.h>

#include "sdk/lib/fdio/internal.h"
#include "sdk/lib/fdio/zxio.h"

namespace fdio_internal {

size_t LocalVnode::Intermediate::num_children() const { return entries_by_id_.size(); }

zx::result<std::tuple<fbl::RefPtr<LocalVnode>, bool>> LocalVnode::Intermediate::LookupOrInsert(
    fbl::String name, fit::function<zx::result<fbl::RefPtr<LocalVnode>>(ParentAndId)> builder) {
  auto it = entries_by_name_.find(name);
  if (it != entries_by_name_.end()) {
    return zx::ok(std::make_tuple(it->node(), false));
  }
  zx::result vn_res = builder(std::make_tuple(std::reference_wrapper(*this), next_node_id_));
  if (vn_res.is_error()) {
    return vn_res.take_error();
  }
  auto& vn = vn_res.value();

  auto entry = std::make_unique<Entry>(next_node_id_, std::move(name), vn);
  entries_by_name_.insert(entry.get());
  entries_by_id_.insert(std::move(entry));
  next_node_id_++;

  return zx::ok(std::make_tuple(std::move(vn), true));
}

void LocalVnode::Intermediate::RemoveEntry(LocalVnode* vn, uint64_t id) {
  auto it = entries_by_id_.find(id);
  if (it != entries_by_id_.end() && it->node().get() == vn) {
    entries_by_name_.erase(it->name());
    entries_by_id_.erase(it);
  }
}

fbl::RefPtr<LocalVnode> LocalVnode::Intermediate::Lookup(const fbl::String& name) const {
  auto it = entries_by_name_.find(name);
  if (it != entries_by_name_.end()) {
    return it->node();
  }
  return nullptr;
}

LocalVnode::Remote::~Remote() {
  // Close the channel underlying the remote connection without making a Close
  // call to preserve previous behavior.
  zx::channel remote_channel;
  zxio_release(Connection(), remote_channel.reset_and_get_address());
}

LocalVnode::Intermediate::~Intermediate() {
  for (auto& entry : entries_by_id_) {
    entry.node()->parent_and_id_.reset();
  }
  entries_by_name_.clear();
  entries_by_id_.clear();
}

LocalVnode::Local::Local(fdio_open_local_func_t on_open, void* context)
    : on_open_(on_open), context_(context) {}

zx::result<fdio_ptr> LocalVnode::Local::Open() {
  if (on_open_ == nullptr) {
    return zx::error(ZX_ERR_BAD_HANDLE);
  }

  fdio_ptr io = fbl::MakeRefCounted<zxio>();
  if (io == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  zxio_ops_t const* ops = nullptr;
  zx_status_t status = on_open_(&io->zxio_storage(), context_, &ops);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  zxio_init(&io->zxio_storage().io, ops);
  return zx::ok(io);
}

void LocalVnode::UnlinkFromParent() {
  if (std::optional opt = std::exchange(parent_and_id_, {}); opt.has_value()) {
    const auto& [parent, id] = opt.value();
    parent.get().RemoveEntry(this, id);
  }
}

zx_status_t LocalVnode::EnumerateInternal(PathBuffer* path, std::string_view name,
                                          const EnumerateCallback& func) {
  const size_t original_length = path->length();

  // Add this current node to the path, and enumerate it if it has a remote
  // object.
  path->Append(name);

  std::visit(fdio_internal::overloaded{
                 [](const LocalVnode::Local& c) {
                   // Nothing to do as the node has no children and is not a
                   // remote node.
                 },
                 [&path, &func, &name](const LocalVnode::Intermediate& c) {
                   // If we added a node with children, add a separator and enumerate all the
                   // children.
                   if (!name.empty()) {
                     path->Append('/');
                   }

                   c.ForAllEntries([&path, &func](LocalVnode& child, std::string_view name) {
                     return child.EnumerateInternal(path, name, func);
                   });
                 },
                 [&path, &func](LocalVnode::Remote& s) {
                   // If we added a remote node, call the enumeration function on the remote node.
                   func(*path, s.Connection());
                 },
             },
             node_type_);

  // To re-use the same prefix buffer, restore the original buffer length
  // after enumeration has completed.
  path->Resize(original_length);
  return ZX_OK;
}

zx_status_t LocalVnode::EnumerateRemotes(const EnumerateCallback& func) {
  PathBuffer path;
  path.Append('/');
  return EnumerateInternal(&path, {}, func);
}

zx::result<std::string_view> LocalVnode::Readdir(uint64_t* last_seen) const {
  return std::visit(fdio_internal::overloaded{
                        [](const LocalVnode::Local&) -> zx::result<std::string_view> {
                          // Calling readdir on a Local node is invalid.
                          return zx::error(ZX_ERR_NOT_DIR);
                        },
                        [&](const LocalVnode::Intermediate& c) { return c.Readdir(last_seen); },
                        [](const LocalVnode::Remote&) -> zx::result<std::string_view> {
                          // If we've called Readdir on a Remote node, the path
                          // was misconfigured.
                          return zx::error(ZX_ERR_BAD_PATH);
                        },
                    },
                    node_type_);
}

zx::result<std::string_view> LocalVnode::Intermediate::Readdir(uint64_t* last_seen) const {
  for (auto it = entries_by_id_.lower_bound(*last_seen); it != entries_by_id_.end(); ++it) {
    if (it->id() <= *last_seen) {
      continue;
    }
    *last_seen = it->id();
    return zx::ok(it->name());
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

template <typename Fn>
zx_status_t LocalVnode::Intermediate::ForAllEntries(Fn fn) const {
  for (const Entry& entry : entries_by_id_) {
    zx_status_t status = fn(*entry.node(), entry.name());
    if (status != ZX_OK) {
      return status;
    }
  }
  return ZX_OK;
}

}  // namespace fdio_internal
