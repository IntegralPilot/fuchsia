// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_BUFFER_COLLECTION_TOKEN_GROUP_H_
#define SRC_SYSMEM_SERVER_BUFFER_COLLECTION_TOKEN_GROUP_H_

#include <fidl/fuchsia.sysmem/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>

#include "lib/zx/channel.h"
#include "node.h"

namespace sysmem_service {

// A BufferCollectionTokenGroup represents a prioritized OR among the child tokens of the group.
//
// For example a participant can create a first token that's preferred and a second token that's
// fallback.  If aggregation using the preferred token fails, aggregation will be re-attempted using
// the fallback token.
class BufferCollectionTokenGroup : public Node {
 public:
  static BufferCollectionTokenGroup& EmplaceInTree(
      fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection,
      NodeProperties* node_properties, const GroupServerEnd& server_end);

  // Node interface
  bool ReadyForAllocation() override;

  void OnBuffersAllocated(const AllocationResult& allocation_result) override;
  BufferCollectionToken* buffer_collection_token() override;
  const BufferCollectionToken* buffer_collection_token() const override;
  BufferCollection* buffer_collection() override;
  const BufferCollection* buffer_collection() const override;
  OrphanedNode* orphaned_node() override;
  const OrphanedNode* orphaned_node() const override;
  BufferCollectionTokenGroup* buffer_collection_token_group() override;
  const BufferCollectionTokenGroup* buffer_collection_token_group() const override;
  bool is_connected_type() const override;
  bool is_currently_connected() const override;
  const char* node_type_string() const override;
  ConnectionVersion connection_version() const override;

  void Bind(GroupServerEnd server_end);

 protected:
  void BindInternalV1(zx::channel group_request,
                      ErrorHandlerWrapper error_handler_wrapper) override;
  void BindInternalV2(zx::channel group_request,
                      ErrorHandlerWrapper error_handler_wrapper) override;
  void BindInternalCombinedV1AndV2(zx::channel server_end,
                                   ErrorHandlerWrapper error_handler_wrapper) override;

  void CloseServerBinding(zx_status_t epitaph) override;

 private:
  struct V1 : public fidl::Server<fuchsia_sysmem::BufferCollectionTokenGroup> {
    explicit V1(BufferCollectionTokenGroup& parent) : parent_(parent) {}

    // FIDL "compose Node" "interface" (identical among BufferCollection, BufferCollectionToken,
    // BufferCollectionTokenGroup)
    void Sync(SyncCompleter::Sync& completer) override;
    void Close(CloseCompleter::Sync& completer) override;
    void GetNodeRef(GetNodeRefCompleter::Sync& completer) override;
    void IsAlternateFor(IsAlternateForRequest& request,
                        IsAlternateForCompleter::Sync& completer) override;
    void SetName(SetNameRequest& request, SetNameCompleter::Sync& completer) override;
    void SetDebugClientInfo(SetDebugClientInfoRequest& request,
                            SetDebugClientInfoCompleter::Sync& completer) override;
    void SetDebugTimeoutLogDeadline(SetDebugTimeoutLogDeadlineRequest& request,
                                    SetDebugTimeoutLogDeadlineCompleter::Sync& completer) override;
    void SetVerboseLogging(SetVerboseLoggingCompleter::Sync& completer) override;

    //
    // fuchsia.sysmem.BufferCollectionTokenGroup interface methods (see also "compose Node" methods
    // above)
    //
    void CreateChild(CreateChildRequest& request, CreateChildCompleter::Sync& completer) override;
    void CreateChildrenSync(CreateChildrenSyncRequest& request,
                            CreateChildrenSyncCompleter::Sync& completer) override;
    void AllChildrenPresent(AllChildrenPresentCompleter::Sync& completer) override;

    BufferCollectionTokenGroup& parent_;
  };

  struct V2 : public fidl::Server<fuchsia_sysmem2::BufferCollectionTokenGroup> {
    explicit V2(BufferCollectionTokenGroup& parent) : parent_(parent) {}

    // FIDL "compose Node" "interface" (identical among BufferCollection, BufferCollectionToken,
    // BufferCollectionTokenGroup)
    void Sync(SyncCompleter::Sync& completer) override;
    void Release(ReleaseCompleter::Sync& completer) override;
    void GetNodeRef(GetNodeRefCompleter::Sync& completer) override;
    void IsAlternateFor(IsAlternateForRequest& request,
                        IsAlternateForCompleter::Sync& completer) override;
    void GetBufferCollectionId(GetBufferCollectionIdCompleter::Sync& completer) override;
    void SetWeak(SetWeakCompleter::Sync& completer) override;
    void SetWeakOk(SetWeakOkRequest& request, SetWeakOkCompleter::Sync& completer) override;
    void SetName(SetNameRequest& request, SetNameCompleter::Sync& completer) override;
    void SetDebugClientInfo(SetDebugClientInfoRequest& request,
                            SetDebugClientInfoCompleter::Sync& completer) override;
    void SetDebugTimeoutLogDeadline(SetDebugTimeoutLogDeadlineRequest& request,
                                    SetDebugTimeoutLogDeadlineCompleter::Sync& completer) override;
    void SetVerboseLogging(SetVerboseLoggingCompleter::Sync& completer) override;
    void AttachNodeTracking(AttachNodeTrackingRequest& request,
                            AttachNodeTrackingCompleter::Sync& completer) override;

    //
    // fuchsia.sysmem.BufferCollectionTokenGroup interface methods (see also "compose Node" methods
    // above)
    //
    void CreateChild(CreateChildRequest& request, CreateChildCompleter::Sync& completer) override;
    void CreateChildrenSync(CreateChildrenSyncRequest& request,
                            CreateChildrenSyncCompleter::Sync& completer) override;
    void AllChildrenPresent(AllChildrenPresentCompleter::Sync& completer) override;

    void handle_unknown_method(
        fidl::UnknownMethodMetadata<fuchsia_sysmem2::BufferCollectionTokenGroup> metadata,
        fidl::UnknownMethodCompleter::Sync& completer) override;

    BufferCollectionTokenGroup& parent_;
  };

  BufferCollectionTokenGroup(fbl::RefPtr<LogicalBufferCollection> parent,
                             NodeProperties* new_node_properties, const GroupServerEnd& server_end);

  template <typename Completer>
  bool CommonCreateChildStage1(ConnectionVersion version, Completer& completer,
                               std::optional<uint32_t> input_rights_attenuation_mask,
                               NodeProperties** out_node_properties);

  template <typename Completer>
  void CommonAllChildrenPresent(ConnectionVersion version, Completer& completer);

  std::optional<V1> v1_server_;
  std::optional<V2> v2_server_;

  std::optional<fidl::ServerBindingRef<fuchsia_sysmem::BufferCollectionTokenGroup>>
      server_binding_v1_;
  std::optional<fidl::ServerBindingRef<fuchsia_sysmem2::BufferCollectionTokenGroup>>
      server_binding_v2_;

  bool is_all_children_present_ = false;
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_BUFFER_COLLECTION_TOKEN_GROUP_H_
