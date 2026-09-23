// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_BUFFER_COLLECTION_TOKEN_H_
#define SRC_SYSMEM_SERVER_BUFFER_COLLECTION_TOKEN_H_

#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2.internal/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>

#include "lib/zx/channel.h"
#include "logging.h"
#include "logical_buffer_collection.h"
#include "node.h"

namespace sysmem_service {

class BufferCollectionToken;

class BufferCollectionToken : public Node, public LoggingMixin {
 public:
  ~BufferCollectionToken() override;

  // The returned reference is owned by new_node_properties, which in turn is owned by
  // logical_buffer_collection->root_.
  static BufferCollectionToken& EmplaceInTree(
      fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection,
      NodeProperties* new_node_properties, const TokenServerEnd& server_end);

  template <class CompleterSync>
  void TokenReleaseImpl(ConnectionVersion version, CompleterSync& completer) {
    // BufferCollectionToken has one additional error case we want to check, so check before calling
    // Node::CloseImpl().
    if (buffer_collection_request_) {
      FailSync(FROM_HERE, version, completer, ZX_ERR_BAD_STATE,
               "BufferCollectionToken::Close()/Release() when buffer_collection_request_");
      // We're failing async - no need to try to fail sync.
      return;
    }
    ReleaseImpl(version, completer);
  }

  void OnServerKoid();

  void SetDispensableInternal();

  bool is_done();

  void SetBufferCollectionRequest(CollectionServerEnd buffer_collection_request);

  std::optional<CollectionServerEnd> TakeBufferCollectionRequest();

  void CloseServerBinding(zx_status_t epitaph) override;

  // Node interface
  bool ReadyForAllocation() override;
  void OnBuffersAllocated(const AllocationResult& allocation_result) override;
  BufferCollectionToken* buffer_collection_token() override;
  const BufferCollectionToken* buffer_collection_token() const override;
  BufferCollection* buffer_collection() override;
  const BufferCollection* buffer_collection() const override;
  BufferCollectionTokenGroup* buffer_collection_token_group() override;
  const BufferCollectionTokenGroup* buffer_collection_token_group() const override;
  OrphanedNode* orphaned_node() override;
  const OrphanedNode* orphaned_node() const override;
  bool is_connected_type() const override;
  bool is_currently_connected() const override;
  const char* node_type_string() const override;
  ConnectionVersion connection_version() const override;

  void Bind(TokenServerEnd server_end);

  // This gets applied to the NodeProperties as the BufferCollectionToken is being replaced by a
  // BufferCollection, so that the Allocator's debug info applies after any SetDebugClientInfo
  // messages queued on the token at the time of BindSharedCollection.
  void QueueAllocatorClientDebugInfo(ClientDebugInfo debug_info);
  std::optional<ClientDebugInfo> take_queued_allocator_client_debug_info();

 protected:
  void BindInternalV1(zx::channel token_request,
                      ErrorHandlerWrapper error_handler_wrapper) override;
  void BindInternalV2(zx::channel token_request,
                      ErrorHandlerWrapper error_handler_wrapper) override;
  void BindInternalCombinedV1AndV2(zx::channel server_end,
                                   ErrorHandlerWrapper error_handler_wrapper) override;

 private:
  friend class FidlServer;

  // We keep the protocol impl separate to clarify which methods are protocol impl.  Sysmem serves
  // CombinedBufferCollectionToken which is both a V1 and V2 token at the same time.  This allows
  // us to keep the V1 and V2 protocols separate from a client's point of view, while also avoiding
  // any pointless token version conversions.  This happens to make sense for BufferCollectionToken
  // V1 and V2 since the token is logically the same in all stateful aspects; this approach being
  // used here is not an implicit proposal / promise to use this same approach for any hypothetical
  // V3.
  struct CombinedTokenServer
      : public fidl::Server<fuchsia_sysmem2_internal::CombinedBufferCollectionToken> {
    explicit CombinedTokenServer(BufferCollectionToken& parent) : parent_(parent) {}

    // V1
    //
    // FIDL "compose Node" "interface" (identical among BufferCollection, BufferCollectionToken,
    // BufferCollectionTokenGroup)
    void SyncV1(SyncV1Completer::Sync& completer) override;
    void CloseV1(CloseV1Completer::Sync& completer) override;
    void GetNodeRefV1(GetNodeRefV1Completer::Sync& completer) override;
    void IsAlternateForV1(IsAlternateForV1Request& request,
                          IsAlternateForV1Completer::Sync& completer) override;
    void SetNameV1(SetNameV1Request& request, SetNameV1Completer::Sync& completer) override;
    void SetDebugClientInfoV1(SetDebugClientInfoV1Request& request,
                              SetDebugClientInfoV1Completer::Sync& completer) override;
    void SetDebugTimeoutLogDeadlineV1(
        SetDebugTimeoutLogDeadlineV1Request& request,
        SetDebugTimeoutLogDeadlineV1Completer::Sync& completer) override;
    void SetVerboseLoggingV1(SetVerboseLoggingV1Completer::Sync& completer) override;
    // fuchsia.sysmem.BufferCollectionToken interface methods (see also "compose Node" methods
    // above)
    void DuplicateSyncV1(DuplicateSyncV1Request& request,
                         DuplicateSyncV1Completer::Sync& completer) override;
    void DuplicateV1(DuplicateV1Request& request, DuplicateV1Completer::Sync& completer) override;
    void CreateBufferCollectionTokenGroupV1(
        CreateBufferCollectionTokenGroupV1Request& request,
        CreateBufferCollectionTokenGroupV1Completer::Sync& completer) override;
    void SetDispensableV1(SetDispensableV1Completer::Sync& completer) override;

    // V2
    //
    // FIDL "compose Node" "interface" (identical among BufferCollection, BufferCollectionToken,
    // BufferCollectionTokenGroup)
    void SyncV2(SyncV2Completer::Sync& completer) override;
    void ReleaseV2(ReleaseV2Completer::Sync& completer) override;
    void GetNodeRefV2(GetNodeRefV2Completer::Sync& completer) override;
    void IsAlternateForV2(IsAlternateForV2Request& request,
                          IsAlternateForV2Completer::Sync& completer) override;
    void GetBufferCollectionIdV2(GetBufferCollectionIdV2Completer::Sync& completer) override;
    void SetWeakV2(SetWeakV2Completer::Sync& completer) override;
    void SetWeakOkV2(SetWeakOkV2Request& request, SetWeakOkV2Completer::Sync& completer) override;
    void SetNameV2(SetNameV2Request& request, SetNameV2Completer::Sync& completer) override;
    void SetDebugClientInfoV2(SetDebugClientInfoV2Request& request,
                              SetDebugClientInfoV2Completer::Sync& completer) override;
    void SetDebugTimeoutLogDeadlineV2(
        SetDebugTimeoutLogDeadlineV2Request& request,
        SetDebugTimeoutLogDeadlineV2Completer::Sync& completer) override;
    void SetVerboseLoggingV2(SetVerboseLoggingV2Completer::Sync& completer) override;
    // fuchsia.sysmem.BufferCollectionToken interface methods (see also "compose Node" methods
    // above)
    void DuplicateSyncV2(DuplicateSyncV2Request& request,
                         DuplicateSyncV2Completer::Sync& completer) override;
    void DuplicateV2(DuplicateV2Request& request, DuplicateV2Completer::Sync& completer) override;
    void CreateBufferCollectionTokenGroupV2(
        CreateBufferCollectionTokenGroupV2Request& request,
        CreateBufferCollectionTokenGroupV2Completer::Sync& completer) override;
    void SetDispensableV2(SetDispensableV2Completer::Sync& completer) override;
    void AttachNodeTrackingV2(AttachNodeTrackingV2Request& request,
                              AttachNodeTrackingV2Completer::Sync& completer) override;

    void handle_unknown_method(
        fidl::UnknownMethodMetadata<fuchsia_sysmem2_internal::CombinedBufferCollectionToken>
            metadata,
        fidl::UnknownMethodCompleter::Sync& completer) override;

    BufferCollectionToken& parent_;
  };

  BufferCollectionToken(fbl::RefPtr<LogicalBufferCollection> parent,
                        NodeProperties* new_node_properties, const TokenServerEnd& server_end);

  void FailAsync(Location location, zx_status_t status, const char* format, ...);

  template <typename Completer>
  bool CommonDuplicateStage1(uint32_t rights_attenuation_mask, ConnectionVersion version,
                             Completer& completer, NodeProperties** out_node_properties);

  template <typename Completer>
  bool CommonCreateBufferCollectionTokenGroupStage1(ConnectionVersion version, Completer& completer,
                                                    NodeProperties** out_node_properties);

  std::optional<CombinedTokenServer> server_;

  std::optional<zx_status_t> async_failure_result_;

  std::optional<fidl::ServerBindingRef<fuchsia_sysmem2_internal::CombinedBufferCollectionToken>>
      server_binding_;
  ConnectionVersion last_seen_version_ = ConnectionVersion::kNoConnection;

  // This is set up to once during
  // LogicalBufferCollection::BindSharedCollection(), and essentially curries
  // the buffer_collection_request past the processing of any remaining
  // inbound messages on the BufferCollectionToken before starting to serve
  // the BufferCollection that the token was exchanged for.  This way, inbound
  // Duplicate() messages in the BufferCollectionToken are seen before any
  // BufferCollection::SetConstraints() (which might otherwise try to allocate
  // buffers too soon before all tokens are gone)

  std::optional<CollectionServerEnd> buffer_collection_request_;
  std::optional<ClientDebugInfo> queued_allocator_client_debug_info_;

  inspect::Node inspect_node_;
  inspect::UintProperty debug_id_property_;
  inspect::StringProperty debug_name_property_;
  inspect::ValueList properties_;
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_BUFFER_COLLECTION_TOKEN_H_
