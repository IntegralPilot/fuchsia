// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_FUCHSIA_VFS_H_
#define SRC_STORAGE_LIB_VFS_CPP_FUCHSIA_VFS_H_

#include <fidl/fuchsia.fs/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/sync/completion.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>

#include <fbl/intrusive_hash_table.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

namespace internal {
class Connection;
class DirectoryConnection;
}  // namespace internal

// An internal version of fuchsia_io::wire::FilesystemInfo with a simpler API and default
// initializers. See that FIDL struct for documentation.
struct FilesystemInfo {
  uint64_t total_bytes = 0;
  uint64_t used_bytes = 0;
  uint64_t total_nodes = 0;
  uint64_t used_nodes = 0;
  uint64_t free_shared_pool_bytes = 0;
  uint64_t fs_id = 0;
  uint32_t block_size = 0;
  uint32_t max_filename_size = 0;
  fuchsia_fs::VfsType fs_type = fuchsia_fs::VfsType::Unknown();
  std::string name;  // Length must be less than MAX_FS_NAME_BUFFER.

  // To ensure global uniqueness, filesystems should create and maintain an event object. The koid
  // of this object is guaranteed unique in the system and is used for the filesystem ID. This
  // function extracts the koid of the given event object and sets it as the filesystem ID.
  void SetFsId(const zx::event& event);

  // Writes this object's values to the given FIDL object.
  fuchsia_io::wire::FilesystemInfo ToFidl() const;
};

// Vfs specialization that adds Fuchsia-specific
class FuchsiaVfs : public Vfs {
 private:
  friend class internal::DirectoryConnection;  // To allow access to ServeResult

  // To deal with the lifetime issues with connections, we roll our own (limited) shared/weak
  // pointers.  We can't easily use shared_ptr because that would involve changing all the clients,
  // or would incur some overheads that we don't need.  Connections cannot be terminated
  // synchronously (FIDL doesn't provide a synchronous unbind), so we have to allow connections to
  // outlive the VFS.  To avoid connections making bad calls into the VFS instance after it has been
  // destroyed, connections hold weak references to the VFS instance and upgrade when they need to.
  // The VFS instance will block destruction whilst there are strong references.  This Ref struct
  // stores the reference counts and can outlive the VFS instance.  When there are no strong or weak
  // references the Ref instance is destroyed.
  struct Ref {
    std::atomic<int> strong_count;
    std::atomic<int> weak_count;
    FuchsiaVfs* vfs;
  };

 public:
  class SharedPtr {
   public:
    SharedPtr(const SharedPtr& other) { *this = other; }
    SharedPtr& operator=(const SharedPtr& other);
    ~SharedPtr() { Reset(); }
    void Reset();

    FuchsiaVfs& operator*() const { return *vfs_; }
    FuchsiaVfs* operator->() const { return vfs_; }
    explicit operator bool() const { return !!vfs_; }
    FuchsiaVfs* get() const { return vfs_; }

   private:
    friend class FuchsiaVfs;

    // Adopts a strong reference.
    explicit SharedPtr(FuchsiaVfs* vfs) : vfs_(vfs) {}

    FuchsiaVfs* vfs_ = nullptr;
  };

  class WeakPtr {
   public:
    explicit WeakPtr(FuchsiaVfs* vfs) : ref_(vfs->ref_) { ref_->weak_count.fetch_add(1); }
    explicit WeakPtr(const SharedPtr& ptr) : ref_(ptr->ref_) { ref_->weak_count.fetch_add(1); }
    WeakPtr(const WeakPtr&) = delete;
    WeakPtr& operator=(const WeakPtr&) = delete;
    ~WeakPtr() {
      // The last weak count also means no more strong counts because we hold an implicit weak
      // reference whilst there are strong references.
      if (ref_->weak_count.fetch_sub(1) == 1)
        delete ref_;
    }
    SharedPtr Upgrade() const;

   private:
    friend class FuchsiaVfs;

    // Adopts a weak reference.
    explicit WeakPtr(Ref* ref) : ref_(ref) {}

    Ref* ref_;
  };

  explicit FuchsiaVfs(async_dispatcher_t* dispatcher = nullptr);

  // Not copyable or movable
  FuchsiaVfs(const FuchsiaVfs&) = delete;
  FuchsiaVfs& operator=(const FuchsiaVfs&) = delete;

  ~FuchsiaVfs() override;

  using ShutdownCallback = fit::callback<void(zx_status_t status)>;
  using CloseAllConnectionsForVnodeCallback = fit::callback<void()>;

  // Identifies if the filesystem is in the process of terminating. May be checked by active
  // connections, which, upon reading new port packets, should ignore them and close immediately.
  bool IsTerminating() const { return is_terminating_; }

  // Vfs overrides.
  zx_status_t Unlink(fbl::RefPtr<Vnode> vn, std::string_view name, bool must_be_dir) override
      __TA_EXCLUDES(vfs_lock_);

  void TokenDiscard(zx::event ios_token) __TA_EXCLUDES(vfs_lock_);
  zx_status_t VnodeToToken(fbl::RefPtr<Vnode> vn, zx::event* ios_token, zx::event* out)
      __TA_EXCLUDES(vfs_lock_);
  zx_status_t Link(zx::event token, fbl::RefPtr<Vnode> oldparent, std::string_view oldStr,
                   std::string_view newStr) __TA_EXCLUDES(vfs_lock_);
  zx_status_t Rename(zx::event token, fbl::RefPtr<Vnode> oldparent, std::string_view oldStr,
                     std::string_view newStr, fuchsia_io::Rights source_rights)
      __TA_EXCLUDES(vfs_lock_);

  // Provides the implementation for fuchsia.io.Directory.QueryFilesystem().
  // This default implementation returns ZX_ERR_NOT_SUPPORTED.
  virtual zx::result<FilesystemInfo> GetFilesystemInfo() __TA_EXCLUDES(vfs_lock_);

  async_dispatcher_t* dispatcher() const { return dispatcher_; }
  void SetDispatcher(async_dispatcher_t* dispatcher);

  // Begins serving VFS messages over the specified channel. The protocol to use will be determined
  // by the intersection of the protocols requested in |options| and those supported by |vnode|.
  // |server_end| usually speaks a protocol that composes |fuchsia.io/Node|, but may speak an
  // arbitrary arbitrary protocol for service connections.
  //
  // On failure, |server_end| will be closed with an epitaph matching the returned error.
  //
  // *NOTE*: |vnode| must be opened before calling this function, and will be automatically closed
  // on failure. This does not apply to node reference connections, which should not open |vnode|.
  // TODO(https://fxbug.dev/324080864): Remove this method when we no longer need to support Open1.
  zx_status_t ServeDeprecated(const fbl::RefPtr<Vnode>& vnode, zx::channel server_end,
                              DeprecatedOptions options) __TA_EXCLUDES(vfs_lock_);

  // Begins serving VFS messages over the specified channel. The protocol to use will be determined
  // by the intersection of the protocols requested in |flags| and those supported by |vnode|.
  // The connection rights will be set to the |fuchsia.io/Flags.PERM_*| bits present in |flags|.
  // |server_end| usually speaks a protocol that composes |fuchsia.io/Node|, but may speak an
  // arbitrary arbitrary protocol for service connections.
  //
  // On failure, |channel| will be closed with an epitaph matching the returned status.
  zx_status_t Serve(fbl::RefPtr<Vnode> vn, zx::channel channel, fuchsia_io::Flags flags);

  // Serves a Vnode over the specified channel (used for creating new filesystems); the
  // Vnode must be a directory.
  zx_status_t ServeDirectory(fbl::RefPtr<Vnode> vn,
                             fidl::ServerEnd<fuchsia_io::Directory> server_end,
                             fuchsia_io::Rights rights);

  // Convenience wrapper over |ServeDirectory| with maximum rights.
  zx_status_t ServeDirectory(fbl::RefPtr<Vnode> vn,
                             fidl::ServerEnd<fuchsia_io::Directory> server_end) {
    return ServeDirectory(std::move(vn), std::move(server_end), fuchsia_io::Rights::kMask);
  }

  // Closes all connections to a Vnode and calls |callback| after all connections are closed. The
  // caller must ensure that no new connections or transactions are created during this point.
  virtual void CloseAllConnectionsForVnode(const Vnode& node,
                                           CloseAllConnectionsForVnodeCallback callback) = 0;

  bool IsTokenAssociatedWithVnode(zx::event token) __TA_EXCLUDES(vfs_lock_);

 protected:
  // Unmounts the underlying filesystem. The result of shutdown is delivered via calling |closure|.
  //
  // |Shutdown| may be synchronous or asynchronous. The closure may be invoked before or after
  // |Shutdown| returns.
  virtual void Shutdown(ShutdownCallback closure) = 0;

  // Serve |open_result| using negotiated protocol and specified |rights|. On failure, if
  // |object_request| was not consumed, the caller should close it with an epitaph.
  //
  // *NOTE*: |rights| and |flags| are ignored for services.
  zx::result<> ServeResult(OpenResult open_result, fuchsia_io::Rights rights,
                           zx::channel& object_request, fuchsia_io::Flags flags,
                           const fuchsia_io::wire::Options& options);

  // On success, starts handling requests for |vnode| over |server_end|. On failure, callers are
  // responsible for closing |server_end|.
  zx_status_t ServeImpl(fbl::RefPtr<Vnode> vn, zx::channel& server_end, fuchsia_io::Flags flags);

  // On success, starts handling requests for |vnode| over |server_end|. On failure, callers are
  // responsible for closing |vnode| and |server_end|.
  zx_status_t ServeDeprecatedImpl(const fbl::RefPtr<Vnode>& vnode, zx::channel& server_end,
                                  DeprecatedOptions options) __TA_EXCLUDES(vfs_lock_);

  // Starts FIDL message dispatching on |channel|, at the same time starts to manage the lifetime of
  // |connection|. Consumes |channel| on success. On error, callers must close the associated vnode.
  virtual zx::result<> RegisterConnection(std::unique_ptr<internal::Connection> connection,
                                          zx::channel& channel) = 0;

  // Indicates this VFS instance is soon to be destroyed.  After calling this, `WaitTillDone` can be
  // called to wait until there are no strong references remaining.  It is not safe to call this
  // more than once.
  void WillDestroy() {
    ZX_ASSERT(!is_terminating_);
    is_terminating_.store(true);
    // Return the strong count taken in the constructor.
    SharedPtr strong(this);
  }

  // Waits till there are no strong references.
  void WaitTillDone() { sync_completion_wait(&done_, ZX_TIME_INFINITE); }

 private:
  zx_status_t TokenToVnode(zx::event token, fbl::RefPtr<Vnode>* out) __TA_REQUIRES(vfs_lock_);

  fbl::HashTable<zx_koid_t, std::unique_ptr<VnodeToken>> vnode_tokens_;

  async_dispatcher_t* dispatcher_ = nullptr;

  std::atomic<bool> is_terminating_ = false;

  // Signalled when there are no more strong references.
  sync_completion_t done_;

  // Holds the reference counts.
  Ref* ref_;
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_FUCHSIA_VFS_H_
