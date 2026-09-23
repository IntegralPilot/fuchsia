// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"

#include <dirent.h>
#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>
#include <fbl/string.h>

#include "src/storage/lib/vfs/cpp/shared_mutex.h"
#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs {

PseudoDir::PseudoDir() = default;

PseudoDir::~PseudoDir() {
  entries_by_name_.clear_unsafe();
  entries_by_id_.clear();
}

zx_status_t PseudoDir::Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out) {
  std::lock_guard lock(mutex_);

  auto it = entries_by_name_.find(name);
  if (it != entries_by_name_.end()) {
    *out = it->node();
    return ZX_OK;
  }

  return ZX_ERR_NOT_FOUND;
}

void PseudoDir::Notify(std::string_view name, fio::wire::WatchEvent event) {
  watcher_.Notify(name, event);
}

zx_status_t PseudoDir::WatchDir(fs::FuchsiaVfs* vfs, fio::wire::WatchMask mask, uint32_t options,
                                fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) {
  return watcher_.WatchDir(vfs, this, mask, options, std::move(watcher));
}

zx_status_t PseudoDir::Readdir(VdirCookie* cookie, void* dirents, size_t len, size_t* out_actual) {
  fs::DirentFiller df(dirents, len);
  if (cookie->n < kDotId) {
    constexpr uint64_t ino = fio::kInoUnknown;
    if (!df.Next(".", fio::DirentType::kDirectory, ino)) {
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    cookie->n = kDotId;
  }

  std::lock_guard lock(mutex_);

  for (auto it = entries_by_id_.lower_bound(cookie->n); it != entries_by_id_.end(); ++it) {
    if (cookie->n >= it->id()) {
      continue;
    }
    zx::result attr = it->node()->GetAttributes();
    if (!attr.is_ok()) {
      continue;
    }
    uint32_t mode =
        attr->mode ? *attr->mode
                   : internal::GetPosixMode(it->node()->GetProtocols(), it->node()->GetAbilities());
    const uint8_t d_type = IFTODT(mode);

    if (!df.Next(it->name(), fio::DirentType{d_type}, attr->id.value_or(fio::kInoUnknown))) {
      *out_actual = df.BytesFilled();
      return *out_actual == 0 ? ZX_ERR_BUFFER_TOO_SMALL : ZX_OK;
    }
    cookie->n = it->id();
  }

  *out_actual = df.BytesFilled();
  return ZX_OK;
}

fuchsia_io::NodeProtocolKinds PseudoDir::GetProtocols() const {
  return fuchsia_io::NodeProtocolKinds::kDirectory;
}

fuchsia_io::Abilities PseudoDir::GetAbilities() const {
  return fio::Abilities::kGetAttributes | fio::Abilities::kTraverse | fio::Abilities::kEnumerate;
}

zx_status_t PseudoDir::AddEntry(fbl::String name, fbl::RefPtr<fs::Vnode> vn) {
  ZX_DEBUG_ASSERT(vn);

  if (!IsValidName(name)) {
    return ZX_ERR_INVALID_ARGS;
  }

  std::lock_guard lock(mutex_);

  if (entries_by_name_.find(name) != entries_by_name_.end()) {
    return ZX_ERR_ALREADY_EXISTS;
  }

  Notify(name, fio::wire::WatchEvent::kAdded);
  auto entry = std::make_unique<Entry>(next_node_id_++, std::move(name), std::move(vn));
  entries_by_name_.insert(entry.get());
  entries_by_id_.insert(std::move(entry));
  return ZX_OK;
}

zx_status_t PseudoDir::RemoveEntry(std::string_view name) {
  std::lock_guard lock(mutex_);

  auto it = entries_by_name_.find(name);
  if (it != entries_by_name_.end()) {
    entries_by_name_.erase(it);
    entries_by_id_.erase(it->id());
    Notify(name, fio::wire::WatchEvent::kRemoved);
    return ZX_OK;
  }

  return ZX_ERR_NOT_FOUND;
}

zx_status_t PseudoDir::RemoveEntry(std::string_view name, fs::Vnode* vn) {
  std::lock_guard lock(mutex_);

  auto it = entries_by_name_.find(name);
  if (it != entries_by_name_.end() && it->node().get() == vn) {
    entries_by_name_.erase(it);
    entries_by_id_.erase(it->id());
    Notify(name, fio::wire::WatchEvent::kRemoved);
    return ZX_OK;
  }

  return ZX_ERR_NOT_FOUND;
}

void PseudoDir::RemoveAllEntries() {
  std::lock_guard lock(mutex_);

  for (auto& entry : entries_by_name_) {
    Notify(entry.name(), fio::wire::WatchEvent::kRemoved);
  }
  entries_by_name_.clear();
  entries_by_id_.clear();
}

bool PseudoDir::IsEmpty() const {
  SharedLock lock(mutex_);
  return entries_by_name_.is_empty();
}

PseudoDir::Entry::Entry(uint64_t id, fbl::String name, fbl::RefPtr<fs::Vnode> node)
    : id_(id), name_(std::move(name)), node_(std::move(node)) {}

PseudoDir::Entry::~Entry() = default;

}  // namespace fs
