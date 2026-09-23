// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "buffer_collection_token_group.h"

#include <lib/fidl/cpp/wire/channel.h>
#include <lib/trace/event.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include "fidl/fuchsia.sysmem/cpp/markers.h"
#include "node.h"
#include "src/sysmem/server/node_properties.h"

namespace sysmem_service {

void BufferCollectionTokenGroup::V1::Sync(SyncCompleter::Sync& completer) {
  parent_.SyncImpl(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionTokenGroup::V2::Sync(SyncCompleter::Sync& completer) {
  parent_.SyncImpl(ConnectionVersion::kVersion2, completer);
}

void BufferCollectionTokenGroup::V1::Close(CloseCompleter::Sync& completer) {
  if (!parent_.ReadyForAllocation()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_BAD_STATE,
                     "Close() before AllChildrenPresent()");
    return;
  }
  parent_.ReleaseImpl(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionTokenGroup::V2::Release(ReleaseCompleter::Sync& completer) {
  if (!parent_.ReadyForAllocation()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "Release() before AllChildrenPresent()");
    return;
  }
  parent_.ReleaseImpl(ConnectionVersion::kVersion2, completer);
}

void BufferCollectionTokenGroup::V1::GetNodeRef(GetNodeRefCompleter::Sync& completer) {
  parent_.GetNodeRefImplV1(completer);
}

void BufferCollectionTokenGroup::V2::GetNodeRef(GetNodeRefCompleter::Sync& completer) {
  parent_.GetNodeRefImplV2(completer);
}

void BufferCollectionTokenGroup::V1::IsAlternateFor(IsAlternateForRequest& request,
                                                    IsAlternateForCompleter::Sync& completer) {
  parent_.IsAlternateForImplV1(request, completer);
}

void BufferCollectionTokenGroup::V2::IsAlternateFor(IsAlternateForRequest& request,
                                                    IsAlternateForCompleter::Sync& completer) {
  parent_.IsAlternateForImplV2(request, completer);
}

void BufferCollectionTokenGroup::V2::GetBufferCollectionId(
    GetBufferCollectionIdCompleter::Sync& completer) {
  parent_.GetBufferCollectionIdImplV2(completer);
}

void BufferCollectionTokenGroup::V2::SetWeak(SetWeakCompleter::Sync& completer) {
  parent_.SetWeakImplV2(completer);
}

void BufferCollectionTokenGroup::V2::SetWeakOk(SetWeakOkRequest& request,
                                               SetWeakOkCompleter::Sync& completer) {
  parent_.SetWeakOkImplV2(request, completer);
}

void BufferCollectionTokenGroup::V1::SetName(SetNameRequest& request,
                                             SetNameCompleter::Sync& completer) {
  parent_.SetNameImplV1(request, completer);
}

void BufferCollectionTokenGroup::V2::SetName(SetNameRequest& request,
                                             SetNameCompleter::Sync& completer) {
  parent_.SetNameImplV2(request, completer);
}

void BufferCollectionTokenGroup::V1::SetDebugClientInfo(
    SetDebugClientInfoRequest& request, SetDebugClientInfoCompleter::Sync& completer) {
  parent_.SetDebugClientInfoImplV1(request, completer);
}

void BufferCollectionTokenGroup::V2::SetDebugClientInfo(
    SetDebugClientInfoRequest& request, SetDebugClientInfoCompleter::Sync& completer) {
  parent_.SetDebugClientInfoImplV2(request, completer);
}

void BufferCollectionTokenGroup::V1::SetDebugTimeoutLogDeadline(
    SetDebugTimeoutLogDeadlineRequest& request,
    SetDebugTimeoutLogDeadlineCompleter::Sync& completer) {
  parent_.SetDebugTimeoutLogDeadlineImplV1(request, completer);
}

void BufferCollectionTokenGroup::V2::SetDebugTimeoutLogDeadline(
    SetDebugTimeoutLogDeadlineRequest& request,
    SetDebugTimeoutLogDeadlineCompleter::Sync& completer) {
  parent_.SetDebugTimeoutLogDeadlineImplV2(request, completer);
}

void BufferCollectionTokenGroup::V1::SetVerboseLogging(
    SetVerboseLoggingCompleter::Sync& completer) {
  parent_.SetVerboseLoggingImpl(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionTokenGroup::V2::SetVerboseLogging(
    SetVerboseLoggingCompleter::Sync& completer) {
  parent_.SetVerboseLoggingImpl(ConnectionVersion::kVersion2, completer);
}

void BufferCollectionTokenGroup::V2::AttachNodeTracking(
    AttachNodeTrackingRequest& request, AttachNodeTrackingCompleter::Sync& completer) {
  parent_.AttachNodeTrackingImpl(request, completer);
}

template <typename Completer>
bool BufferCollectionTokenGroup::CommonCreateChildStage1(
    ConnectionVersion version, Completer& completer,
    std::optional<uint32_t> input_rights_attenuation_mask, NodeProperties** out_node_properties) {
  if (is_done_) {
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE, "CreateChild() after Close()");
    return false;
  }
  if (is_all_children_present_) {
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE,
             "CreateChild() after AllChildrenPresent()");
    return false;
  }
  uint32_t rights_attenuation_mask = ZX_RIGHT_SAME_RIGHTS;
  if (input_rights_attenuation_mask.has_value()) {
    rights_attenuation_mask = input_rights_attenuation_mask.value();
  }
  if (rights_attenuation_mask == 0) {
    FailSync(FROM_HERE, version, completer, ZX_ERR_INVALID_ARGS,
             "CreateChild() rights_attenuation_mask 0 not permitted");
    return false;
  }
  NodeProperties* new_node_properties = node_properties().NewChild(&logical_buffer_collection());
  if (rights_attenuation_mask != ZX_RIGHT_SAME_RIGHTS) {
    new_node_properties->rights_attenuation_mask() &= rights_attenuation_mask;
  }
  *out_node_properties = new_node_properties;
  return true;
}

void BufferCollectionTokenGroup::V1::CreateChild(CreateChildRequest& request,
                                                 CreateChildCompleter::Sync& completer) {
  if (!request.token_request().has_value()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_INVALID_ARGS,
                     "CreateChild() requires token_request set");
    return;
  }

  std::optional<uint32_t> rights_attenuation_mask;
  if (request.rights_attenuation_mask().has_value()) {
    rights_attenuation_mask = request.rights_attenuation_mask().value();
  }
  NodeProperties* new_node_properties;
  if (!parent_.CommonCreateChildStage1(ConnectionVersion::kVersion1, completer,
                                       rights_attenuation_mask, &new_node_properties)) {
    return;
  }

  parent_.logical_buffer_collection().CreateBufferCollectionTokenV1(
      parent_.shared_logical_buffer_collection(), new_node_properties,
      std::move(request.token_request().value()));
}

void BufferCollectionTokenGroup::V2::CreateChild(CreateChildRequest& request,
                                                 CreateChildCompleter::Sync& completer) {
  if (!request.token_request().has_value()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_INVALID_ARGS,
                     "CreateChild() requires token_request set");
    return;
  }

  // In contrast to CreateChildrenSync(), and in contrast to BufferCollectionToken::Duplicate(), we
  // don't require rights_attenuation_mask to be set, since a BufferCollectionTokenGroup is often
  // not the ideal place to impose a rights_attenuation_mask in the first place, so don't force the
  // client to fill out a field that would very often just be ZX_RIGHT_SAME_RIGHTS anyway.
  std::optional<uint32_t> rights_attenuation_mask;
  if (request.rights_attenuation_mask().has_value()) {
    rights_attenuation_mask = request.rights_attenuation_mask().value();
  }
  NodeProperties* new_node_properties;
  if (!parent_.CommonCreateChildStage1(ConnectionVersion::kVersion2, completer,
                                       rights_attenuation_mask, &new_node_properties)) {
    return;
  }

  parent_.logical_buffer_collection().CreateBufferCollectionTokenV2(
      parent_.shared_logical_buffer_collection(), new_node_properties,
      std::move(request.token_request().value()));
}

void BufferCollectionTokenGroup::V1::CreateChildrenSync(
    CreateChildrenSyncRequest& request, CreateChildrenSyncCompleter::Sync& completer) {
  if (parent_.is_done_) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_BAD_STATE,
                     "CreateChildrenSync() after Close()");
    return;
  }
  if (parent_.is_all_children_present_) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_BAD_STATE,
                     "CreateChildrenSync() after AllChildrenPresent()");
    return;
  }
  for (auto& rights_attenuation_mask : request.rights_attenuation_masks()) {
    if (rights_attenuation_mask == 0) {
      parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_INVALID_ARGS,
                       "CreateChildrenSync() rights_attenuation_mask 0 not permitted");
      return;
    }
  }
  std::vector<fidl::ClientEnd<fuchsia_sysmem::BufferCollectionToken>> new_tokens;
  for (auto& rights_attenuation_mask : request.rights_attenuation_masks()) {
    auto token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem::BufferCollectionToken>();
    if (!token_endpoints.is_ok()) {
      parent_.FailSync(
          FROM_HERE, ConnectionVersion::kVersion1, completer, token_endpoints.status_value(),
          "BufferCollectionTokenGroup::CreateChildrenSync() failed to create token channel.");
      return;
    }

    NodeProperties* new_node_properties =
        parent_.node_properties().NewChild(&parent_.logical_buffer_collection());
    if (rights_attenuation_mask != ZX_RIGHT_SAME_RIGHTS) {
      new_node_properties->rights_attenuation_mask() &= rights_attenuation_mask;
    }
    parent_.logical_buffer_collection().CreateBufferCollectionTokenV1(
        parent_.shared_logical_buffer_collection(), new_node_properties,
        std::move(token_endpoints->server));
    new_tokens.push_back(std::move(token_endpoints->client));
  }
  completer.Reply(std::move(new_tokens));
}

void BufferCollectionTokenGroup::V2::CreateChildrenSync(
    CreateChildrenSyncRequest& request, CreateChildrenSyncCompleter::Sync& completer) {
  if (parent_.is_done_) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "CreateChildrenSync() after Close()");
    return;
  }
  if (parent_.is_all_children_present_) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "CreateChildrenSync() after AllChildrenPresent()");
    return;
  }
  if (!request.rights_attenuation_masks().has_value()) {
    // The size of rights_attenuation_mask determines how many children get created, so we need this
    // set, despite it sometimes requiring the client to send a few ZX_RIGHT_SAME_RIGHTS, just to
    // get the right number of children created (such as when the client is attenuating rights via
    // a separate Duplicate() / DuplicateSync()).
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "CreateChildrenSync() requires rights_attenuation_masks set");
    return;
  }
  for (auto& rights_attenuation_mask : *request.rights_attenuation_masks()) {
    if (rights_attenuation_mask == 0) {
      parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_INVALID_ARGS,
                       "CreateChildrenSync() rights_attenuation_mask 0 not permitted");
      return;
    }
  }
  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> new_tokens;
  for (auto& rights_attenuation_mask : request.rights_attenuation_masks().value()) {
    auto token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
    if (!token_endpoints.is_ok()) {
      parent_.FailSync(
          FROM_HERE, ConnectionVersion::kVersion2, completer, token_endpoints.status_value(),
          "BufferCollectionTokenGroup::CreateChildrenSync() failed to create token channel.");
      return;
    }

    NodeProperties* new_node_properties =
        parent_.node_properties().NewChild(&parent_.logical_buffer_collection());
    if (rights_attenuation_mask != ZX_RIGHT_SAME_RIGHTS) {
      new_node_properties->rights_attenuation_mask() &= rights_attenuation_mask;
    }
    parent_.logical_buffer_collection().CreateBufferCollectionTokenV2(
        parent_.shared_logical_buffer_collection(), new_node_properties,
        std::move(token_endpoints->server));
    new_tokens.push_back(std::move(token_endpoints->client));
  }
  fuchsia_sysmem2::BufferCollectionTokenGroupCreateChildrenSyncResponse response;
  response.tokens().emplace(std::move(new_tokens));
  completer.Reply(std::move(response));
}

template <typename Completer>
void BufferCollectionTokenGroup::CommonAllChildrenPresent(ConnectionVersion version,
                                                          Completer& completer) {
  if (is_done_) {
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE, "AllChildrenPresent() after Close()");
    return;
  }
  if (is_all_children_present_) {
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE,
             "AllChildrenPresent() after AllChildrenPresent()");
    return;
  }
  if (node_properties().child_count() < 1) {
    // If this restriction creates a hassle, we could add a non-default per-group bool to allow zero
    // children under a group if a client indicates that it may happen for a specific group. For now
    // a client can add a child that sets empty constraints, if it turns out after creating a group
    // that the group won't need to have any children.
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE,
             "AllChildrenPresent() without any children");
    return;
  }
  is_all_children_present_ = true;
  logical_buffer_collection().OnDependencyReady();
}

void BufferCollectionTokenGroup::V1::AllChildrenPresent(
    AllChildrenPresentCompleter::Sync& completer) {
  parent_.CommonAllChildrenPresent(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionTokenGroup::V2::AllChildrenPresent(
    AllChildrenPresentCompleter::Sync& completer) {
  parent_.CommonAllChildrenPresent(ConnectionVersion::kVersion2, completer);
}

void BufferCollectionTokenGroup::V2::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_sysmem2::BufferCollectionTokenGroup> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_NOT_SUPPORTED,
                   "token group unknown method - ordinal: %" PRIx64, metadata.method_ordinal);
}

BufferCollectionTokenGroup& BufferCollectionTokenGroup::EmplaceInTree(
    fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection,
    NodeProperties* new_node_properties, const GroupServerEnd& server_end) {
  auto group = fbl::AdoptRef(new BufferCollectionTokenGroup(
      std::move(logical_buffer_collection), new_node_properties, std::move(server_end)));
  auto group_ptr = group.get();
  new_node_properties->SetNode(group);
  return *group_ptr;
}

BufferCollectionTokenGroup::BufferCollectionTokenGroup(fbl::RefPtr<LogicalBufferCollection> parent,
                                                       NodeProperties* new_node_properties,
                                                       const GroupServerEnd& server_end)
    : Node(std::move(parent), new_node_properties, GetUnownedChannel(server_end)) {
  TRACE_DURATION("gfx", "BufferCollectionTokenGroup::BufferCollectionTokenGroup", "this", this,
                 "logical_buffer_collection", &this->logical_buffer_collection());
  ZX_DEBUG_ASSERT(shared_logical_buffer_collection());
  inspect_node_ =
      this->logical_buffer_collection().inspect_node().CreateChild(CreateUniqueName("group-"));
}

void BufferCollectionTokenGroup::Bind(GroupServerEnd server_end) {
  Node::Bind(TakeNodeServerEnd(std::move(server_end)));
}

void BufferCollectionTokenGroup::BindInternalV1(zx::channel group_request,
                                                ErrorHandlerWrapper error_handler_wrapper) {
  v1_server_.emplace(*this);
  server_binding_v1_ = fidl::BindServer(
      parent_sysmem()->loop_dispatcher(),
      fidl::ServerEnd<fuchsia_sysmem::BufferCollectionTokenGroup>(std::move(group_request)),
      &v1_server_.value(),
      [error_handler_wrapper = std::move(error_handler_wrapper)](
          BufferCollectionTokenGroup::V1* group, fidl::UnbindInfo info, GroupServerEndV1 channel) {
        error_handler_wrapper(info);
      });
}

void BufferCollectionTokenGroup::BindInternalV2(zx::channel group_request,
                                                ErrorHandlerWrapper error_handler_wrapper) {
  v2_server_.emplace(*this);
  server_binding_v2_ = fidl::BindServer(
      parent_sysmem()->loop_dispatcher(),
      fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionTokenGroup>(std::move(group_request)),
      &v2_server_.value(),
      [error_handler_wrapper = std::move(error_handler_wrapper)](
          BufferCollectionTokenGroup::V2* group, fidl::UnbindInfo info, GroupServerEndV2 channel) {
        error_handler_wrapper(info);
      });
}

void BufferCollectionTokenGroup::BindInternalCombinedV1AndV2(
    zx::channel server_end, ErrorHandlerWrapper error_handler_wrapper) {
  ZX_PANIC("BufferCollectionTokenGroup only serves V1 or V2 separately - never combined V1 and V2");
}

bool BufferCollectionTokenGroup::ReadyForAllocation() { return is_all_children_present_; }

void BufferCollectionTokenGroup::OnBuffersAllocated(const AllocationResult& allocation_result) {
  node_properties().SetBuffersLogicallyAllocated();
}

BufferCollectionToken* BufferCollectionTokenGroup::buffer_collection_token() { return nullptr; }

const BufferCollectionToken* BufferCollectionTokenGroup::buffer_collection_token() const {
  return nullptr;
}

BufferCollection* BufferCollectionTokenGroup::buffer_collection() { return nullptr; }

const BufferCollection* BufferCollectionTokenGroup::buffer_collection() const { return nullptr; }

OrphanedNode* BufferCollectionTokenGroup::orphaned_node() { return nullptr; }

const OrphanedNode* BufferCollectionTokenGroup::orphaned_node() const { return nullptr; }

BufferCollectionTokenGroup* BufferCollectionTokenGroup::buffer_collection_token_group() {
  return this;
}

const BufferCollectionTokenGroup* BufferCollectionTokenGroup::buffer_collection_token_group()
    const {
  return this;
}

bool BufferCollectionTokenGroup::is_connected_type() const { return true; }

bool BufferCollectionTokenGroup::is_currently_connected() const {
  return server_binding_v1_.has_value() || server_binding_v2_.has_value();
}

void BufferCollectionTokenGroup::CloseServerBinding(zx_status_t epitaph) {
  if (server_binding_v1_.has_value()) {
    server_binding_v1_->Close(epitaph);
  }
  if (server_binding_v2_.has_value()) {
    server_binding_v2_->Close(ZX_ERR_INTERNAL);
  }
  server_binding_v1_ = {};
  server_binding_v2_ = {};
}

const char* BufferCollectionTokenGroup::node_type_string() const { return "group"; }

ConnectionVersion BufferCollectionTokenGroup::connection_version() const {
  if (server_binding_v2_.has_value()) {
    return ConnectionVersion::kVersion2;
  }
  if (server_binding_v1_.has_value()) {
    return ConnectionVersion::kVersion1;
  }
  return ConnectionVersion::kNoConnection;
}

}  // namespace sysmem_service
