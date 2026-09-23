// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_MEMFS_DNODE_H_
#define SRC_STORAGE_MEMFS_DNODE_H_

#include <lib/zx/result.h>
#include <limits.h>
#include <zircon/types.h>

#include <cstddef>
#include <memory>
#include <string>
#include <string_view>

#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vnode.h"

namespace memfs {

class Vnode;

constexpr size_t kDnodeNameMax = NAME_MAX;
static_assert(NAME_MAX == 255, "NAME_MAX must be 255");

// The named portion of a node, representing the named hierarchy.
//
// Dnodes always have one corresponding Vnode (a name represents one vnode).
// Vnodes may be represented by multiple Dnodes (a vnode may have many names).
//
// Dnodes are owned by their parents.
class Dnode : public fbl::DoublyLinkedListable<std::unique_ptr<Dnode>> {
 public:
  DISALLOW_COPY_ASSIGN_AND_MOVE(Dnode);

  // Allocates a dnode, attached to a vnode
  static zx::result<std::unique_ptr<Dnode>> Create(std::string name, fbl::RefPtr<Vnode> vn);

  // Takes a parent-less node and makes it a child of the parent node.
  //
  // Increments child link count by one.
  // If the child is a directory, increments the parent link count by one.
  static void AddChild(Dnode* parent, std::unique_ptr<Dnode> child);

  ~Dnode();

  // Removes a dnode from its parent (if dnode has a parent)
  // Decrements parent link count by one.
  std::unique_ptr<Dnode> RemoveFromParent();

  // Detaches a dnode from its parent / vnode.
  // Decrements dn->vnode link count by one (if it exists).
  //
  // Precondition: Dnode has no children.
  // Postcondition: "this" may be destroyed.
  void Detach();

  bool HasChildren() const { return !children_.is_empty(); }

  // Look up the child dnode (within a parent directory) by name.
  // Returns nullptr if the child is not found.
  Dnode* Lookup(std::string_view name);

  // Acquire a pointer to the vnode underneath this dnode.
  // Acquires a reference to the underlying vnode.
  fbl::RefPtr<Vnode> AcquireVnode() const;

  // Get a pointer to the parent Dnode. If current Dnode is root, return nullptr.
  Dnode* GetParent() const;

  // Returns ZX_OK if the dnode may be unlinked
  zx_status_t CanUnlink() const;

  // Populates df with this directory's entries.
  zx_status_t Readdir(fs::DirentFiller& df, void* cookie) const;

  // Answers the question: "Is dn a subdirectory of this?"
  bool IsSubdirectory(const Dnode* dn) const;

  // Functions to take / steal the allocated dnode name.
  std::string TakeName();
  void PutName(std::string name);

  bool IsDirectory() const;

 private:
  friend struct TypeChildTraits;

  Dnode(fbl::RefPtr<Vnode> vn, std::string name);

  fbl::RefPtr<Vnode> vnode_;
  // Refers to the parent named node in the directory hierarchy.
  // A weak reference is used here to avoid a circular dependency, where
  // parents own children, but children point to their parents.
  Dnode* parent_;
  // Used to impose an absolute order on dnodes within a directory.
  size_t ordering_token_;
  fbl::DoublyLinkedList<std::unique_ptr<Dnode>> children_;
  std::string name_;
};

}  // namespace memfs

#endif  // SRC_STORAGE_MEMFS_DNODE_H_
