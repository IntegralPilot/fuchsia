// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/vnode.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <lib/zx/result.h>
#include <limits.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <string_view>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

#ifdef __Fuchsia__
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/file-lock/file-lock.h>
#include <lib/zx/channel.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#include <zircon/availability.h>

#include <map>
#include <memory>
#include <utility>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#endif  // __Fuchsia__

namespace fio = fuchsia_io;

namespace fs {

#ifdef __Fuchsia__
namespace {
std::mutex gFileLockMutex;
std::map<const Vnode*, std::shared_ptr<file_lock::FileLock>> gFileLockMap
    __TA_GUARDED(gFileLockMutex);
}  // namespace
#endif

#ifdef __Fuchsia__
Vnode::~Vnode() {
  std::scoped_lock lock(gFileLockMutex);
  ZX_DEBUG_ASSERT_MSG(!gFileLockMap.contains(this),
                      "lock entry in gFileLockMap not cleaned up for Vnode");
}
#else
Vnode::~Vnode() = default;
#endif

#ifdef __Fuchsia__

zx::result<zx::stream> Vnode::CreateStream(uint32_t stream_options) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t Vnode::ConnectService(zx::channel channel) { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t Vnode::WatchDir(FuchsiaVfs* vfs, fio::wire::WatchMask mask, uint32_t options,
                            fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Vnode::GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) {
  return ZX_ERR_NOT_SUPPORTED;
}

#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
void Vnode::DeprecatedOpenRemote(fuchsia_io::OpenFlags, fuchsia_io::ModeType, fidl::StringView,
                                 fidl::ServerEnd<fuchsia_io::Node>) const {
  ZX_PANIC("OpenRemote should only be called on remote nodes!");
}
#endif

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void Vnode::OpenRemote(fuchsia_io::wire::OpenableOpenRequest request) const {
#else
void Vnode::OpenRemote(fuchsia_io::wire::DirectoryOpenRequest request) const {
#endif
  ZX_PANIC("OpenRemote should only be called on remote nodes!");
}

std::shared_ptr<file_lock::FileLock> Vnode::GetVnodeFileLock() {
  std::scoped_lock lock_access(gFileLockMutex);
  auto file_lock_it = gFileLockMap.find(this);
  if (file_lock_it != gFileLockMap.end()) {
    return file_lock_it->second;
  } else {
    auto inserted = gFileLockMap.emplace(this, std::make_shared<file_lock::FileLock>());
    return inserted.first->second;
  }
}

void Vnode::DeleteFileLockInConnectionTeardown(zx_koid_t owner) {
  std::scoped_lock lock_access(gFileLockMutex);
  auto file_lock_it = gFileLockMap.find(this);
  if (file_lock_it != gFileLockMap.end()) {
    file_lock_it->second->Forget(owner);
    if (file_lock_it->second->NoLocksHeld()) {
      gFileLockMap.erase(this);
    }
  }
}

#endif  // __Fuchsia__

bool Vnode::ValidateRights([[maybe_unused]] fuchsia_io::Rights rights) const { return true; }

zx::result<> Vnode::DeprecatedValidateOptions(DeprecatedOptions options) const {
  // The connection should ensure only one of DIRECTORY and NOT_DIRECTORY is set.
  ZX_DEBUG_ASSERT(!((options.flags & fuchsia_io::OpenFlags::kDirectory) &&
                    options.flags & fuchsia_io::OpenFlags::kNotDirectory));
  if (!Supports(options.protocols())) {
    if (options.protocols() & fuchsia_io::NodeProtocolKinds::kDirectory) {
      return zx::error(ZX_ERR_NOT_DIR);
    }
    return zx::error(ZX_ERR_NOT_FILE);
  }
  if (!ValidateRights(options.rights)) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  return zx::ok();
}

zx_status_t Vnode::Open(fbl::RefPtr<Vnode>* out_redirect) {
  {
    std::lock_guard lock(mutex_);
    open_count_++;
  }

  if (zx_status_t status = OpenNode(out_redirect); status != ZX_OK) {
    // Roll back the open count since we won't get a close for it.
    std::lock_guard lock(mutex_);
    open_count_--;
    return status;
  }
  return ZX_OK;
}

zx_status_t Vnode::Close() {
  {
    std::lock_guard lock(mutex_);
    open_count_--;
  }
  return CloseNode();
}

zx_status_t Vnode::Read(void* data, size_t len, size_t off, size_t* out_actual) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Vnode::Write(const void* data, size_t len, size_t offset, size_t* out_actual) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Vnode::Append(const void* data, size_t len, size_t* out_end, size_t* out_actual) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Vnode::Lookup(std::string_view name, fbl::RefPtr<Vnode>* out) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx::result<fs::VnodeAttributes> Vnode::GetAttributes() const {
  // Return the empty set of attributes by default.
  return zx::ok(fs::VnodeAttributes{});
}

zx::result<> Vnode::UpdateAttributes(const VnodeAttributesUpdate&) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t Vnode::Readdir(VdirCookie* cookie, void* dirents, size_t len, size_t* out_actual) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx::result<fbl::RefPtr<Vnode>> Vnode::Create(std::string_view name, CreationType type) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t Vnode::Unlink(std::string_view name, bool must_be_dir) { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t Vnode::Truncate(size_t len) { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t Vnode::Rename(fbl::RefPtr<Vnode> newdir, std::string_view oldname,
                          std::string_view newname, bool src_must_be_dir, bool dst_must_be_dir) {
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t Vnode::Link(std::string_view name, fbl::RefPtr<Vnode> target) {
  return ZX_ERR_NOT_SUPPORTED;
}

void Vnode::Sync(SyncCallback closure) { closure(ZX_ERR_NOT_SUPPORTED); }

bool Vnode::IsRemote() const { return false; }

fio::Abilities Vnode::GetAbilities() const {
  fio::Abilities abilities = fio::Abilities::kGetAttributes;
  if (SupportedMutableAttributes()) {
    abilities |= fio::Abilities::kUpdateAttributes;
  }
  fio::NodeProtocolKinds protocols = GetProtocols();
  if (protocols & fio::NodeProtocolKinds::kDirectory) {
    abilities |=
        fio::Abilities::kModifyDirectory | fio::Abilities::kTraverse | fio::Abilities::kEnumerate;
  }
  if (protocols & fio::NodeProtocolKinds::kFile) {
    abilities |= fio::Abilities::kReadBytes | fio::Abilities::kWriteBytes;
  }
  return abilities;
}

DirentFiller::DirentFiller(void* ptr, size_t len)
    : ptr_(static_cast<char*>(ptr)), pos_(0), len_(len) {}

bool DirentFiller::Next(std::string_view name, fio::DirentType type, uint64_t ino) {
  // Assert in debug builds that the name isn't too long. Truncate the name in release builds.
  ZX_DEBUG_ASSERT(name.length() <= NAME_MAX);
  size_t name_length = std::min(name.length(), static_cast<size_t>(NAME_MAX));
  fs::DirectoryEntry* de = reinterpret_cast<fs::DirectoryEntry*>(ptr_ + pos_);
  size_t sz = sizeof(fs::DirectoryEntry) + name_length;
  if (sz > len_ - pos_) {
    return false;
  }
  de->ino = ino;
  de->name_length = static_cast<uint8_t>(name_length);
  de->type = type;
  memcpy(de->name, name.data(), name_length);
  pos_ += sz;
  return true;
}

}  // namespace fs
