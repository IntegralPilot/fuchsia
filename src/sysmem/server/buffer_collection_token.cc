// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "buffer_collection_token.h"

#include <lib/trace/event.h>
#include <lib/zx/channel.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include "node.h"
#include "node_properties.h"
#include "sysmem.h"

namespace sysmem_service {

BufferCollectionToken::~BufferCollectionToken() {
  TRACE_DURATION("gfx", "BufferCollectionToken::~BufferCollectionToken", "this", this,
                 "logical_buffer_collection", &logical_buffer_collection());

  // zx_koid_t values are never re-used during lifetime of running system, so
  // it's fine that the channel is already closed (no possibility of re-use
  // of value in the tracked set of values).

  // It's fine if server_koid() is ZX_KOID_INVALID - no effect in that case.
  parent_sysmem()->UntrackToken(this);
}

void BufferCollectionToken::CloseServerBinding(zx_status_t epitaph) {
  if (server_binding_.has_value()) {
    if (last_seen_version_ == ConnectionVersion::kVersion1) {
      server_binding_->Close(epitaph);
    } else {
      server_binding_->Close(ZX_ERR_INTERNAL);
    }
  }
  server_binding_ = {};
  parent_sysmem()->UntrackToken(this);
}

// static
BufferCollectionToken& BufferCollectionToken::EmplaceInTree(
    fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection,
    NodeProperties* new_node_properties, const TokenServerEnd& server_end) {
  auto token = fbl::AdoptRef(new BufferCollectionToken(std::move(logical_buffer_collection),
                                                       new_node_properties, server_end));
  auto token_ptr = token.get();
  new_node_properties->SetNode(token);
  return *token_ptr;
}

void BufferCollectionToken::Bind(TokenServerEnd token_server_end) {
  Node::Bind(TakeNodeServerEnd(std::move(token_server_end)));
}

void BufferCollectionToken::QueueAllocatorClientDebugInfo(ClientDebugInfo debug_info) {
  ZX_DEBUG_ASSERT(!queued_allocator_client_debug_info_.has_value());
  queued_allocator_client_debug_info_.emplace(std::move(debug_info));
}

std::optional<ClientDebugInfo> BufferCollectionToken::take_queued_allocator_client_debug_info() {
  std::optional<ClientDebugInfo> result = std::move(queued_allocator_client_debug_info_);
  // We don't want to leave a has_value() true optional with moved-out contents, so reset() this
  // field before returning - this can be a reset() on an already nullopt sometimes.
  queued_allocator_client_debug_info_.reset();
  return result;
}

void BufferCollectionToken::BindInternalV1(zx::channel token_request,
                                           ErrorHandlerWrapper error_handler_wrapper) {
  ZX_PANIC("BufferCollectionToken never serves V1 alone - only combined V1 and V2");
}

void BufferCollectionToken::BindInternalV2(zx::channel token_request,
                                           ErrorHandlerWrapper error_handler_wrapper) {
  ZX_PANIC("BufferCollectionToken never serves V2 alone - only combined V1 and V2");
}

void BufferCollectionToken::BindInternalCombinedV1AndV2(zx::channel token_request,
                                                        ErrorHandlerWrapper error_handler_wrapper) {
  server_.emplace(*this);
  server_binding_ =
      fidl::BindServer(parent_sysmem()->loop_dispatcher(),
                       TokenServerEndCombinedV1AndV2(std::move(token_request)), &server_.value(),
                       [error_handler_wrapper = std::move(error_handler_wrapper)](
                           BufferCollectionToken::CombinedTokenServer* token, fidl::UnbindInfo info,
                           TokenServerEndCombinedV1AndV2 channel) { error_handler_wrapper(info); });
}

void BufferCollectionToken::CombinedTokenServer::DuplicateSyncV1(
    DuplicateSyncV1Request& request, DuplicateSyncV1Completer::Sync& completer) {
  TRACE_DURATION("gfx", "BufferCollectionToken::CombinedTokenServer::DuplicateSyncV1", "this", this,
                 "logical_buffer_collection", &parent_.logical_buffer_collection());
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  if (parent_.is_done_) {
    // Probably a Close() followed by DuplicateSync(), which is illegal and
    // causes the whole LogicalBufferCollection to fail.
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_BAD_STATE,
                     "BufferCollectionToken::DuplicateSync() attempted when is_done_");
    return;
  }
  std::vector<fidl::ClientEnd<fuchsia_sysmem::BufferCollectionToken>> new_tokens;

  for (auto& rights_attenuation_mask : request.rights_attenuation_masks()) {
    if (rights_attenuation_mask == 0) {
      parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer, ZX_ERR_INVALID_ARGS,
                       "DuplicateSync() rights_attenuation_mask 0 not permitted");
      return;
    }
  }

  for (auto& rights_attenuation_mask : request.rights_attenuation_masks()) {
    auto token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem::BufferCollectionToken>();
    if (!token_endpoints.is_ok()) {
      parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion1, completer,
                       token_endpoints.status_value(),
                       "BufferCollectionToken::DuplicateSync() failed to create token channel.");
      return;
    }

    NodeProperties* new_node_properties =
        parent_.node_properties().NewChild(&parent_.logical_buffer_collection());
    if (rights_attenuation_mask != ZX_RIGHT_SAME_RIGHTS) {
      new_node_properties->rights_attenuation_mask() &=
          safe_cast<uint32_t>(rights_attenuation_mask);
    }
    parent_.logical_buffer_collection().CreateBufferCollectionTokenV1(
        parent_.shared_logical_buffer_collection(), new_node_properties,
        std::move(token_endpoints->server));
    new_tokens.push_back(std::move(token_endpoints->client));
  }

  fuchsia_sysmem2_internal::CombinedBufferCollectionTokenDuplicateSyncV1Response response;
  response.tokens() = std::move(new_tokens);
  completer.Reply(std::move(response));
}

void BufferCollectionToken::CombinedTokenServer::DuplicateSyncV2(
    DuplicateSyncV2Request& request, DuplicateSyncV2Completer::Sync& completer) {
  TRACE_DURATION("gfx", "BufferCollectionToken::CombiendTokenServer::DuplicateSyncV2", "this", this,
                 "logical_buffer_collection", &parent_.logical_buffer_collection());
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  if (parent_.is_done_) {
    // Probably a Close() followed by DuplicateSync(), which is illegal and
    // causes the whole LogicalBufferCollection to fail.
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "DuplicateSync() attempted when is_done_");
    return;
  }
  if (!request.rights_attenuation_masks().has_value()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "DuplicateSync() requires rights_attenuation_masks set");
    return;
  }

  for (auto& rights_attenuation_mask : *request.rights_attenuation_masks()) {
    if (rights_attenuation_mask == 0) {
      parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_INVALID_ARGS,
                       "DuplicateSync() rights_attenuation_mask 0 not permitted");
      return;
    }
  }

  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>> new_tokens;
  for (auto& rights_attenuation_mask : request.rights_attenuation_masks().value()) {
    auto token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
    if (!token_endpoints.is_ok()) {
      parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer,
                       token_endpoints.status_value(),
                       "BufferCollectionToken::DuplicateSync() failed to create token channel.");
      return;
    }

    NodeProperties* new_node_properties =
        parent_.node_properties().NewChild(&parent_.logical_buffer_collection());
    if (rights_attenuation_mask != ZX_RIGHT_SAME_RIGHTS) {
      new_node_properties->rights_attenuation_mask() &=
          safe_cast<uint32_t>(rights_attenuation_mask);
    }
    parent_.logical_buffer_collection().CreateBufferCollectionTokenV2(
        parent_.shared_logical_buffer_collection(), new_node_properties,
        std::move(token_endpoints->server));
    new_tokens.push_back(std::move(token_endpoints->client));
  }

  fuchsia_sysmem2_internal::CombinedBufferCollectionTokenDuplicateSyncV2Response response;
  response.tokens() = std::move(new_tokens);
  completer.Reply(std::move(response));
}

template <typename Completer>
bool BufferCollectionToken::CommonDuplicateStage1(uint32_t rights_attenuation_mask,
                                                  ConnectionVersion version, Completer& completer,
                                                  NodeProperties** out_node_properties) {
  if (is_done_) {
    // Probably a Close() followed by Duplicate(), which is illegal and
    // causes the whole LogicalBufferCollection to fail.
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE,
             "BufferCollectionToken::Duplicate() attempted when is_done_");
    return false;
  }
  NodeProperties* new_node_properties = node_properties().NewChild(&logical_buffer_collection());
  if (rights_attenuation_mask == 0) {
    logical_buffer_collection().LogClientError(
        FROM_HERE, &node_properties(),
        "rights_attenuation_mask of 0 is DEPRECATED - use ZX_RIGHT_SAME_RIGHTS instead.");
    rights_attenuation_mask = ZX_RIGHT_SAME_RIGHTS;
  }
  if (rights_attenuation_mask != ZX_RIGHT_SAME_RIGHTS) {
    new_node_properties->rights_attenuation_mask() &= rights_attenuation_mask;
  }
  *out_node_properties = new_node_properties;
  return true;
}

void BufferCollectionToken::CombinedTokenServer::DuplicateV1(
    DuplicateV1Request& request, DuplicateV1Completer::Sync& completer) {
  TRACE_DURATION("gfx", "BufferCollectionToken::DuplicateV1", "this", this,
                 "logical_buffer_collection", &parent_.logical_buffer_collection());
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  NodeProperties* new_node_properties;
  if (!parent_.CommonDuplicateStage1(request.rights_attenuation_mask(),
                                     ConnectionVersion::kVersion1, completer,
                                     &new_node_properties)) {
    return;
  }
  parent_.logical_buffer_collection().CreateBufferCollectionTokenV1(
      parent_.shared_logical_buffer_collection(), new_node_properties,
      std::move(request.token_request()));
}

void BufferCollectionToken::CombinedTokenServer::DuplicateV2(
    DuplicateV2Request& request, DuplicateV2Completer::Sync& completer) {
  TRACE_DURATION("gfx", "BufferCollectionToken::DuplicateV2", "this", this,
                 "logical_buffer_collection", &parent_.logical_buffer_collection());
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  if (!request.rights_attenuation_mask().has_value()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "Duplicate() requires rights_attenuation_mask set");
    return;
  }
  if (!request.token_request().has_value()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "Duplicate() requires token_request set");
    return;
  }
  NodeProperties* new_node_properties;
  if (!parent_.CommonDuplicateStage1(request.rights_attenuation_mask().value(),
                                     ConnectionVersion::kVersion2, completer,
                                     &new_node_properties)) {
    return;
  }
  parent_.logical_buffer_collection().CreateBufferCollectionTokenV2(
      parent_.shared_logical_buffer_collection(), new_node_properties,
      std::move(request.token_request().value()));
}

void BufferCollectionToken::CombinedTokenServer::SyncV1(SyncV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.SyncImpl(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionToken::CombinedTokenServer::SyncV2(SyncV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SyncImpl(ConnectionVersion::kVersion2, completer);
}

// Clean token close without causing LogicalBufferCollection failure.
void BufferCollectionToken::CombinedTokenServer::CloseV1(CloseV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.TokenReleaseImpl(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionToken::CombinedTokenServer::ReleaseV2(ReleaseV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.TokenReleaseImpl(ConnectionVersion::kVersion2, completer);
}

void BufferCollectionToken::OnServerKoid() {
  ZX_DEBUG_ASSERT(has_server_koid());
  parent_sysmem()->TrackToken(this);
  if (parent_sysmem()->TryRemoveKoidFromUnfoundTokenList(server_koid())) {
    set_unfound_node();
    // LogicalBufferCollection will print an error, since it might have useful client information.
  }
}

bool BufferCollectionToken::is_done() { return is_done_; }

void BufferCollectionToken::SetBufferCollectionRequest(
    CollectionServerEnd buffer_collection_request) {
  if (is_done_ || buffer_collection_request_.has_value()) {
    FailAsync(FROM_HERE, ZX_ERR_BAD_STATE,
              "BufferCollectionToken::SetBufferCollectionRequest() attempted "
              "when already is_done_ || buffer_collection_request_");
    return;
  }
  ZX_DEBUG_ASSERT(!buffer_collection_request_);
  buffer_collection_request_ = std::move(buffer_collection_request);
}

std::optional<CollectionServerEnd> BufferCollectionToken::TakeBufferCollectionRequest() {
  return std::exchange(buffer_collection_request_, std::nullopt);
}

void BufferCollectionToken::CombinedTokenServer::SetNameV1(SetNameV1Request& request,
                                                           SetNameV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.SetNameImplV1(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetNameV2(SetNameV2Request& request,
                                                           SetNameV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetNameImplV2(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetDebugClientInfoV1(
    SetDebugClientInfoV1Request& request, SetDebugClientInfoV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.SetDebugClientInfoImplV1(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetDebugClientInfoV2(
    SetDebugClientInfoV2Request& request, SetDebugClientInfoV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetDebugClientInfoImplV2(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetDebugTimeoutLogDeadlineV1(
    SetDebugTimeoutLogDeadlineV1Request& request,
    SetDebugTimeoutLogDeadlineV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.SetDebugTimeoutLogDeadlineImplV1(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetDebugTimeoutLogDeadlineV2(
    SetDebugTimeoutLogDeadlineV2Request& request,
    SetDebugTimeoutLogDeadlineV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetDebugTimeoutLogDeadlineImplV2(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetDispensableV1(
    SetDispensableV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.SetDispensableInternal();
}

void BufferCollectionToken::CombinedTokenServer::SetDispensableV2(
    SetDispensableV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetDispensableInternal();
}

void BufferCollectionToken::CombinedTokenServer::AttachNodeTrackingV2(
    AttachNodeTrackingV2Request& request, AttachNodeTrackingV2Completer::Sync& completer) {
  parent_.AttachNodeTrackingImpl(request, completer);
}

void BufferCollectionToken::CombinedTokenServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_sysmem2_internal::CombinedBufferCollectionToken> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  parent_.FailSync(FROM_HERE, parent_.last_seen_version_, completer, ZX_ERR_NOT_SUPPORTED,
                   "token unknown method - ordinal: %" PRIx64, metadata.method_ordinal);
}

void BufferCollectionToken::SetDispensableInternal() {
  if (node_properties().error_propagation_mode() <
      ErrorPropagationMode::kPropagateBeforeAllocation) {
    node_properties().error_propagation_mode() = ErrorPropagationMode::kPropagateBeforeAllocation;
  }
}

template <typename Completer>
bool BufferCollectionToken::CommonCreateBufferCollectionTokenGroupStage1(
    ConnectionVersion version, Completer& completer, NodeProperties** out_node_properties) {
  if (is_done_) {
    // Probably a Close() followed by Duplicate(), which is illegal and
    // causes the whole LogicalBufferCollection to fail.
    FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE,
             "BufferCollectionToken::CreateBufferCollectionTokenGroup() attempted when is_done_");
    return false;
  }
  NodeProperties* new_node_properties = node_properties().NewChild(&logical_buffer_collection());
  *out_node_properties = new_node_properties;
  return true;
}

void BufferCollectionToken::CombinedTokenServer::CreateBufferCollectionTokenGroupV1(
    CreateBufferCollectionTokenGroupV1Request& request,
    CreateBufferCollectionTokenGroupV1Completer::Sync& completer) {
  TRACE_DURATION("gfx", "BufferCollectionTokenGroup::CreateBufferCollectionTokenGroupV1", "this",
                 this, "logical_buffer_collection", &parent_.logical_buffer_collection());
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  NodeProperties* new_node_properties;
  if (!parent_.CommonCreateBufferCollectionTokenGroupStage1(ConnectionVersion::kVersion1, completer,
                                                            &new_node_properties)) {
    return;
  }
  parent_.logical_buffer_collection().CreateBufferCollectionTokenGroupV1(
      parent_.shared_logical_buffer_collection(), new_node_properties,
      std::move(request.group_request()));
}

void BufferCollectionToken::CombinedTokenServer::CreateBufferCollectionTokenGroupV2(
    CreateBufferCollectionTokenGroupV2Request& request,
    CreateBufferCollectionTokenGroupV2Completer::Sync& completer) {
  TRACE_DURATION("gfx", "BufferCollectionTokenGroup::CreateBufferCollectionTokenGroupV2", "this",
                 this, "logical_buffer_collection", &parent_.logical_buffer_collection());
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  if (!request.group_request().has_value()) {
    parent_.FailSync(FROM_HERE, ConnectionVersion::kVersion2, completer, ZX_ERR_BAD_STATE,
                     "CreateBufferCollectionTokenGroup() requires group_request set");
    return;
  }
  NodeProperties* new_node_properties;
  if (!parent_.CommonCreateBufferCollectionTokenGroupStage1(ConnectionVersion::kVersion2, completer,
                                                            &new_node_properties)) {
    return;
  }
  parent_.logical_buffer_collection().CreateBufferCollectionTokenGroupV2(
      parent_.shared_logical_buffer_collection(), new_node_properties,
      std::move(request.group_request().value()));
}

void BufferCollectionToken::CombinedTokenServer::SetVerboseLoggingV1(
    SetVerboseLoggingV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.SetVerboseLoggingImpl(ConnectionVersion::kVersion1, completer);
}

void BufferCollectionToken::CombinedTokenServer::SetVerboseLoggingV2(
    SetVerboseLoggingV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetVerboseLoggingImpl(ConnectionVersion::kVersion2, completer);
}

void BufferCollectionToken::CombinedTokenServer::GetNodeRefV1(
    GetNodeRefV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.GetNodeRefImplV1(completer);
}

void BufferCollectionToken::CombinedTokenServer::GetNodeRefV2(
    GetNodeRefV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.GetNodeRefImplV2<
      GetNodeRefV2Completer::Sync,
      fuchsia_sysmem2_internal::CombinedBufferCollectionTokenGetNodeRefV2Response>(completer);
}

void BufferCollectionToken::CombinedTokenServer::IsAlternateForV1(
    IsAlternateForV1Request& request, IsAlternateForV1Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion1;
  parent_.IsAlternateForImplV1<
      IsAlternateForV1Request, IsAlternateForV1Completer::Sync,
      fuchsia_sysmem2_internal::CombinedBufferCollectionTokenIsAlternateForV1Response>(request,
                                                                                       completer);
}

void BufferCollectionToken::CombinedTokenServer::IsAlternateForV2(
    IsAlternateForV2Request& request, IsAlternateForV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.IsAlternateForImplV2<
      IsAlternateForV2Request, IsAlternateForV2Completer::Sync,
      fuchsia_sysmem2_internal::CombinedBufferCollectionTokenIsAlternateForV2Response>(request,
                                                                                       completer);
}

void BufferCollectionToken::CombinedTokenServer::GetBufferCollectionIdV2(
    GetBufferCollectionIdV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.GetBufferCollectionIdImplV2<
      GetBufferCollectionIdV2Completer::Sync,
      fuchsia_sysmem2_internal::CombinedBufferCollectionTokenGetBufferCollectionIdV2Response>(
      completer);
}

void BufferCollectionToken::CombinedTokenServer::SetWeakV2(SetWeakV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetWeakImplV2(completer);
}

void BufferCollectionToken::CombinedTokenServer::SetWeakOkV2(
    SetWeakOkV2Request& request, SetWeakOkV2Completer::Sync& completer) {
  parent_.last_seen_version_ = ConnectionVersion::kVersion2;
  parent_.SetWeakOkImplV2(request, completer);
}

BufferCollectionToken::BufferCollectionToken(
    fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection_param,
    NodeProperties* new_node_properties, const TokenServerEnd& server_end)
    : Node(std::move(logical_buffer_collection_param), new_node_properties,
           GetUnownedChannel(server_end)),
      LoggingMixin("BufferCollectionToken") {
  TRACE_DURATION("gfx", "BufferCollectionToken::BufferCollectionToken", "this", this,
                 "logical_buffer_collection", &logical_buffer_collection());
  inspect_node_ =
      logical_buffer_collection().inspect_node().CreateChild(CreateUniqueName("token-"));
  if (create_status() != ZX_OK) {
    // Node::Node() failed and maybe !has_server_koid().
    return;
  }
  // Node::Node filled this out (or didn't and status() reflected that, which was already checked
  // above).
  ZX_DEBUG_ASSERT(has_server_koid());
  OnServerKoid();
}

void BufferCollectionToken::FailAsync(Location location, zx_status_t status, const char* format,
                                      ...) {
  va_list args;
  va_start(args, format);
  vLog(fuchsia_logging::LogSeverity::Warn, location.file(), location.line(), logging_prefix(),
       format, args);
  va_end(args);

  // Idempotent, so only close once.
  if (!server_binding_.has_value()) {
    return;
  }

  async_failure_result_ = status;

  server_binding_->Close(status);
  server_binding_ = {};
}

bool BufferCollectionToken::ReadyForAllocation() { return false; }

void BufferCollectionToken::OnBuffersAllocated(const AllocationResult& allocation_result) {
  ZX_PANIC("Unexpected call to BufferCollectionToken::OnBuffersAllocated()");
}

BufferCollectionToken* BufferCollectionToken::buffer_collection_token() { return this; }

const BufferCollectionToken* BufferCollectionToken::buffer_collection_token() const { return this; }

BufferCollection* BufferCollectionToken::buffer_collection() { return nullptr; }

const BufferCollection* BufferCollectionToken::buffer_collection() const { return nullptr; }

BufferCollectionTokenGroup* BufferCollectionToken::buffer_collection_token_group() {
  return nullptr;
}

const BufferCollectionTokenGroup* BufferCollectionToken::buffer_collection_token_group() const {
  return nullptr;
}

OrphanedNode* BufferCollectionToken::orphaned_node() { return nullptr; }

const OrphanedNode* BufferCollectionToken::orphaned_node() const { return nullptr; }

bool BufferCollectionToken::is_connected_type() const { return true; }

bool BufferCollectionToken::is_currently_connected() const { return server_binding_.has_value(); }

const char* BufferCollectionToken::node_type_string() const { return "token"; }

ConnectionVersion BufferCollectionToken::connection_version() const {
  if (!server_binding_.has_value()) {
    return ConnectionVersion::kNoConnection;
  }
  return last_seen_version_;
}

}  // namespace sysmem_service
