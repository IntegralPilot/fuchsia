// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "node_properties.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/zx/event.h>
#include <lib/zx/eventpair.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <vector>

#include "koid_util.h"
#include "logical_buffer_collection.h"
#include "node.h"
#include "utils.h"

namespace sysmem_service {

namespace {

const size_t kMaxLifetimeServerEnds = 256;

}  // namespace

NodeProperties::~NodeProperties() {
  ZX_DEBUG_ASSERT(child_count() == 0);
  if (node_) {
    logical_buffer_collection_->RemoveCountsForNode(*node_);
  }
  if (!is_weak_) {
    logical_buffer_collection_->DecStrongNodeTally();
  }
  logical_buffer_collection_->UntrackNodeProperties(this);
}

// static
std::unique_ptr<NodeProperties> NodeProperties::NewRoot(
    LogicalBufferCollection* logical_buffer_collection, const ClientDebugInfo* client_debug_info) {
  auto result = std::unique_ptr<NodeProperties>(new NodeProperties(logical_buffer_collection));
  ZX_DEBUG_ASSERT(!result->parent_);
  // Set later with SetNode().
  ZX_DEBUG_ASSERT(!result->node_);
  ZX_DEBUG_ASSERT(result->children_.empty());
  result->logical_buffer_collection_->IncStrongNodeTally();
  if (client_debug_info) {
    auto& new_debug_info = result->client_debug_info();
    new_debug_info.name = client_debug_info->name;
    new_debug_info.id = client_debug_info->id;
  }
  return result;
}

NodeProperties* NodeProperties::NewChild(LogicalBufferCollection* logical_buffer_collection) {
  auto result = std::unique_ptr<NodeProperties>(new NodeProperties(logical_buffer_collection));
  auto result_ptr = result.get();
  // The parent Node owns the child Node.
  this->children_.emplace_back(std::move(result));
  result_ptr->parent_ = this;
  // Set later with SetNode().
  ZX_DEBUG_ASSERT(!result_ptr->node_);
  ZX_DEBUG_ASSERT(result_ptr->children_.empty());
  // Default to parent's debug info until/unless overriden later (by the client, or by later code
  // that always runs regardless of client behavior).
  //
  // struct copy
  result_ptr->client_debug_info_ = client_debug_info_;
  // Soon we'll do another &= on this mask with the rights_attenuation_mask supplied by the client
  // when creating the child, but the child starts with the same rights masked away as the parent.
  result_ptr->rights_attenuation_mask_ = rights_attenuation_mask_;
  ZX_DEBUG_ASSERT(result_ptr->error_propagation_mode_ == ErrorPropagationMode::kPropagate);

  // The is_weak must propagate when the child is created; later changes to parent is_weak_ don't
  // change the child's is_weak_, intentionally.
  result_ptr->is_weak_ = is_weak_;
  if (!is_weak_) {
    result_ptr->logical_buffer_collection_->IncStrongNodeTally();
  }

  // is_weak_ok_ doesn't propagate to children by default
  if (is_weak_ok_for_child_nodes_also_) {
    ZX_DEBUG_ASSERT(is_weak_ok_);
    // propagation to all descendents of the child as well
    result_ptr->is_weak_ok_for_child_nodes_also_ = true;
    result_ptr->is_weak_ok_ = is_weak_ok_;
    result_ptr->is_weak_ok_from_parent_ = true;
  }

  return result_ptr;
}

// static
std::unique_ptr<NodeProperties> NodeProperties::NewTemporary(
    LogicalBufferCollection* logical_buffer_collection,
    fuchsia_sysmem2::BufferCollectionConstraints buffer_collection_constraints,
    std::string debug_name) {
  auto result = std::unique_ptr<NodeProperties>(new NodeProperties(logical_buffer_collection));
  ZX_DEBUG_ASSERT(!result->parent_);
  // Since temporary, won't ever have a node_.
  ZX_DEBUG_ASSERT(!result->node_);
  ZX_DEBUG_ASSERT(result->children_.empty());
  result->SetBufferCollectionConstraints(std::move(buffer_collection_constraints));
  result->client_debug_info().name = debug_name;
  // just to avoid subtracting strong node tally in destructor
  result->is_weak_ = true;
  return result;
}

void NodeProperties::RemoveFromTreeAndDelete() {
  ZX_DEBUG_ASSERT(child_count() == 0);
  // This also deletes "this".
  if (parent_) {
    // As we keep a shared_ptr of `this` in the scope, `parent_` is guaranteed
    // to be valid till the end of the block.
    //
    // It could be tempting to detach `this` from `parent_` here. However,
    // `parent_` of the NodeProperties must not be changed until it's destroyed
    // because it is used to propagate the node counters on the tree on
    // NodeProperties destructor.
    std::shared_ptr<NodeProperties> shared_this = this->shared_from_this();
    Children::iterator iter;
    // typically called to remove last child, so search from end of vector
    for (iter = parent_->children_.end() - 1;; --iter) {
      if (*iter == shared_this) {
        break;
      }
      ZX_DEBUG_ASSERT(iter != parent_->children_.begin());
    }
    parent_->children_.erase(iter);
  } else {
    logical_buffer_collection_->DeleteRoot();
  }
  // "this" is now gone
}

std::vector<NodeProperties*> NodeProperties::BreadthFirstOrder(
    fit::function<NodeFilterResult(const NodeProperties&)> node_filter) {
  std::vector<NodeProperties*> iterate_children_tmp;
  std::vector<NodeProperties*> result;
  NodeFilterResult this_result;
  if (node_filter) {
    this_result = node_filter(*this);
  }
  if (this_result.keep_node) {
    result.push_back(this);
  }
  if (this_result.iterate_children) {
    iterate_children_tmp.push_back(this);
  }
  for (uint32_t i = 0; i < iterate_children_tmp.size(); ++i) {
    NodeProperties* node = iterate_children_tmp[i];
    for (auto& child_shared : node->children_) {
      NodeFilterResult child_result;
      if (node_filter) {
        child_result = node_filter(*child_shared);
      }
      if (child_result.keep_node) {
        result.push_back(child_shared.get());
      }
      if (child_result.iterate_children) {
        iterate_children_tmp.push_back(child_shared.get());
      }
    }
  }
  return result;
}

std::vector<NodeProperties*> NodeProperties::DepthFirstPreOrder(
    fit::function<NodeFilterResult(const NodeProperties&)> node_filter) {
  std::vector<NodeProperties*> result;
  struct StackLevel {
    NodeProperties* node_properties = nullptr;
    uint32_t next_child = 0;
    NodeFilterResult filter_result = {};
  };
  // This vector used as a stack will be on the heap, so avoids stack overflow of the thread stack.
  std::vector<StackLevel> stack;
  stack.emplace_back(
      StackLevel{.node_properties = this, .next_child = 0, .filter_result = node_filter(*this)});
  while (!stack.empty()) {
    auto& cur = stack.back();
    if (cur.next_child == 0 && cur.filter_result.keep_node) {
      result.emplace_back(cur.node_properties);
    }
    if (!cur.filter_result.iterate_children) {
      stack.pop_back();
      continue;
    }
    if (cur.next_child == cur.node_properties->child_count()) {
      stack.pop_back();
      continue;
    }
    auto* child = cur.node_properties->children_[cur.next_child].get();
    ++cur.next_child;
    stack.push_back(StackLevel{
        .node_properties = child, .next_child = 0, .filter_result = node_filter(*child)});
  }
  return result;
}

NodeProperties* NodeProperties::parent() const {
  // Can be nullptr if this is the root.
  return parent_;
}

Node* NodeProperties::node() const {
  // Can be nullptr if no Node is owned yet.
  return node_.get();
}

uint32_t NodeProperties::child_count() const { return safe_cast<uint32_t>(children_.size()); }

NodeProperties& NodeProperties::child(uint32_t which) const { return *children_[which]; }

ClientDebugInfo& NodeProperties::client_debug_info() { return client_debug_info_; }

const ClientDebugInfo& NodeProperties::client_debug_info() const { return client_debug_info_; }

uint32_t& NodeProperties::rights_attenuation_mask() { return rights_attenuation_mask_; }

ErrorPropagationMode& NodeProperties::error_propagation_mode() { return error_propagation_mode_; }

const ErrorPropagationMode& NodeProperties::error_propagation_mode() const {
  return error_propagation_mode_;
}

bool NodeProperties::buffers_logically_allocated() const { return buffers_logically_allocated_; }

void NodeProperties::SetBuffersLogicallyAllocated() {
  ZX_DEBUG_ASSERT(!buffers_logically_allocated_);
  buffers_logically_allocated_ = true;
}

bool NodeProperties::has_constraints() const { return buffer_collection_constraints_.has_value(); }

const fuchsia_sysmem2::BufferCollectionConstraints* NodeProperties::buffer_collection_constraints()
    const {
  if (!buffer_collection_constraints_.has_value()) {
    return nullptr;
  }
  return &*buffer_collection_constraints_;
}

const std::optional<fuchsia_sysmem2::SingleBufferSettings>& NodeProperties::must_match_settings()
    const {
  return must_match_settings_;
}

bool NodeProperties::SetMustMatchVmo(zx::vmo must_match_vmo) {
  // We convert from zx::vmo to SingleBufferSettings here and drop must_match_vmo, to avoid keeping
  // must_match_vmo open any longer than necessary - also to avoid delaying any errors during
  // conversion. We could potentially move this code somewhere else, but the important thing is that
  // we convert to SingleBufferSettings and drop the must_match_vmo during V2 SetConstraints
  // handling.
  zx_info_handle_basic_t basic_info{};
  zx_status_t get_info_status = must_match_vmo.get_info(ZX_INFO_HANDLE_BASIC, &basic_info,
                                                        sizeof(basic_info), nullptr, nullptr);
  if (get_info_status != ZX_OK) {
    // No rights are required for ZX_INFO_HANDLE_BASIC, so this shouldn't happen.
    LogError(FROM_HERE, "must_match_vmo.get_info failed - protocol deviation");
    return false;
  }
  // Likely redundant with FIDL generated code; check anyway for now.
  if (basic_info.type != ZX_OBJ_TYPE_VMO) {
    LogError(FROM_HERE, "must_match_vmo requires VMO handle - protocol deviation");
    return false;
  }
  zx_koid_t vmo_koid = basic_info.koid;
  auto logical_buffer_result =
      logical_buffer_collection_->parent_sysmem_->FindLogicalBufferByVmoKoid(vmo_koid);
  if (!logical_buffer_result.logical_buffer) {
    // The client was supposed to pass a sysmem-provided VMO in must_match_vmo. If the client
    // needs to check first, see GetVmoInfo.
    LogError(FROM_HERE, "not a sysmem-provided VMO - protocol deviation");
    return false;
  }
  auto& logical_buffer = *logical_buffer_result.logical_buffer;
  // We know the VMO was handed out by sysmem, which also implies that the buffer is done
  // allocating. We wouldn't have found it above if somehow the buffer is no longer tracked by
  // sysmem, but that doesn't happen until after the sysmem-provided VMO is gone and we know that
  // hasn't happened yet since we know must_match_vmo exists here. That said, we use "value()"
  // instead of "*" on this line for easier diagnosis just in case. Currently, relevant parts of
  // sysmem run on the same thread, so no lock is currently needed here despite accessing a
  // different LogicalBufferCollection.
  auto& must_match_settings = logical_buffer.logical_buffer_collection()
                                  .buffer_collection_info_before_population_.value()
                                  .settings()
                                  .value();
  ZX_DEBUG_ASSERT(!must_match_settings_.has_value());
  // intentionally clone/copy must_match_settings
  must_match_settings_.emplace(must_match_settings);
  return true;
}

void NodeProperties::SetBufferCollectionConstraints(
    fuchsia_sysmem2::BufferCollectionConstraints buffer_collection_constraints) {
  ZX_DEBUG_ASSERT(!buffer_collection_constraints_.has_value());
  buffer_collection_constraints_.emplace(std::move(buffer_collection_constraints));
}

void NodeProperties::SetNode(fbl::RefPtr<Node> node) {
  // Once a Node is owned, it's ok to switch to a different Node, but not ok to set back to nullptr.
  ZX_DEBUG_ASSERT(node);
  logical_buffer_collection_->AddCountsForNode(*node);
  if (node_) {
    logical_buffer_collection_->RemoveCountsForNode(*node_);
  }

  // Remember the logical node type.  This intentionally does not check for OrphanedNode because
  // OrphanedNode does not correspond to a logical node type.  The logical node type persists after
  // potential replacement of the (non-OrphanedNode) Node by an OrphanedNode.
  if (!!node->buffer_collection_token()) {
    logical_node_type_ = NodeType::kToken;
  } else if (!!node->buffer_collection()) {
    logical_node_type_ = NodeType::kCollection;
  } else if (!!node->buffer_collection_token_group()) {
    logical_node_type_ = NodeType::kGroup;
  }

  node_ = std::move(node);
}

void NodeProperties::SetWhichChild(uint32_t which_child) {
  ZX_DEBUG_ASSERT(which_child < child_count());
  which_child_ = {which_child};
}

void NodeProperties::ResetWhichChild() { which_child_ = {std::nullopt}; }

std::optional<uint32_t> NodeProperties::which_child() const { return which_child_; }

bool NodeProperties::visible() const {
  // We could stop at the root of the pruned sub-tree of the current logical allocation, but for now
  // we just iterate to the root (in visibile true case).
  for (const auto* iter = this; iter; iter = iter->parent()) {
    if (!iter->parent()) {
      return true;
    }
    auto* parent = iter->parent();
    if (!parent->which_child().has_value()) {
      // If which_child() isn't set, then that means "all", which can't be hiding "this".
      continue;
    }
    ZX_DEBUG_ASSERT(*parent->which_child() < parent->child_count());
    if (parent->children_[*parent->which_child()].get() != iter) {
      return false;
    }
  }
  ZX_PANIC("impossible");
  return true;
}

void NodeProperties::SetWeak() {
  if (is_weak_) {
    return;
  }
  logical_buffer_collection_->DecStrongNodeTally();
  is_weak_ = true;
}

void NodeProperties::SetWeakOk(bool for_child_nodes_also) {
  is_weak_ok_ = true;
  if (for_child_nodes_also) {
    is_weak_ok_for_child_nodes_also_ = true;
  }
}

bool NodeProperties::AttachNodeTracking(zx::eventpair server_end) {
  ZX_DEBUG_ASSERT(lifetime_server_ends_.size() <= kMaxLifetimeServerEnds);
  if (lifetime_server_ends_.size() == kMaxLifetimeServerEnds) {
    return false;
  }
  lifetime_server_ends_.push_back(std::move(server_end));
  return true;
}

NodeProperties::NodeProperties(LogicalBufferCollection* logical_buffer_collection)
    : logical_buffer_collection_(logical_buffer_collection) {
  zx_status_t status = zx::event::create(0, &node_ref_);
  if (status != ZX_OK) {
    // Sysmem treats this much like a code page-in that fails due to out of memory.  Both will only
    // happen if we're so low on memory that we've already committed to OOMing (or at least, that's
    // the stated intent IIUC).
    ZX_PANIC("zx::eventpair::create() failed - status: %d", status);
  }
  zx_koid_t not_used;
  status = get_handle_koids(node_ref_, &node_ref_koid_, &not_used, ZX_OBJ_TYPE_EVENT);
  if (status != ZX_OK) {
    ZX_PANIC("get_handle_koids(node_ref_) failed - status: %d", status);
  }
  ZX_DEBUG_ASSERT(logical_buffer_collection_);
  logical_buffer_collection_->TrackNodeProperties(this);
}

uint32_t NodeProperties::node_count() const { return node_count_; }

uint32_t NodeProperties::connected_client_count() const { return connected_client_count_; }

uint32_t NodeProperties::buffer_collection_count() const { return buffer_collection_count_; }

uint32_t NodeProperties::buffer_collection_token_count() const {
  return buffer_collection_token_count_;
}

void NodeProperties::LogInfo(Location location, const char* format, ...) const {
  va_list args;
  va_start(args, format);
  logical_buffer_collection_->VLogClientInfo(location, this, format, args);
  va_end(args);
}

void NodeProperties::LogError(Location location, const char* format, ...) const {
  va_list args;
  va_start(args, format);
  logical_buffer_collection_->VLogClientError(location, this, format, args);
  va_end(args);
}

void NodeProperties::LogConstraints(Location location) {
  if (!has_constraints()) {
    LogInfo(FROM_HERE, "No constraints yet.");
    return;
  }
  logical_buffer_collection_->LogConstraints(location, this, *buffer_collection_constraints());
}

const char* NodeProperties::node_type_name() const { return node()->node_type_string(); }

ConnectionVersion NodeProperties::connection_version() const {
  return node()->connection_version();
}

}  // namespace sysmem_service
