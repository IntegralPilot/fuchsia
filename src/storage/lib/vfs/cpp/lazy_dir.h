// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_LAZY_DIR_H_
#define SRC_STORAGE_LIB_VFS_CPP_LAZY_DIR_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <string_view>

#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/vector.h>

#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

// A |LazyDir| a base class for directories that dynamically update their contents on each
// operation.  Clients should derive from this class and implement GetContents and GetFile for their
// use case.  The base implementation of this class is thread-safe, but it is up to implementers to
// ensure their implementations are thread safe as well.
class LazyDir : public Vnode {
 public:
  // Construct with fbl::MakeRefCounted.

  // Structure storing a single entry in the directory.
  struct LazyEntry {
    /// Non-zero ID for this entry, must remain stable across calls. IDs do not necessarily need to
    /// be unique, however, non-unique IDs may cause duplication in directory listings. Where an ID
    /// is not available, use the INO_UNKNOWN constant from fuchsia.io.
    uint64_t id;
    /// Name for this entry.
    fbl::String name;
    /// The POSIX stat mode type for this entry (e.g. S_IFREG).
    uint32_t type;
  };
  using LazyEntryVector = fbl::Vector<LazyEntry>;

  // |Vnode| implementation.
  fuchsia_io::NodeProtocolKinds GetProtocols() const final;
  // Read the directory contents. Note that cookie->p is used to denote if the "." entry has been
  // returned. All IDs other than 0 are valid.
  zx_status_t Readdir(VdirCookie* cookie, void* dirents, size_t len, size_t* out_actual) final;
  zx_status_t Lookup(std::string_view name, fbl::RefPtr<fs::Vnode>* out_vnode) final;

 protected:
  friend fbl::internal::MakeRefCountedHelper<LazyDir>;
  friend fbl::RefPtr<LazyDir>;

  LazyDir();
  ~LazyDir() override;

  // Get the contents of the directory in an output vector.
  virtual void GetContents(LazyEntryVector* out_vector) = 0;
  // Get the reference to a single file. The id and name of the entry as returned from GetContents
  // are passed in to assist locating the file.
  virtual zx_status_t GetFile(fbl::RefPtr<Vnode>* out_vnode, uint64_t id, fbl::String name) = 0;
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_LAZY_DIR_H_
