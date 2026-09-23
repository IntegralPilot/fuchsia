// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VFS_CPP_LAZY_DIR_H_
#define LIB_VFS_CPP_LAZY_DIR_H_

#include <lib/vfs/cpp/node.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <mutex>
#include <string>

namespace vfs {

// `LazyDir` is an abstract base class for directories that dynamically update their contents.
// Clients should derive from this class and implement GetContents and GetFile for their use case.
//
// The base implementation of this class is thread-safe, but it is up to implementers to ensure
// their implementations are thread safe as well.
//
// Due to lifetime restrictions, implementations of `LazyDir` cannot call `Serve()`. However, the
// `LazyDir` node can be added as a child entry of a `vfs::PseudoDir` which will ensure the lifetime
// requirements.
class LazyDir : public vfs::Node {
 public:
  LazyDir() : Node(MakeLazyDir(this)) {}

  // Structure storing a single entry in the directory.
  struct LazyEntry {
    // Non-zero ID for this entry, must remain stable across calls. IDs do not necessarily need to
    // be unique, however, non-unique IDs may cause duplication in directory listings.
    uint64_t id;
    std::string name;
    uint32_t type;

    bool operator<(const LazyEntry& rhs) const;
  };
  using LazyEntryVector = std::vector<LazyEntry>;

 protected:
  // Returns the contents of the directory as an output vector.
  virtual void GetContents(std::vector<LazyEntry>* out_vector) const = 0;

  // Returns a pointer to an entry during lookup. The ID for the entry matching `name`, as returned
  // by `GetContents()`, is passed as `id` in to assist locating the file.
  virtual zx_status_t GetFile(Node** out_node, uint64_t id, std::string name) const = 0;

  // IDs returned by `GetContents()` should be greater than or equal to this value.
  static constexpr uint64_t GetStartingId() { return kDotId; }

 private:
  static vfs_internal_node_t* MakeLazyDir(LazyDir* self) {
    // *WARNING*: `self` is not fully constructed at this point, so these callbacks are not safe
    // to invoke yet. The function to create the underlying node only copies the pointers and only
    // allows invoking them once the object is in a usable state.
    vfs_internal_lazy_dir_context context{
        .cookie = self,
        .get_contents = &GetContentsCallback,
        .get_entry = &GetEntryCallback,
    };
    vfs_internal_node_t* lazy_dir;
    ZX_ASSERT(vfs_internal_lazy_dir_create(&context, &lazy_dir) == ZX_OK);
    return lazy_dir;
  }

  static void GetContentsCallback(void* self, vfs_internal_lazy_entry** entries_out,
                                  size_t* len_out) __TA_EXCLUDES(mutex_) {
    LazyDir* lazy_dir = static_cast<LazyDir*>(self);
    std::lock_guard guard(lazy_dir->mutex_);
    lazy_dir->RefreshEntries();
    *entries_out = lazy_dir->entries_internal_.data();
    *len_out = lazy_dir->entries_internal_.size();
  }

  static zx_status_t GetEntryCallback(void* self, vfs_internal_node_t** node_out, uint64_t id,
                                      const char* name) {
    LazyDir* lazy_dir = static_cast<LazyDir*>(self);
    Node* node;
    if (zx_status_t status = lazy_dir->GetFile(&node, id, std::string(name)); status != ZX_OK) {
      return status;
    }
    *node_out = node->handle();
    return ZX_OK;
  }

  void RefreshEntries() __TA_REQUIRES(mutex_) {
    entries_ = {};
    entries_internal_ = {};
    GetContents(&entries_);
    for (const auto& entry : entries_) {
      entries_internal_.push_back(vfs_internal_lazy_entry_t{
          .id = entry.id,
          .name = entry.name.c_str(),
          .type = entry.type,
      });
    }
  }

  static constexpr uint64_t kDotId = 1u;
  std::mutex mutex_;
  std::vector<LazyEntry> entries_ __TA_GUARDED(mutex_);  // To keep memory of entry names alive.
  std::vector<vfs_internal_lazy_entry_t> entries_internal_
      __TA_GUARDED(mutex_);  // Pointers to above entries.
};

}  // namespace vfs

#endif  // LIB_VFS_CPP_LAZY_DIR_H_
