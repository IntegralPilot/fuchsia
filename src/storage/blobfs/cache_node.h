// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_CACHE_NODE_H_
#define SRC_STORAGE_BLOBFS_CACHE_NODE_H_

#ifndef __Fuchsia__
#error Fuchsia-only Header
#endif

#include <fbl/intrusive_wavl_tree.h>
#include <fbl/recycler.h>

#include "src/storage/blobfs/format.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"
#include "src/storage/lib/vfs/cpp/paged_vnode.h"

namespace blobfs {

// Forward declared because CacheNode needs a mechanism for accessing the class when it runs out of
// strong references.
class BlobCache;

// An abstract blob-backed Vnode, which is managed by the BlobCache.
class CacheNode : public fs::PagedVnode,
                  private fbl::Recyclable<CacheNode>,
                  public fbl::WAVLTreeContainable<CacheNode*> {
 public:
  explicit CacheNode(fs::PagedVfs& vfs, Digest digest);
  virtual ~CacheNode() = default;

  const Digest& digest() const { return digest_; }

  // Required for memory management, see the class comment above Vnode for more.
  void fbl_recycle() { RecycleNode(); }

  // Returns a reference to the BlobCache.
  //
  // The BlobCache must outlive all CacheNodes; this method is invoked from the recycler of a
  // CacheNode.
  //
  // The implementation of this method must not invoke any other CacheNode methods. The
  // implementation of this method must not attempt to acquire a reference to |this|.
  virtual BlobCache& GetCache() = 0;

  // Identifies if the node should be recycled when it is terminated, keeping it cached (although
  // possibly in a reduced state).
  //
  // This should be true as long as the blob exists on persistent storage, and would be visible
  // again on reboot.
  //
  // The implementation of this method must not invoke any other CacheNode methods. The
  // implementation of this method must not attempt to acquire a reference to |this|.
  virtual bool ShouldCache() const = 0;

 protected:
  // Vnode memory management function called when the reference count reaches 0.
  void RecycleNode() override;

 private:
  Digest digest_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_CACHE_NODE_H_
