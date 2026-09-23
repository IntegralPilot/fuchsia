// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_VMO_FILE_H_
#define SRC_STORAGE_LIB_VFS_CPP_VMO_FILE_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>

#include <fbl/macros.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

// A file node backed by a range of bytes in a VMO.
//
// The file has a fixed size specified at creating time; it does not grow or shrink even when
// written into.
//
// This class is thread-safe.
class VmoFile : public Vnode {
 public:
  // Construct with fbl::MakeRefCounted.

  // Specifies the default behavior when a client asks for the file's underlying VMO, but does not
  // specify if a duplicate handle or copy-on-write clone is required.
  //
  // *NOTE*: This does not affect the behavior of requests that specify the required sharing mode,
  // those requests will still be fulfilled.
  enum class DefaultSharingMode : uint8_t {
    // NOT_SUPPORTED will be returned, unless a sharing mode is specified in the request.
    kNone,

    // The VMO handle is duplicated for each client.
    //
    // This is appropriate when it is okay for clients to access the entire
    // contents of the VMO, possibly extending beyond the pages spanned by the
    // file.
    //
    // This mode is significantly more efficient than |CLONE_COW| and should be
    // preferred when file spans the whole VMO or when the VMO's entire content
    // is safe for clients to read.
    //
    // As size changes are currently untracked, all handles given out in this
    // mode will lack ZX_RIGHT_WRITE and ZX_RIGHT_SET_PROPERTY.
    kDuplicate,

    // The VMO range spanned by the file is cloned on demand, using
    // copy-on-write semantics to isolate modifications of clients which open
    // the file in a writable mode.
    //
    // This is appropriate when clients need to be restricted from accessing
    // portions of the VMO outside of the range of the file and when file
    // modifications by clients should not be visible to each other.
    kCloneCow,
  };

  // The underlying VMO handle.
  const zx::vmo& vmo() const { return vmo_; }

  // The length of the file in bytes.
  size_t length() const { return length_; }

  // True if the file is writable.
  // If false, attempts to open the file for write will fail.
  bool is_writable() const { return writable_; }

  // |Vnode| implementation:
  fuchsia_io::NodeProtocolKinds GetProtocols() const final;
  bool ValidateRights(fuchsia_io::Rights rights) const final;
  zx::result<fs::VnodeAttributes> GetAttributes() const final;
  zx_status_t Read(void* data, size_t length, size_t offset, size_t* out_actual) final;
  zx_status_t Write(const void* data, size_t length, size_t offset, size_t* out_actual) final;
  zx_status_t GetVmo(fuchsia_io::wire::VmoFlags flags, zx::vmo* out_vmo) final;

 protected:
  friend fbl::internal::MakeRefCountedHelper<VmoFile>;
  friend fbl::RefPtr<VmoFile>;

  // Creates a file node backed by a VMO.
  VmoFile(zx::vmo vmo, size_t length, bool writable = false,
          DefaultSharingMode vmo_sharing = DefaultSharingMode::kDuplicate);

  ~VmoFile() override;

 private:
  zx::vmo vmo_;
  size_t const length_;
  bool const writable_;
  DefaultSharingMode const vmo_sharing_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmoFile);
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_VMO_FILE_H_
