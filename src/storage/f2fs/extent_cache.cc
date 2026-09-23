// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/extent_cache.h"

#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

namespace {
static bool IsOverlap(const ExtentInfo &x, const ExtentInfo &y) {
  return (x.fofs < y.fofs + y.len && y.fofs < x.fofs + x.len);
}

static bool IsMergeable(const ExtentInfo &front, const ExtentInfo &back) {
  return (front.fofs + front.len == back.fofs && front.blk_addr + front.len == back.blk_addr);
}

static bool IsFrontSplitable(const ExtentInfo &front, const ExtentInfo &back) {
  return (front.fofs < back.fofs && front.fofs + front.len > back.fofs);
}

static bool IsBackSplitable(const ExtentInfo &front, const ExtentInfo &back) {
  return (back.fofs < front.fofs + front.len && back.fofs + back.len > front.fofs + front.len);
}
}  // namespace

zx::result<> ExtentTree::InsertExtent(ExtentInfo target_extent_info) {
  std::lock_guard lock(tree_lock_);

  // If it overlaps with |largest_extent_info_|, we just invalidate the existing
  // |largest_extent_info_|.
  if (largest_extent_info_.has_value() &&
      IsOverlap(largest_extent_info_.value(), target_extent_info)) {
    largest_extent_info_ = std::nullopt;
  }

  std::vector<ExtentInfo> new_extents;
  auto current = extent_node_tree_.lower_bound(target_extent_info.fofs);
  if (current != extent_node_tree_.begin()) {
    --current;
  }

  while (current != extent_node_tree_.end()) {
    auto next = current;
    ++next;

    if (IsOverlap(current->GetExtentInfo(), target_extent_info) ||
        IsMergeable(current->GetExtentInfo(), target_extent_info) ||
        IsMergeable(target_extent_info, current->GetExtentInfo())) {
      ZX_DEBUG_ASSERT(current->InContainer());
      auto erased = extent_node_tree_.erase(current);

      std::optional<ExtentInfo> front_splitted = std::nullopt;
      std::optional<ExtentInfo> back_splitted = std::nullopt;

      // Front split
      if (IsFrontSplitable(erased->GetExtentInfo(), target_extent_info)) {
        front_splitted = erased->GetExtentInfo();
        front_splitted->len =
            safemath::checked_cast<uint32_t>(target_extent_info.fofs - front_splitted->fofs);

        if (front_splitted->len >= kMinExtentLen) {
          new_extents.push_back(front_splitted.value());
        }
      }

      // Back split
      if (IsBackSplitable(target_extent_info, erased->GetExtentInfo())) {
        back_splitted = erased->GetExtentInfo();
        pgoff_t diff = target_extent_info.fofs + target_extent_info.len - back_splitted->fofs;

        back_splitted->fofs += diff;
        back_splitted->blk_addr += diff;
        back_splitted->len -= diff;

        if (back_splitted->len >= kMinExtentLen) {
          new_extents.push_back(back_splitted.value());
        }
      }

      // Front merge
      if (front_splitted.has_value() && IsMergeable(front_splitted.value(), target_extent_info)) {
        target_extent_info.fofs = front_splitted->fofs;
        target_extent_info.blk_addr = front_splitted->blk_addr;
        target_extent_info.len += front_splitted->len;
      } else if (IsMergeable(erased->GetExtentInfo(), target_extent_info)) {
        target_extent_info.fofs = erased->GetExtentInfo().fofs;
        target_extent_info.blk_addr = erased->GetExtentInfo().blk_addr;
        target_extent_info.len += erased->GetExtentInfo().len;
      }

      // Back merge
      if (back_splitted.has_value() && IsMergeable(target_extent_info, back_splitted.value())) {
        target_extent_info.len += back_splitted->len;
      } else if (IsMergeable(target_extent_info, current->GetExtentInfo())) {
        target_extent_info.len += erased->GetExtentInfo().len;
      }
    } else if (target_extent_info.fofs + target_extent_info.len < current->GetExtentInfo().fofs) {
      break;
    }

    current = next;
  }

  if (target_extent_info.blk_addr != kNullAddr) {
    new_extents.push_back(target_extent_info);
  }

  for (const auto &extent_info : new_extents) {
    if (!largest_extent_info_.has_value() || extent_info.len >= largest_extent_info_->len) {
      largest_extent_info_ = extent_info;
    }

    extent_node_tree_.insert(std::make_unique<ExtentNode>(extent_info));
  }

  return zx::ok();
}

zx::result<ExtentInfo> ExtentTree::LookupExtent(pgoff_t file_offset) {
  fs::SharedLock tree_lock(tree_lock_);
  auto it = extent_node_tree_.upper_bound(file_offset);
  if (it == extent_node_tree_.begin()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  --it;
  ZX_ASSERT(it->GetExtentInfo().fofs <= file_offset);
  if (it->GetExtentInfo().fofs + it->GetExtentInfo().len <= file_offset) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  return zx::ok(it->GetExtentInfo());
}

ExtentInfo ExtentTree::GetLargestExtent() {
  fs::SharedLock lock(tree_lock_);
  if (!largest_extent_info_.has_value()) {
    return ExtentInfo{};
  }
  return largest_extent_info_.value();
}

void ExtentTree::Reset() {
  std::lock_guard lock(tree_lock_);

  auto current = extent_node_tree_.begin();
  while (current != extent_node_tree_.end()) {
    auto next = current;
    ++next;

    ZX_DEBUG_ASSERT(current->InContainer());
    extent_node_tree_.erase(current);
    current = next;
  }

  ZX_DEBUG_ASSERT(extent_node_tree_.is_empty());
}

}  // namespace f2fs
