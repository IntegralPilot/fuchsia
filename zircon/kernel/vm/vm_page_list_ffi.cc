// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/vm_page_list_ffi.h"

#include <lib/btree.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <ktl/memory.h>
#include <ktl/utility.h>
#include <vm/vm_page_list.h>

static_assert(sizeof(VmPageListNode) == 64);
static_assert(alignof(VmPageListNode) == 4);

namespace {

template <typename EntryType, typename Iterator>
inline EntryType unwrap_entry(const Iterator& iter) {
  if (!iter.IsValid()) {
    return {0, nullptr};
  }
  auto [offset, node] = *iter;
  return {offset, node};
}

}  // namespace

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void cpp_vm_page_splice_list_construct(VmPageSpliceList* list) {
  ktl::construct_at(list);
}

FFI_ALWAYS_INLINE void cpp_vm_page_splice_list_destroy(VmPageSpliceList* list) {
  ktl::destroy_at(list);
}

FFI_ALWAYS_INLINE bool cpp_vm_page_splice_list_is_processed(const VmPageSpliceList* list) {
  return list->IsProcessed();
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_init(ffi::Uninitialized<VmPageListBtree>* tree) {
  tree->Initialize();
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_destroy(VmPageListBtree* tree) {
  ktl::destroy_at(tree);
}

FFI_ALWAYS_INLINE bool cpp_vm_page_list_btree_is_empty(const VmPageListBtree* tree) {
  return tree->is_empty();
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_clear(VmPageListBtree* tree) { tree->clear(); }

FFI_ALWAYS_INLINE void* cpp_vm_page_list_btree_find(VmPageListBtree* tree, uint64_t node_offset,
                                                    VmPageListBtreeCursor* out_cursor) {
  auto pln = tree->find(node_offset);
  if (!pln.IsValid()) {
    return nullptr;
  }
  if (out_cursor) {
    out_cursor->iter = pln;
  }
  return (*pln).second;
}

FFI_ALWAYS_INLINE const void* cpp_vm_page_list_btree_find_const(const VmPageListBtree* tree,
                                                                uint64_t node_offset) {
  auto pln = tree->find(node_offset);
  if (!pln.IsValid()) {
    return nullptr;
  }
  return (*pln).second;
}

FFI_ALWAYS_INLINE VmPageListBtreeNodeEntry cpp_vm_page_list_btree_lower_bound(
    VmPageListBtree* tree, uint64_t node_offset, VmPageListBtreeCursor* out_cursor) {
  auto iter = tree->lower_bound(node_offset);
  if (out_cursor) {
    out_cursor->iter = iter;
  }
  return unwrap_entry<VmPageListBtreeNodeEntry>(iter);
}

FFI_ALWAYS_INLINE VmPageListBtreeNodeEntry
cpp_vm_page_list_btree_cursor_get(const VmPageListBtreeCursor* cursor) {
  return unwrap_entry<VmPageListBtreeNodeEntry>(cursor->iter);
}

FFI_ALWAYS_INLINE void* cpp_vm_page_list_btree_insert(VmPageListBtree* tree, uint64_t node_offset,
                                                      VmPageListBtreeCursor* cursor) {
  VmPlnOwner pl = VmPageListNode::Create();
  if (!pl) {
    return nullptr;
  }
  VmPageListNode* raw_ptr = pl.get();
  auto hint = cursor ? cursor->iter : VmPageListBtree::iterator{};
  auto iter = tree->insert(hint, node_offset, ktl::move(pl));
  if (!iter.IsValid()) {
    return nullptr;
  }
  if (cursor) {
    cursor->iter = iter;
  }
  return raw_ptr;
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_erase_at(VmPageListBtree* tree,
                                                       VmPageListBtreeCursor* cursor) {
  DEBUG_ASSERT(cursor->iter.IsValid());
  cursor->iter = tree->erase(cursor->iter);
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_cursor_default_init(VmPageListBtreeCursor* cursor) {
  cursor->iter = {};
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_cursor_init(VmPageListBtreeCursor* cursor,
                                                          VmPageListBtree* tree) {
  cursor->iter = tree->begin();
}

FFI_ALWAYS_INLINE VmPageListBtreeNodeEntry
cpp_vm_page_list_btree_cursor_next(VmPageListBtreeCursor* cursor) {
  auto entry = unwrap_entry<VmPageListBtreeNodeEntry>(cursor->iter);
  if (entry.node) {
    ++cursor->iter;
  }
  return entry;
}

FFI_ALWAYS_INLINE VmPageListBtreeNodeEntry
cpp_vm_page_list_btree_cursor_prev(VmPageListBtreeCursor* cursor) {
  --cursor->iter;
  return unwrap_entry<VmPageListBtreeNodeEntry>(cursor->iter);
}

FFI_ALWAYS_INLINE void cpp_vm_page_list_btree_const_cursor_init(VmPageListBtreeConstCursor* cursor,
                                                                const VmPageListBtree* tree) {
  cursor->iter = tree->begin();
}

FFI_ALWAYS_INLINE VmPageListBtreeConstNodeEntry
cpp_vm_page_list_btree_const_cursor_next(VmPageListBtreeConstCursor* cursor) {
  auto entry = unwrap_entry<VmPageListBtreeConstNodeEntry>(cursor->iter);
  if (entry.node) {
    ++cursor->iter;
  }
  return entry;
}

}  // extern "C"
