// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/memfs/dnode.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstring>
#include <memory>
#include <string>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"
#include "src/storage/memfs/vnode.h"

namespace memfs {

// Create a new dnode and attach it to a vnode
zx::result<std::unique_ptr<Dnode>> Dnode::Create(std::string name, fbl::RefPtr<Vnode> vn) {
  if ((name.length() > kDnodeNameMax) || name.empty()) {
    return zx::error(ZX_ERR_BAD_PATH);
  }

  return zx::ok(std::unique_ptr<Dnode>(new Dnode(std::move(vn), std::move(name))));
}

std::unique_ptr<Dnode> Dnode::RemoveFromParent() {
  ZX_DEBUG_ASSERT(vnode_ != nullptr);

  std::unique_ptr<Dnode> node;
  // Detach from parent
  if (parent_) {
    node = parent_->children_.erase(*this);
    if (IsDirectory()) {
      // '..' no longer references parent.
      parent_->vnode_->link_count_--;
    }
    parent_->vnode_->UpdateModified();
    parent_ = nullptr;
    vnode_->link_count_--;
  }
  return node;
}

void Dnode::Detach() {
  ZX_DEBUG_ASSERT(children_.is_empty());
  if (vnode_ == nullptr) {  // Dnode already detached.
    return;
  }

  auto self = RemoveFromParent();
  // Detach from vnode
  self->vnode_->dnode_ = nullptr;
  self->vnode_ = nullptr;
}

void Dnode::AddChild(Dnode* parent, std::unique_ptr<Dnode> child) {
  ZX_DEBUG_ASSERT(parent != nullptr);
  ZX_DEBUG_ASSERT(child != nullptr);
  ZX_DEBUG_ASSERT(child->parent_ == nullptr);  // Child shouldn't have a parent
  ZX_DEBUG_ASSERT(child.get() != parent);
  ZX_DEBUG_ASSERT(parent->IsDirectory());

  child->parent_ = parent;
  child->vnode_->link_count_++;
  if (child->IsDirectory()) {
    // Child has '..' pointing back at parent.
    parent->vnode_->link_count_++;
  }
  // Ensure that the ordering of tokens in the children list is absolute.
  if (parent->children_.is_empty()) {
    child->ordering_token_ = 2;  // '0' for '.', '1' for '..'
  } else {
    child->ordering_token_ = parent->children_.back().ordering_token_ + 1;
  }
  parent->children_.push_back(std::move(child));
  parent->vnode_->UpdateModified();
}

Dnode* Dnode::Lookup(std::string_view name) {
  auto dn = children_.find_if([&name](const Dnode& elem) -> bool { return elem.name_ == name; });
  if (dn == children_.end()) {
    return nullptr;
  }
  return &(*dn);
}

fbl::RefPtr<Vnode> Dnode::AcquireVnode() const { return vnode_; }

Dnode* Dnode::GetParent() const { return parent_; }

zx_status_t Dnode::CanUnlink() const {
  if (!children_.is_empty()) {
    // Cannot unlink non-empty directory
    return ZX_ERR_NOT_EMPTY;
  }
  if (vnode_->IsRemote()) {
    // Cannot unlink mount points
    return ZX_ERR_UNAVAILABLE;
  }
  return ZX_OK;
}

struct dircookie_t {
  size_t order;  // Minimum 'order' of the next dnode dirent to be read.
};

static_assert(sizeof(dircookie_t) <= sizeof(fs::VdirCookie),
              "MemFS dircookie too large to fit in IO state");

zx_status_t Dnode::Readdir(fs::DirentFiller& df, void* cookie) const {
  dircookie_t* c = static_cast<dircookie_t*>(cookie);

  if (c->order == 0) {
    if (!df.Next(".", fuchsia_io::DirentType::kDirectory, vnode_->ino())) {
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    c->order = 1;
  }

  for (const auto& dn : children_) {
    if (dn.ordering_token_ < c->order) {
      continue;
    }
    fuchsia_io::DirentType type =
        dn.IsDirectory() ? fuchsia_io::DirentType::kDirectory : fuchsia_io::DirentType::kFile;
    if (!df.Next(dn.name_, type, dn.AcquireVnode()->ino())) {
      return df.BytesFilled() == 0 ? ZX_ERR_BUFFER_TOO_SMALL : ZX_OK;
    }
    c->order = dn.ordering_token_ + 1;
  }
  return ZX_OK;
}

// Answers the question: "Is dn a subdirectory of this?"
bool Dnode::IsSubdirectory(const Dnode* dn) const {
  if (IsDirectory() && dn->IsDirectory()) {
    // Iterate all the way up to root
    while (dn->parent_ != nullptr && dn->parent_ != dn) {
      if (vnode_ == dn->vnode_) {
        return true;
      }
      dn = dn->parent_;
    }
  }
  return false;
}

std::string Dnode::TakeName() {
  std::string name = std::move(name_);
  name_.clear();
  return name;
}

void Dnode::PutName(std::string name) { name_ = std::move(name); }

bool Dnode::IsDirectory() const { return vnode_->IsDirectory(); }

Dnode::Dnode(fbl::RefPtr<Vnode> vn, std::string name)
    : vnode_(std::move(vn)), parent_(nullptr), ordering_token_(0), name_(std::move(name)) {}

Dnode::~Dnode() = default;

}  // namespace memfs
