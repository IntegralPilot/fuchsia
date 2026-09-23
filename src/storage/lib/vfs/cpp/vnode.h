// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_VNODE_H_
#define SRC_STORAGE_LIB_VFS_CPP_VNODE_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/string_view.h>
#include <lib/zx/result.h>
#include <limits.h>
#include <stdint.h>
#include <stdlib.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <map>
#include <memory>
#include <mutex>
#include <shared_mutex>
#include <string>
#include <string_view>
#include <utility>

#include <fbl/intrusive_single_list.h>
#include <fbl/macros.h>
#include <fbl/recycler.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/ref_counted.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"

#ifdef __Fuchsia__
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/file-lock/file-lock.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#endif  // __Fuchsia__

namespace fs {

class Vfs;
struct VdirCookie;

#ifdef __Fuchsia__
class FuchsiaVfs;
#endif

inline bool IsValidName(std::string_view name) {
  return !name.empty() && name != "." && name != ".." && name.length() <= NAME_MAX &&
         !cpp23::contains(name, '/');
}

// The VFS interface declares a default abstract Vnode class with common operations that may be
// overridden.
//
// The ops are used for dispatch and the lifecycle of Vnodes are owned by RefPtrs.
//
// All names passed to the Vnode class are valid according to "IsValidName".
//
// Memory management
// -----------------
// The Vnode uses the fbl::Recyclable system to allow caching. This kicks in when the reference
// count of the node goes to zero.
//
// fbl::RefPtr uses fbl::internal::has_fbl_recycle_v which checks whether there is an fbl_recycle()
// implementation on the class being pointed to. This does not catch base class implementations!
//
// Each derived class must inherit from fbl::Recyclable<class_name> and implement an fbl_recycle()
// function. These implementations should call the protected virtual function RecycleNode().
//
// Derived classes should override RecycleNode() to implement the desired caching behavior.
class Vnode : public VnodeRefCounted<Vnode>, public fbl::Recyclable<Vnode> {
 public:
  // Define the current Vfs type so Fuchsia-ifdefed code can assume a FuchsiaVfs associated
  // class if vfs_ is non-null.
#ifdef __Fuchsia__
  using PlatformVfs = FuchsiaVfs;
#else
  using PlatformVfs = Vfs;
#endif

  virtual ~Vnode();

  // See class comment above about memory management.
  void fbl_recycle() { RecycleNode(); }

  // METHODS FOR OPTION VALIDATION AND PROTOCOL NEGOTIATION
  //
  // Implementations should override |GetProtocols| to express which representation(s) are supported
  // by the vnode. Implementations may optionally override |Negotiate| to insert custom tie-breaking
  // behavior when the vnode supports multiple protocols, and the client requested multiple at open
  // time.

  // Returns the set of all protocols supported by the vnode.
  virtual fuchsia_io::NodeProtocolKinds GetProtocols() const = 0;

  // Returns the set of operations the vnode supports. The default implementation assumes files are
  // readable/writable, and directories are mutable.
  virtual fuchsia_io::Abilities GetAbilities() const;

  // Returns true if the vnode supports at least one protocol specified in |protocols|.
  bool Supports(fuchsia_io::NodeProtocolKinds protocols) const {
    return static_cast<bool>(GetProtocols() & protocols);
  }
  // To be overridden by implementations to check that it is valid to access the vnode with the
  // given |rights|. The default implementation always returns true. The vnode will only be opened
  // for a particular request if the validation passes.
  virtual bool ValidateRights(fuchsia_io::Rights rights) const;

  // Ensures that it is valid to access the vnode with given io1 connection options.
  zx::result<> DeprecatedValidateOptions(DeprecatedOptions options) const;

  // Opens the vnode. This is a callback to signal that a new connection is about to be created and
  // I/O operations will follow. In addition, it provides an opportunity to redirect subsequent I/O.
  // If the open fails, the file will be deemed to be not opened and Close() will not be called.
  //
  // Vnode implementations should override OpenNode() which this function calls after some
  // bookkeeping.
  //
  // Open will never be invoked when a node reference connection is created. This corresponds
  // to Posix open()'s O_PATH flag which will create a thing representing the path to the file
  // without giving the ability to do most operations like read or write. In the future, we may want
  // the ability to track these connections, in which case we should add a Connect()/Disconnect()
  // pair that would surround the Open()/Close() for the normal case, but would be called regardless
  // of the flags to cover the node-reference case.
  //
  // If the implementation of |Open()| sets |out_redirect| to a non-null value, all following I/O
  // operations on the opened object will be redirected to the indicated vnode instead of being
  // handled by this instance. This is useful when implementing lazy files/pseudo files, where a
  // different vnode may be used for each new connection to a file. Note that the |out_redirect|
  // vnode is not |Open()|ed further for the purpose of creating this connection. Furthermore, the
  // redirected vnode must support the same set of protocols as the original vnode.
  zx_status_t Open(fbl::RefPtr<Vnode>* out_redirect) __TA_EXCLUDES(mutex_);

  // METHODS FOR OPENED NODES
  //
  // The following operations will not be invoked unless the Vnode has been "Open()"-ed
  // successfully.
  //
  // For files opened with O_PATH (as a file descriptor only) the base classes' implementation of
  // some of these functions may be invoked anyway.

  // Invoked by the VFS layer whenever files are added or removed.
  virtual void Notify(std::string_view name, fuchsia_io::wire::WatchEvent event) {}

#ifdef __Fuchsia__
  // Serves a custom FIDL protocol over the specified |channel|, when the node protocol is
  // |VnodeProtocol::kService|.
  //
  // The default implementation returns |ZX_ERR_NOT_SUPPORTED|.
  // Subclasses may override this behavior to serve custom protocols over the channel.
  virtual zx_status_t ConnectService(zx::channel channel);

  virtual zx_status_t WatchDir(FuchsiaVfs* vfs, fuchsia_io::wire::WatchMask mask, uint32_t options,
                               fidl::ServerEnd<fuchsia_io::DirectoryWatcher> watcher);

  // Create a |zx::stream| for reading and writing to this vnode.
  //
  // On success, all |Read|, |Write|, and |Append| operations will be directed to the returned
  // stream rather than to the |Read|, |Write|, and |Append| methods on the vnode.
  //
  // The |zx::stream| will be duplicated and transported to a remote process to improve performance.
  // The |zx::vmo| backing the |zx::stream| will itself need to be |zx::pager| backed in order to
  // detect writes made to the |zx::stream|.
  //
  // Implementations should pass the given |stream_options| as the options to |zx::stream::create|.
  // These options ensure that the created |zx::stream| object has the appropriate rights for the
  // given connection.
  //
  // If the vnode does not support reading and writing using a |zx::stream|, return
  // ZX_ERR_NOT_SUPPORTED, which will cause |Read|, |Write|, and |Append| operations to be called as
  // methods on the vnode. Other errors are considered fatal and will terminate the connection.
  virtual zx::result<zx::stream> CreateStream(uint32_t stream_options);

  // Acquire a vmo from a vnode.
  //
  // At the moment, mmap can only map files from read-only filesystems, since (without paging) there
  // is no mechanism to update either
  // 1) The file by writing to the mapping, or
  // 2) The mapping by writing to the underlying file.
  virtual zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo);

  // If |IsRemote()| returns true, requests to open this Vnode using
  // fuchsia.io/Directory.DeprecatedOpen will be forwarded to this function.
  // TODO(https://fxbug.dev/324080864): This should be removed when we drop support for the
  // fuchsia.io/Directory.DeprecatedOpen method.
#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  virtual void DeprecatedOpenRemote(fuchsia_io::OpenFlags, fuchsia_io::ModeType, fidl::StringView,
                                    fidl::ServerEnd<fuchsia_io::Node>) const;
#endif

  // If |IsRemote()| returns true, requests to open this Vnode via fuchsia.io/Directory.Open will
  // be forwarded to this function.
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
  virtual void OpenRemote(fuchsia_io::wire::OpenableOpenRequest request) const;
#else
  virtual void OpenRemote(fuchsia_io::wire::DirectoryOpenRequest request) const;
#endif

  // Instead of adding a |file_lock::FileLock| member variable to |Vnode|,
  // maintain a map from |this| to the lock objects. This is done, because
  // file locking only applies to regular files, so we want to avoid the
  // memory overhead for all other |Vnode| types.
  std::shared_ptr<file_lock::FileLock> GetVnodeFileLock();

  // Removes any locks held by |owner|. This should only be called when tearing down a connection
  // and |owner| should be the koid of the connection.
  void DeleteFileLockInConnectionTeardown(zx_koid_t owner);
#endif  // __Fuchsia__

  // Closes the vnode. Will be called once for each successful Open().
  //
  // Vnode implementations should override CloseNode() which this function calls after some
  // bookkeeping.
  zx_status_t Close() __TA_EXCLUDES(mutex_);

  // Read data from the vnode at offset.
  //
  // If successful, returns the number of bytes read in |out_actual|. This must be less than or
  // equal to |len|.
  //
  // See |CreateStream| for a mechanism to offload |Read| to a |zx::stream| object.
  virtual zx_status_t Read(void* data, size_t len, size_t off, size_t* out_actual);

  // Write |len| bytes of |data| to the file, starting at |offset|.
  //
  // If successful, returns the number of bytes written in |out_actual|. This must be less than or
  // equal to |len|.
  //
  // See |CreateStream| for a mechanism to offload |Write| to a |zx::stream| object.
  virtual zx_status_t Write(const void* data, size_t len, size_t offset, size_t* out_actual);

  // Write |len| bytes of |data| to the end of the file.
  //
  // If successful, returns the number of bytes written in |out_actual|, and
  // returns the new end of file offset in |out_end|.
  //
  // See |CreateStream| for a mechanism to offload |Append| to a |zx::stream| object.
  virtual zx_status_t Append(const void* data, size_t len, size_t* out_end, size_t* out_actual);

  // Change the size of the vnode.
  virtual zx_status_t Truncate(size_t len);

  // Syncs the vnode with its underlying storage.
  //
  // Returns the result status through a closure. The closure may be executed on a different thread
  // than called the Sync() function, or reentrantly from the same thread.
  using SyncCallback = fit::callback<void(zx_status_t status)>;
  virtual void Sync(SyncCallback closure);

  // Read directory entries of vn, error if not a directory. FS-specific Cookie must be a buffer of
  // VdirCookie size or smaller. Cookie must be zero'd before first call and will be used by the
  // readdir implementation to maintain state across calls. To "rewind" and start from the
  // beginning, cookie may be zero'd.
  virtual zx_status_t Readdir(VdirCookie* cookie, void* dirents, size_t len, size_t* out_actual);

  // METHODS FOR OPENED OR UNOPENED NODES
  //
  // The following operations may be invoked on a Vnode, even if it has not been "Open()"-ed.

  // Attempt to find child of vn, child returned on success.
  virtual zx_status_t Lookup(std::string_view name, fbl::RefPtr<Vnode>* out);

  // Get attributes of the vnode.
  virtual zx::result<fs::VnodeAttributes> GetAttributes() const;

  // Returns the set of attributes this vnode supports. Requests to update attributes which the
  // vnode does not support will be rejected.
  virtual VnodeAttributesQuery SupportedMutableAttributes() const { return {}; }

  // Update attributes of the vnode. Only those attributes specified by
  // |SupportedMutableAttributes()| will be specified in |attributes|.
  virtual zx::result<> UpdateAttributes(const VnodeAttributesUpdate& attributes);

  // Create a new object with specified |name| and |type| under this Vnode. On success, the newly
  // created Vnode must already be opened (i.e. |Open()| will not be called on the result).
  virtual zx::result<fbl::RefPtr<Vnode>> Create(std::string_view name, CreationType type);

  // Removes name from directory vn
  virtual zx_status_t Unlink(std::string_view name, bool must_be_dir);

  // Renames the path at oldname in olddir to the path at newname in newdir. Called on the "olddir"
  // vnode.
  //
  // Unlinks any prior newname if it already exists.
  virtual zx_status_t Rename(fbl::RefPtr<Vnode> newdir, std::string_view oldname,
                             std::string_view newname, bool src_must_be_dir, bool dst_must_be_dir);

  // Creates a hard link to the 'target' vnode with a provided name in vndir
  virtual zx_status_t Link(std::string_view name, fbl::RefPtr<Vnode> target);

  // Returns true if this is a remote filesystem mount point. This is only relevant on Fuchsia
  // builds (the remote handling below is all Fuchsia-only) but this can exist and just return false
  // on host builds to simplify platform handling.
  virtual bool IsRemote() const;

 protected:
  DISALLOW_COPY_ASSIGN_AND_MOVE(Vnode);

  // Implementation of fbl_recycle(). Normal fbl_recycle() use is non-virtual and requires different
  // inheritance paths to fbl::Recyclable. This virtual implementation allows there to be one
  // implementation.
  //
  // This function is called when the object reference count drops to 0. This default implementation
  // just deletes the object to get "normal" reference counting. Derived classes can override to
  // implement caching if desired.
  //
  // See the class comment above on recycling, this is subtle.
  virtual void RecycleNode() { delete this; }

  // Opens/Closes the vnode. These are the callbacks that the Vnode implementation overrides to do
  // the open and close work. They are called by the public Open() and Close() functions which
  // handles bookkeeping for the base class.
  //
  // The open_count() will be updated BEFORE each call. If OpenNode fails, the open count will be
  // rolled back.
  //
  // See Open() above for documentation.
  virtual zx_status_t OpenNode(fbl::RefPtr<Vnode>* out_redirect) __TA_EXCLUDES(mutex_) {
    return ZX_OK;
  }
  virtual zx_status_t CloseNode() __TA_EXCLUDES(mutex_) { return ZX_OK; }

  Vnode() = default;

  // Mutex for the data of this vnode. This is a shared mutex to support derived classes
  // implementing multiple simultaneous readers if desired.
  mutable std::shared_mutex mutex_;

  // Returns the number of open connections, not counting node_reference connections. See Open().
  size_t open_count() const __TA_REQUIRES_SHARED(mutex_) { return open_count_; }

 private:
  size_t open_count_ __TA_GUARDED(mutex_) = 0;
};

// Opens a vnode by reference.
// The |vnode| reference is updated in-place if redirection occurs.
inline zx_status_t OpenVnode(fbl::RefPtr<Vnode>* vnode) {
  fbl::RefPtr<Vnode> redirect;
  zx_status_t status = (*vnode)->Open(&redirect);
  if (status == ZX_OK && redirect != nullptr) {
    ZX_DEBUG_ASSERT((*vnode)->GetProtocols() == redirect->GetProtocols());
    *vnode = std::move(redirect);
  }
  return status;
}

// Helper class used to fill direntries during calls to Readdir.
class DirentFiller {
 public:
  DISALLOW_COPY_ASSIGN_AND_MOVE(DirentFiller);

  DirentFiller(void* ptr, size_t len);

  // Attempts to add the name to the end of the dirent buffer which is returned by readdir.
  //  - Returns true if the name was added to the buffer.
  //  - Returns false if the name was not added to the buffer because it did not fit.
  //  - Panics if the name is longer than |NAME_MAX|.
  bool Next(std::string_view name, fuchsia_io::DirentType type, uint64_t ino);

  size_t BytesFilled() const { return pos_; }

 private:
  char* ptr_;
  size_t pos_;
  const size_t len_;
};

// Helper class to track outstanding operations associated to a
// particular Vnode.
class VnodeToken : public fbl::SinglyLinkedListable<std::unique_ptr<VnodeToken>> {
 public:
  VnodeToken(zx_koid_t koid, fbl::RefPtr<Vnode> vnode) : koid_(koid), vnode_(std::move(vnode)) {}

  zx_koid_t get_koid() const { return koid_; }
  fbl::RefPtr<Vnode> get_vnode() const { return vnode_; }

  // Trait implementation for fbl::HashTable
  zx_koid_t GetKey() const { return koid_; }
  static size_t GetHash(zx_koid_t koid) { return koid; }

 private:
  zx_koid_t koid_;
  fbl::RefPtr<Vnode> vnode_;
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_VNODE_H_
