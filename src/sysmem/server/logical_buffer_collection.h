// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_LOGICAL_BUFFER_COLLECTION_H_
#define SRC_SYSMEM_SERVER_LOGICAL_BUFFER_COLLECTION_H_

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <inttypes.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/channel.h>

#include <cstdint>
#include <list>
#include <map>
#include <memory>
#include <unordered_map>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/string_printf.h>

#include "src/sysmem/server/allocation_result.h"
#include "src/sysmem/server/indent.h"
#include "src/sysmem/server/logging.h"
#include "src/sysmem/server/node_properties.h"
#include "src/sysmem/server/sysmem.h"
#include "src/sysmem/server/usage_pixel_format_cost.h"
#include "src/sysmem/server/utils.h"
#include "src/sysmem/server/versions.h"

namespace sysmem_service {

class BufferCollectionToken;
class BufferCollectionTokenGroup;
class BufferCollection;
class LogicalBuffer;
class LogicalBufferCollection;
class MemoryAllocator;
class Node;

// This class can be used to hold an inspect snapshot of one set of constraints taken from a client
// at a particular point in time.
struct ConstraintInfoSnapshot {
  inspect::Node inspect_node;
};

// Each TrackedParentVmo keeps the LogicalBufferCollection alive as long as there are child VMOs
// outstanding (no revoking of child VMOs for now).
//
// This tracking is for the benefit of MemoryAllocator sub-classes that need
// a Delete() call, such as to clean up a slab allocation and/or to inform
// an external allocator of delete.
class TrackedParentVmo {
 public:
  using DoDelete = fit::callback<void(TrackedParentVmo* parent)>;
  // The do_delete callback will be invoked upon the sooner of (A) the client code causing
  // ~TrackedParentVmo, or (B) ZX_VMO_ZERO_CHILDREN occurring async after StartWait() is called.
  //
  // Each TrackedParentVmo associated with a LogicalBufferCollection keeps the
  // LogicalBufferCollection alive. Once a (child) VMO has been given out by sysmem, the only
  // mechanism to delete TrackedParentVmo is ZX_VMO_ZERO_CHILDREN.
  TrackedParentVmo(fbl::RefPtr<LogicalBufferCollection> logical_buffer, zx::vmo vmo,
                   uint32_t buffer_index, DoDelete do_delete);
  ~TrackedParentVmo();

  // This should only be called after client code has created a child VMO, and will begin the wait
  // for ZX_VMO_ZERO_CHILDREN.
  zx_status_t StartWait(async_dispatcher_t* dispatcher);

  // Cancel the wait.
  zx_status_t CancelWait();

  zx::vmo TakeVmo();
  [[nodiscard]] const zx::vmo& vmo() const;

  void set_child_koid(zx_koid_t koid) {
    ZX_DEBUG_ASSERT(!child_koid_.has_value());
    child_koid_ = koid;
  }
  [[nodiscard]] std::optional<zx_koid_t> child_koid() const { return child_koid_; }

  void set_client_debug_info(ClientDebugInfo client_debug_info) {
    client_debug_info_ = client_debug_info;
  }
  [[nodiscard]] ClientDebugInfo* get_client_debug_info() {
    return client_debug_info_.has_value() ? &*client_debug_info_ : nullptr;
  }

  uint32_t buffer_index() { return buffer_index_; }

  // no copy, no move (async::WaitMethod isn't anyway, but just to be clear about it)
  TrackedParentVmo(const TrackedParentVmo&) = delete;
  TrackedParentVmo& operator=(const TrackedParentVmo&) = delete;
  TrackedParentVmo(TrackedParentVmo&&) = delete;
  TrackedParentVmo& operator=(TrackedParentVmo&&) = delete;

 private:
  void OnZeroChildren(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                      const zx_packet_signal_t* signal);
  fbl::RefPtr<LogicalBufferCollection> buffer_collection_;
  zx::vmo vmo_;
  const uint32_t buffer_index_ = 0x80000000;

  // For TrackedParentVmo(s) which are direct parents of sysmem-provided VMO(s), the child VMO's
  // koid is retained here for calling Device::RemoveVmoKoid later, since GetVmoInfo by definition
  // only asks about VMOs that have handles open (a handle is passed into GetVmoInfo which ensures
  // this).
  std::optional<zx_koid_t> child_koid_;

  // A TrackedParentVmo can outlast a Node, so own a copy here.
  std::optional<ClientDebugInfo> client_debug_info_;

  DoDelete do_delete_;

  async::WaitMethod<TrackedParentVmo, &TrackedParentVmo::OnZeroChildren> zero_children_wait_;

  // Only for asserts:
  bool waiting_ = {};
};

// This is all the per-VMO info and mechanism except for
// LogicalBufferCollection::allocation_result_info_, which we keep in sync with the set of
// LogicalBuffer(s) in LogicalBufferCollection::buffers_.
//
// This is always held in a std::unique_ptr<> instead of move-only because TrackedParentVmo
// do_delete callbacks within need to capture a non-moving LogicalBuffer. We use std::unique_ptr<>
// rather than shared_ptr<> or fbl::RefPtr<> because we need to ensure we can just delete all the
// buffers in buffers_ if LogicalBufferCollection::Allocate() fails to allocate a later buffer.
class LogicalBuffer {
 public:
  static fit::result<zx_status_t, std::unique_ptr<LogicalBuffer>> Create(
      fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection, uint32_t buffer_index,
      zx::vmo parent_vmo);
  fit::result<zx_status_t, std::optional<zx::vmo>> CreateWeakVmo(
      const ClientDebugInfo& client_debug_info);

  LogicalBufferCollection& logical_buffer_collection();
  uint32_t buffer_index();

  // Client code should take this VMO before moving LogicalBuffer from stack to heap.
  zx::vmo TakeStrongChildVmo();

  fit::result<zx_status_t, zx::eventpair> DupCloseWeakAsapClientEnd();

  // move-only
  LogicalBuffer(const LogicalBuffer& to_copy) = delete;
  LogicalBuffer& operator=(const LogicalBuffer& to_copy) = delete;
  LogicalBuffer(LogicalBuffer&& to_move) = default;
  LogicalBuffer& operator=(LogicalBuffer&& to_move) = default;

 private:
  friend class LogicalBufferCollection;

  LogicalBuffer(fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection,
                uint32_t buffer_index, zx::vmo parent_vmo);
  // true iff construction was successful
  bool is_ok();
  // requires !is_ok(); returns the failure status
  zx_status_t error();

  void ComplainLoudlyAboutStillExistingWeakVmoHandles();

  fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection_;
  uint32_t buffer_index_;

  // This is the allocator-provided VMO (parent-most VMO tracked by sysmem; the allocator itself
  // may keep a parent VMO of this VMO, but that further parent is not known to sysmem).
  //
  // When this sees ZX_VMO_ZERO_CHILDREN, we can tell the allocator to free the allocator's VMO and
  // reclaim the space. This also deletes parent_vmo_ and LogicalBuffer.
  std::unique_ptr<TrackedParentVmo> parent_vmo_;

  // This is a child VMO of parent_vmo_, and has as children all sysmem strong VMOs associated with
  // this LogicalBuffer.
  //
  // When this sees ZX_VMO_ZERO_CHILDREN, we can ask clients to close any remaining weak VMOs by
  // closing the server end of close_weak_asap, and we set a timer to complain loudly if the
  // LogicalBuffer still exists after a while, since this would essentially count as a client
  // leaking a VMO. This also deletes strong_parent_vmo_.
  std::unique_ptr<TrackedParentVmo> strong_parent_vmo_;

  // These are parents of each weak VMO that was sent to a client (separate parent for each sent
  // weak VMO).
  //
  // This map exists to keep each TrackedParentVmo alive until all its child VMOs are gone, and to
  // have a way to complain about specific sent child weak VMOs that still haven't closed a while
  // after all strong VMOs of a buffer_index have closed.
  //
  // The TrackedParentVmo's vmo has its name set based on client debug info.
  std::unordered_map<TrackedParentVmo*, std::unique_ptr<TrackedParentVmo>> weak_parent_vmos_;

  struct CloseWeakAsap {
    zx::eventpair server_end;
    zx::eventpair client_end;
  };
  // ~close_weak_asap_ will signal clients via ZX_EVENTPAIR_PEER_CLOSED. This gets deleted when
  // ~strong_parent_vmo_.
  std::optional<CloseWeakAsap> close_weak_asap_;
  bool close_weak_asap_created_ = false;
  // If there were any outstanding weak VMOs when *close_weak_asap_ was deleted, this is when
  // close_weak_asap_ was deleted, signalling clients via ZX_EVENTPAIR_PEER_CLOSED. If there were
  // no outstanding weak VMOs when close_weak_asap_ was deleted, we don't add tallies to
  // weak_vmo_histograms because we want the histograms to only include logical buffer collections
  // that had any weak VMOs that needed to be closed; this is to avoid a bunch of tallies for
  // strong-only collections adding a bunch of tally counts in low duration histogram buckets.
  std::optional<zx::time> close_weak_asap_time_;

  // Set to a failing status if LogicalBuffer::LogicalBuffer() failed.
  zx_status_t error_ = ZX_OK;

  zx::vmo strong_child_vmo_;
};

// TODO(dustingreen): MaybeAllocate() should sweep all related incoming channels for ZX_PEER_CLOSED
// and not attempt allocation until all channel close(es) that were pending at the time have been
// processed.  Ignoring new channel closes is fine/good.
class LogicalBufferCollection : public fbl::RefCounted<LogicalBufferCollection> {
 public:
  using Arena = fidl::Arena<>;
  using CollectionMap = std::map<BufferCollection*, fbl::RefPtr<BufferCollection>>;

  // This is only called during driver stop. Deletion of the LogicalBufferCollection can be async.
  void Fail();

  ~LogicalBufferCollection();

  static void CreateV1(TokenServerEndV1 buffer_collection_token_request, Sysmem* parent_device,
                       const ClientDebugInfo* client_debug_info);
  static void CreateV2(TokenServerEndV2 buffer_collection_token_request, Sysmem* parent_device,
                       const ClientDebugInfo* client_debug_info);

  // |parent_device| the Device* that the calling allocator is part of.  The
  // tokens_by_koid_ for each Device is separate.  If somehow two clients were
  // to get connected to two separate sysmem device instances hosted in the
  // same devhost, those clients (intentionally) won't be able to share a
  // LogicalBufferCollection.
  //
  // |buffer_collection_token| the client end of the BufferCollectionToken
  // being turned in by the client to get a BufferCollection in exchange.
  //
  // |buffer_collection_request| the server end of a BufferCollection channel
  // to be served by the LogicalBufferCollection associated with
  // buffer_collection_token.
  static void BindSharedCollection(Sysmem* parent_device, zx::channel buffer_collection_token,
                                   CollectionServerEnd buffer_collection_request,
                                   const ClientDebugInfo* client_debug_info);

  // ZX_OK if the token is known to the server.
  // ZX_ERR_NOT_FOUND if the token isn't known to the server.
  static zx_status_t ValidateBufferCollectionToken(Sysmem* parent_device,
                                                   zx_koid_t token_server_koid);

  // This is used to create the initial BufferCollectionToken, and also used
  // by BufferCollectionToken::Duplicate().
  //
  // The |self| parameter exists only because LogicalBufferCollection can't
  // hold a std::weak_ptr<> to itself because that requires libc++ (the binary
  // not just the headers) which isn't available in Zircon so far.
  void CreateBufferCollectionTokenV1(fbl::RefPtr<LogicalBufferCollection> self,
                                     NodeProperties* new_node_properties,
                                     TokenServerEndV1 token_request);
  void CreateBufferCollectionTokenV2(fbl::RefPtr<LogicalBufferCollection> self,
                                     NodeProperties* new_node_properties,
                                     TokenServerEndV2 token_request);

  // This is used by BufferCollectionToken to create a BufferCollectionTokenGroup during the
  // FIDL request of the same name.
  void CreateBufferCollectionTokenGroupV1(fbl::RefPtr<LogicalBufferCollection> self,
                                          NodeProperties* new_node_properties,
                                          GroupServerEndV1 group_request);
  void CreateBufferCollectionTokenGroupV2(fbl::RefPtr<LogicalBufferCollection> self,
                                          NodeProperties* new_node_properties,
                                          GroupServerEndV2 group_request);
  bool CommonCreateBufferCollectionTokenGroupStage1(fbl::RefPtr<LogicalBufferCollection> self,
                                                    NodeProperties* new_node_properties,
                                                    const GroupServerEnd& group_request,
                                                    BufferCollectionTokenGroup** out_group);

  void AttachLifetimeTracking(zx::eventpair server_end, uint32_t buffers_remaining);
  void SweepLifetimeTracking();

  // Calling this extra times (including after allocation complete) isn't harmful from a correctness
  // point of view.
  void OnDependencyReady();

  void SetName(uint32_t priority, std::string name);
  void SetDebugTimeoutLogDeadline(int64_t deadline);
  void SetVerboseLogging();

  uint64_t CreateDispensableOrdinal();

  enum class LogSeverity : fuchsia_logging::RawLogSeverity {
    Trace = 0x10,
    Debug = 0x20,
    InfoOrDebug = 0x28,
    Info = 0x30,
    Warn = 0x40,
    Error = 0x50,
    Fatal = 0x60,
  };
  fuchsia_logging::LogSeverity LogSeverityToFuchsiaLogSeverity(
      LogicalBufferCollection::LogSeverity log_severity) const;
  void VLogClient(LogSeverity log_severity, Location location,
                  const NodeProperties* node_properties, const char* format, va_list args) const;
  void LogClientInfo(Location location, const NodeProperties* node_properties, const char* format,
                     ...) const __PRINTFLIKE(4, 5);
  void LogClientWarn(Location location, const NodeProperties* node_properties, const char* format,
                     ...) const __PRINTFLIKE(4, 5);
  void LogClientError(Location location, const NodeProperties* node_properties, const char* format,
                      ...) const __PRINTFLIKE(4, 5);
  void VLogClientInfo(Location location, const NodeProperties* node_properties, const char* format,
                      va_list args) const;
  void VLogClientWarn(Location location, const NodeProperties* node_properties, const char* format,
                      va_list args) const;
  void VLogClientError(Location location, const NodeProperties* node_properties, const char* format,
                       va_list args) const;

  // start buffering log output into log_buffer_ instead of logging immediately
  void LogBufferEnable();
  // flush (output) log_buffer_ and stop buffering further log output
  void LogBufferFlushAndDisable();
  // discard (drop) log_buffer_ and stop buffering further log output
  //
  // when is_verbose_logging() true, we attenuate the log_buffer_ to INFO but still output it
  void LogBufferDiscardAndDisable();

  Sysmem* parent_sysmem() const { return parent_sysmem_; }

  // For tests.
  std::vector<const BufferCollection*> collection_views() const;

  // Track/untrack the node by the koid of the client end of its FIDL channel.
  //
  // While tracked, a node can be found with FindNodeByClientChannelKoid().
  //
  // Only is_currently_connected() true Node(s) are tracked.
  //
  // Aside from this tracking, LogicalBufferCollection only cares about NodeProperties, not Nodes,
  // but since we need to track by client_koid which is a Node-specific thing, this tracking allows
  // for that.
  //
  // This tracking exists for the benefit of IsAlternateFor(), which is essentially called on one
  // node and refers to another node by client endpoint koid, which must be owned by the same
  // process as the calling node.
  void TrackNodeProperties(NodeProperties* node_properties);
  void UntrackNodeProperties(NodeProperties* node_properties);
  std::optional<NodeProperties*> FindNodePropertiesByNodeRefKoid(zx_koid_t node_ref_keep_koid);

  std::optional<std::string> name() const {
    return name_.has_value() ? std::make_optional(name_->name) : std::nullopt;
  }

  inspect::Node& inspect_node() { return inspect_node_; }

  bool is_verbose_logging() const { return is_verbose_logging_; }

  uint64_t buffer_collection_id() const { return buffer_collection_id_; }

  static fit::result<zx_status_t, BufferCollectionToken*> CommonConvertToken(
      Sysmem* parent_device, zx::channel buffer_collection_token,
      const ClientDebugInfo* client_debug_info, const char* fidl_message_name);

  fit::result<zx_status_t, std::optional<zx::vmo>> CreateWeakVmo(
      uint32_t buffer_index, const ClientDebugInfo& client_debug_info);
  fit::result<zx_status_t, std::optional<zx::eventpair>> DupCloseWeakAsapClientEnd(
      uint32_t buffer_index);

  void LogSummary(IndentTracker& indent);

  zx::time create_time_monotonic() const { return create_time_monotonic_; }

  const fuchsia_sysmem2::SingleBufferSettings& single_buffer_settings() {
    // Caller must ensure this will be present by the time
    // single_buffer_settings is called. For example, by only calling
    // single_buffer_settings when a vmo handle given out as part of the buffer
    // collection has come back from a sysmem client.
    ZX_ASSERT(buffer_collection_info_before_population_.has_value());
    return *buffer_collection_info_before_population_->settings();
  }

  bool CheckConstraintsAgainstExistingSettings(
      const fuchsia_sysmem2::BufferCollectionConstraints& constraints);

 private:
  friend class LogicalBuffer;
  friend class NodeProperties;
  friend class TrackedParentVmo;

  enum class CheckSanitizeStage { kInitial, kNotAggregated, kAggregated };

  class Constraints {
   public:
    Constraints(const Constraints&) = delete;
    Constraints(Constraints&&) = default;
    Constraints(fuchsia_sysmem2::BufferCollectionConstraints constraints,
                std::optional<fuchsia_sysmem2::SingleBufferSettings> must_match_settings,
                const NodeProperties& node_properties)
        : buffer_collection_constraints_(std::move(constraints)),
          must_match_settings_(std::move(must_match_settings)),
          node_properties_(node_properties) {}

    const fuchsia_sysmem2::BufferCollectionConstraints& constraints() const {
      return buffer_collection_constraints_;
    }
    fuchsia_sysmem2::BufferCollectionConstraints& mutate_constraints() {
      return buffer_collection_constraints_;
    }

    const std::optional<fuchsia_sysmem2::SingleBufferSettings>& must_match_settings() const {
      return must_match_settings_;
    }

    const ClientDebugInfo& client_debug_info() const {
      return node_properties_.client_debug_info();
    }
    const NodeProperties& node_properties() const { return node_properties_; }

   private:
    fuchsia_sysmem2::BufferCollectionConstraints buffer_collection_constraints_;
    std::optional<fuchsia_sysmem2::SingleBufferSettings> must_match_settings_;
    const NodeProperties& node_properties_;
  };

  using ConstraintsList = std::list<Constraints>;

  struct CollectionName {
    uint32_t priority{};
    std::string name;
  };

  explicit LogicalBufferCollection(Sysmem* parent_device);

  const UsagePixelFormatCost& usage_pixel_format_cost() {
    return parent_sysmem_->usage_pixel_format_cost();
  }

  // Will log an error, and then FailRoot().
  void LogAndFailRootNode(Location location, fuchsia_sysmem2::Error error, const char* format, ...)
      __PRINTFLIKE(4, 5);
  // This fails the entire LogicalBufferCollection, by failing the root Node, which propagates that
  // failure to the entire Node tree, and also results in zero remaining Node(s).  Then once all
  // outstanding VMOs have been closed, the LogicalBufferCollection is deleted.
  //
  // This cleans out a lot of state that's unnecessary after a failure.
  void FailRootNode(fuchsia_sysmem2::Error error);

  NodeProperties* FindTreeToFail(NodeProperties* failing_node);

  // Fails the tree rooted at tree_to_fail.  The tree_to_fail is allowed to be the root_, or it can
  // be a sub-tree. All the Node(s) from tree_to_fail down are Fail()ed.  Any sysmem(1) Node(s) that
  // still have channels open will send epitaph. Sysmem2 Node(s) always send ZX_ERR_INTERNAL as the
  // epitaph (mainly because an epitaph is required by FIDL bindings). In either case the channel is
  // closed. All Node(s) from tree_to_fail downward are also removed from the tree. If tree_to_fail
  // is the root, then when all child VMOs have been closed, the LogicalBufferCollection will be
  // deleted.
  //
  // Node(s) are Fail()ed and removed from the tree in child-first order.
  //
  // These two are only appropriate to use if it's already known which node should fail.  If it's
  // not yet known which node should fail, call FindTreeToFail() first, or use LogAndFailNode() /
  // FailNode().
  void LogAndFailDownFrom(Location location, NodeProperties* tree_to_fail,
                          fuchsia_sysmem2::Error error, const char* format, ...) __PRINTFLIKE(5, 6);
  void FailDownFrom(NodeProperties* tree_to_fail, fuchsia_sysmem2::Error error);

  // Find the root-most node of member_node's failure domain, and fail that whole failure domain and
  // any of its child failure domains.
  void LogAndFailNode(Location location, NodeProperties* member_node, fuchsia_sysmem2::Error error,
                      const char* format, ...) __PRINTFLIKE(5, 6);
  void FailNode(NodeProperties* member_node, fuchsia_sysmem2::Error error);

  void LogInfo(Location location, const char* format, ...) const;
  static void LogErrorStatic(Location location, const ClientDebugInfo* client_debug_info,
                             const char* format, ...) __PRINTFLIKE(3, 4);

  void LogWarn(Location location, const char* format, ...) const __PRINTFLIKE(3, 4);
  void LogError(Location location, const char* format, ...) const __PRINTFLIKE(3, 4);

  // Don't call these directly; use LogInfo, LogWarn, LogError, or in rare cases
  // LogErrorStatic.
  void VLogWarn(Location location, const char* format, va_list args) const;
  void VLogError(Location location, const char* format, va_list args) const;
  void LogBuffered(fuchsia_logging::LogSeverity log_severity, Location location, const char* format,
                   ...) const __PRINTFLIKE(4, 5);
  void vLogBuffered(fuchsia_logging::LogSeverity severity, const char* file, int line,
                    const char* prefix, const char* format, va_list args) const;

  void ResetGroupChildSelection(std::vector<NodeProperties*>& groups_by_priority);
  void InitGroupChildSelection(std::vector<NodeProperties*>& groups_by_priority);
  void NextGroupChildSelection(std::vector<NodeProperties*> groups_by_priority);
  bool DoneWithGroupChildSelections(std::vector<NodeProperties*> groups_by_priority);

  // The caller must keep "this" alive.  We require this of the caller since the caller is fairly
  // likely to want to keep "this" alive longer than MaybeAllocate() could anyway.
  void MaybeAllocate();

  fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, fuchsia_sysmem2::Error> TryAllocate(
      std::vector<NodeProperties*> nodes);

  std::optional<fuchsia_sysmem2::Error> TryLateLogicalAllocation(
      std::vector<NodeProperties*> nodes);

  fuchsia_sysmem2::BufferCollectionConstraints GetExistingConstraints(
      const fuchsia_sysmem2::SingleBufferSettings& settings,
      std::optional<uint32_t> min_buffer_count, std::optional<uint32_t> max_buffer_count);

  // The tmp_nodes vector must be empty on entry to this method. After calling this method, the
  // caller must keep tmp_nodes alive until after subsequent call to CombineConstraints or any other
  // usage of constraints_list, and should delete the vector soon after that.
  //
  // For any existing entry that has must_match_vmo set, this adds another entry to constraints_list
  // based on the SingleBufferSettings of the must_match_vmo's collection. This does not add any
  // constraints on the buffer count.
  bool AugmentConstraintsList(ConstraintsList& constraints_list,
                              std::vector<std::unique_ptr<NodeProperties>>& tmp_nodes);

  zx::result<bool> CompareBufferCollectionInfo(fuchsia_sysmem2::BufferCollectionInfo& lhs,
                                               fuchsia_sysmem2::BufferCollectionInfo& rhs);

  void InitializeConstraintSnapshots(const ConstraintsList& constraints_list);

  void SetFailedAllocationResult(fuchsia_sysmem2::Error error);

  void SetAllocationResult(std::vector<NodeProperties*> visible_pruned_sub_tree,
                           fuchsia_sysmem2::BufferCollectionInfo info,
                           std::vector<NodeProperties*> whole_pruned_sub_tree);

  void SendAllocationResult(std::vector<NodeProperties*> nodes);

  void SetFailedLateLogicalAllocationResult(NodeProperties* tree,
                                            fuchsia_sysmem2::Error status_param);

  void SetSucceededLateLogicalAllocationResult(std::vector<NodeProperties*> visible_pruned_sub_tree,
                                               std::vector<NodeProperties*> whole_pruned_sub_tree);

  void BindSharedCollectionInternal(BufferCollectionToken* token,
                                    CollectionServerEnd buffer_collection_request);

  fpromise::result<fuchsia_sysmem2::BufferCollectionConstraints, void> CombineConstraints(
      ConstraintsList* constraints_list);

  bool CheckSanitizeBufferCollectionConstraints(
      CheckSanitizeStage stage, fuchsia_sysmem2::BufferCollectionConstraints& constraints);

  bool CheckSanitizeBufferUsage(CheckSanitizeStage stage,
                                fuchsia_sysmem2::BufferUsage& buffer_usage);

  bool CheckSanitizeHeap(CheckSanitizeStage stage, fuchsia_sysmem2::Heap& heap);

  bool CheckSanitizeBufferMemoryConstraints(CheckSanitizeStage stage,
                                            const fuchsia_sysmem2::BufferUsage& buffer_usage,
                                            fuchsia_sysmem2::BufferMemoryConstraints& constraints);

  bool CheckSanitizeImageFormatConstraints(CheckSanitizeStage stage,
                                           const fuchsia_sysmem2::BufferUsage& buffer_usage,
                                           fuchsia_sysmem2::ImageFormatConstraints& constraints);

  bool AccumulateConstraintBufferCollection(fuchsia_sysmem2::BufferCollectionConstraints* acc,
                                            fuchsia_sysmem2::BufferCollectionConstraints c);

  bool AccumulateConstraintsBufferUsage(fuchsia_sysmem2::BufferUsage* acc,
                                        fuchsia_sysmem2::BufferUsage c);

  bool AccumulateConstraintPermittedHeaps(std::vector<fuchsia_sysmem2::Heap>* acc,
                                          std::vector<fuchsia_sysmem2::Heap> c);

  bool AccumulateConstraintBufferMemory(fuchsia_sysmem2::BufferMemoryConstraints* acc,
                                        fuchsia_sysmem2::BufferMemoryConstraints c);

  bool AccumulateConstraintImageFormats(std::vector<fuchsia_sysmem2::ImageFormatConstraints>* acc,
                                        std::vector<fuchsia_sysmem2::ImageFormatConstraints> c);

  bool AccumulateConstraintImageFormat(fuchsia_sysmem2::ImageFormatConstraints* acc,
                                       fuchsia_sysmem2::ImageFormatConstraints c);

  bool AccumulateConstraintColorSpaces(std::vector<fuchsia_images2::ColorSpace>* acc,
                                       std::vector<fuchsia_images2::ColorSpace> c);

  size_t InitialCapacityOrZero(CheckSanitizeStage stage, size_t initial_capacity);

  bool IsColorSpaceEqual(const fuchsia_images2::ColorSpace& a,
                         const fuchsia_images2::ColorSpace& b);

  fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, zx_status_t>
  GenerateUnpopulatedBufferCollectionInfo(
      const fuchsia_sysmem2::BufferCollectionConstraints& constraints);

  fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, fuchsia_sysmem2::Error> Allocate(
      const fuchsia_sysmem2::BufferCollectionConstraints& constraints,
      fuchsia_sysmem2::BufferCollectionInfo* buffer_collection_info);

  fpromise::result<zx::vmo> AllocateVmo(MemoryAllocator* allocator,
                                        const fuchsia_sysmem2::SingleBufferSettings& settings,
                                        std::optional<uint64_t> min_physical_alignment,
                                        uint32_t index);

  int32_t CompareImageFormatConstraintsTieBreaker(const fuchsia_sysmem2::ImageFormatConstraints& a,
                                                  const fuchsia_sysmem2::ImageFormatConstraints& b);

  int32_t CompareImageFormatConstraintsByIndex(
      const fuchsia_sysmem2::BufferCollectionConstraints& constraints, uint32_t index_a,
      uint32_t index_b);

  void CreationTimedOut(async_dispatcher_t* dispatcher, async::TaskBase* task, zx_status_t status);

  AllocationResult allocation_result();

  void LogBufferCollectionInfoDiffs(const fuchsia_sysmem2::BufferCollectionInfo& o,
                                    const fuchsia_sysmem2::BufferCollectionInfo& n);

  // To be called only by CombineConstraints().
  bool IsMinBufferSizeSpecifiedByAnyParticipant(const ConstraintsList& constraints_list);

  // This is the root and any sub-trees created via SetDispensable() or AttachToken().
  std::vector<NodeProperties*> FailureDomainSubtrees();

  // A Node is a pruned sub-tree eligible for logical allocation if it is not yet logically
  // allocated, and does not have a non-logically-allocated direct parent.
  //
  // Such sub-trees will always have a root-most Node that is either the root or an
  // ErrorPropagationMode kDoNotPropagate Node.
  //
  // The returned pruned sub-trees may not be ready for logical allocation (may not have all its
  // constraints yet), nor do they necessarily have constraints that are satisfiable.
  std::vector<NodeProperties*> PrunedSubtreesEligibleForLogicalAllocation();

  // The granularity of logical allocation is a sub-tree pruned of any ErrorPropagationMode
  // kDoNotPropagate Node(s) other than the sub-tree's root-most node.
  //
  // The root-most Node of a pruned subtree can be the root, or it can be a Node that has
  // ErrorPropagationMode kDoNotPropagate (implying that AttachToken() was used to create the
  // non-root Node).
  //
  // The pruning is the same for both the root and a non-root Node so that the locally-observable
  // (by a participant) logical allocation granularity is the same regardless of whether the logical
  // allocation granule has the root as its root-most Node, or has an ErrorPropagationMode
  // kDoNotPropagate Node as its root-most Node.
  std::vector<NodeProperties*> NodesOfPrunedSubtreeEligibleForLogicalAllocation(
      NodeProperties& subtree);

  std::vector<NodeProperties*> PrioritizedGroupsOfPrunedSubtreeEligibleForLogicalAllocation(
      NodeProperties& subtree);

  // For NodeProperties:
  void AddCountsForNode(const Node& node);
  void RemoveCountsForNode(const Node& node);
  void AdjustRelevantNodeCounts(const Node& node, fit::function<void(uint32_t& count)> visitor);
  void DeleteRoot();

  // Diff printing.

#if defined(PRINT_DIFF)
#error "Let's pick a different name for PRINT_DIFF"
#endif
  // If LLCPP had generic field objects, we wouldn't need this macro.  We #undef the macro name
  // after we're done using it so it doesn't escape this header.
#define PRINT_DIFF(child_field_name)                                                               \
  do {                                                                                             \
    const std::string& parent_field_name = field_name;                                             \
    if (o.child_field_name().has_value() != n.child_field_name().has_value()) {                    \
      LogError(FROM_HERE,                                                                          \
               "o%s." #child_field_name                                                            \
               "().has_value(): %d "                                                               \
               "n%s." #child_field_name "().has_value(): %d",                                      \
               parent_field_name.c_str(), o.child_field_name().has_value(),                        \
               parent_field_name.c_str(), n.child_field_name().has_value());                       \
    } else if (o.child_field_name().has_value()) {                                                 \
      std::string field_name =                                                                     \
          fbl::StringPrintf("%s.%s()", parent_field_name.c_str(), #child_field_name).c_str();      \
      DiffPrinter<std::remove_const_t<std::remove_reference_t<decltype(*o.child_field_name())>>>:: \
          PrintDiff(*this, field_name, *o.child_field_name(), *n.child_field_name());              \
    }                                                                                              \
  } while (false)

  template <typename FieldType, typename Enable = void>
  struct DiffPrinter {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const FieldType& o, const FieldType& n);
  };
  template <typename ElementType>
  struct DiffPrinter<std::vector<ElementType>, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const std::vector<ElementType>& o,
                          const std::vector<ElementType>& n) {
      if (o.size() != n.size()) {
        buffer_collection.LogError(FROM_HERE, "o%s.size(): %" PRIu64 " n%s.size(): %" PRIu64,
                                   field_name.c_str(), o.size(), field_name.c_str(), n.size());
      }
      for (uint32_t i = 0; i < std::max(o.size(), n.size()); ++i) {
        std::string new_field_name = fbl::StringPrintf("%s[%u]", field_name.c_str(), i).c_str();
        const ElementType& blank = ElementType();
        const ElementType& o_element = i < o.size() ? o[i] : blank;
        const ElementType& n_element = i < n.size() ? n[i] : blank;
        DiffPrinter<ElementType>::PrintDiff(buffer_collection, new_field_name, o_element,
                                            n_element);
      }
    }
  };
  template <typename TableType>
  struct DiffPrinter<TableType, std::enable_if_t<fidl::IsTable<TableType>::value>> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const TableType& o, const TableType& n) {
      buffer_collection.LogTableDiffs<TableType>(field_name, o, n);
    }
  };
  template <>
  struct DiffPrinter<bool, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const bool& o, const bool& n) {
      if (o != n) {
        buffer_collection.LogError(FROM_HERE, "o%s: %d n%s: %d", field_name.c_str(), o,
                                   field_name.c_str(), n);
      }
    }
  };
  template <>
  struct DiffPrinter<uint32_t, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const uint32_t& o, const uint32_t& n) {
      if (o != n) {
        buffer_collection.LogError(FROM_HERE, "o%s: %u n%s: %u", field_name.c_str(), o,
                                   field_name.c_str(), n);
      }
    }
  };
  template <typename EnumType>
  struct DiffPrinter<EnumType, std::enable_if_t<sysmem::IsFidlEnum_v<EnumType>>> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const EnumType& o, const EnumType& n) {
      using UnderlyingType = sysmem::FidlUnderlyingTypeOrType_t<EnumType>;
      const UnderlyingType o_underlying = safe_cast<UnderlyingType>(o);
      const UnderlyingType n_underlying = safe_cast<UnderlyingType>(n);
      DiffPrinter<UnderlyingType>::PrintDiff(buffer_collection, field_name, o_underlying,
                                             n_underlying);
    }
  };
  template <>
  struct DiffPrinter<uint64_t, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const uint64_t& o, const uint64_t& n) {
      if (o != n) {
        buffer_collection.LogError(FROM_HERE, "o%s: %" PRIu64 " n%s: %" PRIu64, field_name.c_str(),
                                   o, field_name.c_str(), n);
      }
    }
  };
  template <>
  struct DiffPrinter<zx::vmo, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const zx::vmo& o, const zx::vmo& n) {
      // We don't expect to call the zx::vmo variant since !has_vmo(), but if we do get here,
      // complain + print the values regardless of what the values are or whether they differ.
      buffer_collection.LogError(FROM_HERE,
                                 "Why did we call zx::vmo PrintDiff? --- o%s: %u n%s: %u",
                                 field_name.c_str(), o.get(), field_name.c_str(), n.get());
    }
  };
  template <>
  struct DiffPrinter<zx::eventpair, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const zx::eventpair& o,
                          const zx::eventpair& n) {
      // We don't expect to call the zx::eventpair variant since !has_close_weak_asap(), but if we
      // do get here, complain + print the values regardless of what the values are or whether they
      // differ.
      buffer_collection.LogError(FROM_HERE,
                                 "Why did we call zx::eventpair PrintDiff? --- o%s: %u n%s: %u",
                                 field_name.c_str(), o.get(), field_name.c_str(), n.get());
    }
  };
  template <>
  struct DiffPrinter<fuchsia_math::SizeU, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const fuchsia_math::SizeU& o,
                          const fuchsia_math::SizeU& n) {
      if (o.width() != n.width() || o.height() != n.height()) {
        buffer_collection.LogError(FROM_HERE, "o%s: %u x %u n%s: %u x %u", field_name.c_str(),
                                   o.width(), o.height(), field_name.c_str(), n.width(),
                                   n.height());
      }
    }
  };
  template <>
  struct DiffPrinter<std::string, void> {
    static void PrintDiff(const LogicalBufferCollection& buffer_collection,
                          const std::string& field_name, const std::string& o,
                          const std::string& n) {
      if (o != n) {
        buffer_collection.LogError(FROM_HERE, "o%s: %s n%s: %s", field_name.c_str(), o.c_str(),
                                   field_name.c_str(), n.c_str());
      }
    }
  };

  template <typename Table>
  void LogTableDiffs(const std::string& field_name, const Table& o, const Table& n) const;

  template <>
  void LogTableDiffs<fuchsia_sysmem2::BufferMemorySettings>(
      const std::string& field_name, const fuchsia_sysmem2::BufferMemorySettings& o,
      const fuchsia_sysmem2::BufferMemorySettings& n) const {
    PRINT_DIFF(size_bytes);
    PRINT_DIFF(is_physically_contiguous);
    PRINT_DIFF(is_secure);
    PRINT_DIFF(coherency_domain);
    PRINT_DIFF(heap);
  }

  template <>
  void LogTableDiffs<fuchsia_sysmem2::ImageFormatConstraints>(
      const std::string& field_name, const fuchsia_sysmem2::ImageFormatConstraints& o,
      const fuchsia_sysmem2::ImageFormatConstraints& n) const {
    PRINT_DIFF(pixel_format);
    PRINT_DIFF(pixel_format_modifier);
    PRINT_DIFF(color_spaces);
    PRINT_DIFF(min_size);
    PRINT_DIFF(max_size);
    PRINT_DIFF(min_bytes_per_row);
    PRINT_DIFF(max_bytes_per_row);
    PRINT_DIFF(max_width_times_height);
    PRINT_DIFF(size_alignment);
    PRINT_DIFF(display_rect_alignment);
    PRINT_DIFF(required_min_size);
    PRINT_DIFF(required_max_size);
    PRINT_DIFF(required_max_size_list);
    PRINT_DIFF(bytes_per_row_divisor);
    PRINT_DIFF(start_offset_divisor);
  }

  template <>
  void LogTableDiffs<fuchsia_sysmem2::SingleBufferSettings>(
      const std::string& field_name, const fuchsia_sysmem2::SingleBufferSettings& o,
      const fuchsia_sysmem2::SingleBufferSettings& n) const {
    PRINT_DIFF(buffer_settings);
    PRINT_DIFF(image_format_constraints);
  }

  template <>
  void LogTableDiffs<fuchsia_sysmem2::VmoBuffer>(const std::string& field_name,
                                                 const fuchsia_sysmem2::VmoBuffer& o,
                                                 const fuchsia_sysmem2::VmoBuffer& n) const {
    PRINT_DIFF(vmo);
    PRINT_DIFF(vmo_usable_start);
    PRINT_DIFF(close_weak_asap);
  }

  template <>
  void LogTableDiffs<fuchsia_sysmem2::BufferCollectionInfo>(
      const std::string& field_name, const fuchsia_sysmem2::BufferCollectionInfo& o,
      const fuchsia_sysmem2::BufferCollectionInfo& n) const {
    PRINT_DIFF(settings);
    PRINT_DIFF(buffers);
  }

  template <>
  void LogTableDiffs<fuchsia_sysmem2::Heap>(const std::string& field_name,
                                            const fuchsia_sysmem2::Heap& o,
                                            const fuchsia_sysmem2::Heap& n) const {
    PRINT_DIFF(heap_type);
    PRINT_DIFF(id);
  }
#undef PRINT_DIFF

  void LogDiffsBufferCollectionInfo(const fuchsia_sysmem2::BufferCollectionInfo& o,
                                    const fuchsia_sysmem2::BufferCollectionInfo& n) const {
    LOG(WARNING, "LogDiffsBufferCollectionInfo()");
    LogTableDiffs<fuchsia_sysmem2::BufferCollectionInfo>("", o, n);
  }

  void LogConstraints(Location location, NodeProperties* node_properties,
                      const fuchsia_sysmem2::BufferCollectionConstraints& constraints) const;
  void LogBufferCollectionInfo(IndentTracker& indent_tracker,
                               const fuchsia_sysmem2::BufferCollectionInfo& bci) const;
  void LogPixelFormatAndModifier(
      IndentTracker& indent_tracker, NodeProperties* node_properties,
      const fuchsia_sysmem2::PixelFormatAndModifier& pixel_format_and_modifier) const;
  void LogImageFormatConstraints(IndentTracker& indent_tracker, NodeProperties* node_properties,
                                 const fuchsia_sysmem2::ImageFormatConstraints& ifc) const;
  void LogPrunedSubTree(NodeProperties* subtree) const;
  void LogNodeConstraints(std::vector<NodeProperties*> nodes) const;

  // subtree must remain alive >= returned filter
  fit::function<NodeFilterResult(const NodeProperties&)> PrunedSubtreeFilter(
      NodeProperties& subtree, fit::function<bool(const NodeProperties&)> visit_keep) const;

  static fbl::RefPtr<LogicalBufferCollection> CommonCreate(
      Sysmem* parent_device, const ClientDebugInfo* client_debug_info);

  bool CommonCreateBufferCollectionTokenStage1(fbl::RefPtr<LogicalBufferCollection> self,
                                               NodeProperties* new_node_properties,
                                               const TokenServerEnd& token_request,
                                               BufferCollectionToken** out_token);

  void HandleTokenFailure(BufferCollectionToken& token, zx_status_t status);

  void IncStrongNodeTally();
  void DecStrongNodeTally();
  void CheckForZeroStrongNodes();

  void IncStrongParentVmoCount();
  void DecStrongParentVmoCount();
  void CheckForZeroStrongParentVmoCount();

  void CreateParentVmoInspect(zx_koid_t parent_vmo_koid);

  void ClearBuffers();

  bool FlattenPixelFormatAndModifiers(const fuchsia_sysmem2::BufferUsage& buffer_usage,
                                      fuchsia_sysmem2::BufferCollectionConstraints& constraints);

  bool IsSecureHeap(const fuchsia_sysmem2::Heap& heap);
  bool IsSecurePermitted(const fuchsia_sysmem2::BufferMemoryConstraints& constraints);
  fpromise::result<fuchsia_sysmem2::Heap, zx_status_t> GetHeap(
      const fuchsia_sysmem2::BufferMemoryConstraints& constraints, Sysmem* device);

  Sysmem* parent_sysmem_ = nullptr;

  // This owns the current tree of BufferCollectionToken, BufferCollection, OrphanedNode.  The
  // Location in the tree is determined by creation path.  Child Node(s) are children because they
  // were created via their parent node.  The root node is created by CreateSharedCollection() which
  // also creates the LogicalBufferCollection.
  //
  // We use shared_ptr<> to manage NodeProperties only so that Node can have a std::weak_ptr<> back
  // to NodeProperties instead of a raw pointer, since the Node can last longer than NodeProperties,
  // despite NodeProperties being the primary non-transient owner of Node.  This way we avoid using
  // a raw pointer from Node to NodeProperties.
  //
  // The tree at root_ is the only non-transient ownership of NodeProperties.  Transient ownership
  // lasts only during a portion of a single dispatch on parent_device_->dispatcher().
  std::shared_ptr<NodeProperties> root_;

  std::vector<ConstraintInfoSnapshot> constraints_at_allocation_;

  bool is_allocate_attempted_ = false;

  // Iff true, initial allocation has been attempted and has succeeded or
  // failed.  Both allocation_result_status_ and allocation_result_info_ are
  // not meaningful until has_allocation_result_ is true.
  bool has_allocation_result_ = false;
  std::optional<fuchsia_sysmem2::BufferCollectionInfo> buffer_collection_info_before_population_;
  std::optional<fuchsia_sysmem2::Error> maybe_allocation_error_;
  std::optional<fuchsia_sysmem2::BufferCollectionInfo> allocation_result_info_;

  MemoryAllocator* memory_allocator_ = nullptr;
  std::optional<CollectionName> name_;

  // 0 means not dispensable.  Non-zero means dispensable, with each value being a group of
  // BufferCollectionToken(s) / BufferCollection(s) that were all created from a single
  // BufferCollectionToken that was created with AttachToken() or which had SetDispensable() called
  // on it.  Each group's constraints are aggregated together and succeed or fail to logically
  // allocate as a group, considered in order by when each group's constraints are ready, not in
  // order by dispensable_ordinal values.
  uint64_t next_dispensable_ordinal_ = 1;

  // Information about the current client - only valid while aggregating state for a particular
  // client.
  const NodeProperties* current_node_properties_ = nullptr;

  inspect::Node inspect_node_;
  inspect::StringProperty name_property_;
  inspect::UintProperty vmo_count_property_;
  inspect::ValueList vmo_properties_;

  // This does not actually need to be a koid, but for now we do get the value from a koid, so we
  // want the initial value at the start of the constructor to be the invalid koid value, until we
  // set this to a real koid value that's unique to "this".
  uint64_t buffer_collection_id_ = ZX_KOID_INVALID;

  // From buffers_remaining to server_end.
  std::multimap<uint32_t, zx::eventpair> lifetime_tracking_;

  // The key is buffer_index. In the success case, removing a single item from this map is performed
  // by TrackedParentVmo::do_delete. Because do_delete also runs when we want to just clear buffers_
  // in an error path, when we clear buffers_ we move it out, clear it, then delete the moved-out
  // items. This way we don't try to mutate buffers_ during buffers_.clear().
  std::unordered_map<uint32_t, std::unique_ptr<LogicalBuffer>> buffers_;

  // It's nice for members containing timers to be last for destruction order purposes, but the
  // destructor also explicitly cancels timers to avoid any brittle-ness from members potentially
  // added below creation_timer_ (so this doesn't actually need to be last).
  async::TaskMethod<LogicalBufferCollection, &LogicalBufferCollection::CreationTimedOut>
      creation_timer_{this};
  uint32_t creation_timer_run_count_ = 0;

  bool is_verbose_logging_ = false;

  // Only tracked while Node::is_currently_connected() true, to allow for Node sub-classes that may
  // stick around for a short duration after their channel is closed, depending on FIDL C++
  // generated code requirements on how close / disconnect works.
  std::unordered_map<zx_koid_t, NodeProperties*> node_properties_by_node_ref_keep_koid_;

  bool done_with_group_child_selection_ = false;

  // This can become true before initial allocation if waiting on secure allocators at least once
  // is/was necessary. Once this becomes true it stays true, even after this collection is no longer
  // currently waiting on secure allocators.
  //
  // This field helps avoid creation of extra child inspect nodes when the wait is over.
  bool waiting_for_secure_allocators_ready_ = false;

  uint32_t strong_node_count_ = 0;
  uint32_t strong_parent_vmo_count_ = 0;

  const zx::time create_time_monotonic_ = zx::time::infinite_past();

  class BufferedLogEntry {
   public:
    BufferedLogEntry() = delete;
    BufferedLogEntry(fuchsia_logging::LogSeverity severity, const char* file, int line,
                     std::string formatted_str)
        : severity_(severity), file_(file), line_(line), formatted_str_(std::move(formatted_str)) {}
    // move-only
    BufferedLogEntry(const BufferedLogEntry& to_copy) = delete;
    BufferedLogEntry& operator=(const BufferedLogEntry& to_copy) = delete;
    BufferedLogEntry(BufferedLogEntry&& to_move) = default;
    BufferedLogEntry& operator=(BufferedLogEntry&& to_move) = default;
    fuchsia_logging::LogSeverity severity() { return severity_; }
    const char* file() { return file_; }
    int line() { return line_; }
    const std::string& formatted_str() { return formatted_str_; }

   private:
    fuchsia_logging::LogSeverity severity_ = fuchsia_logging::LogSeverity::Trace;
    const char* file_ = nullptr;
    int line_ = 0;
    std::string formatted_str_;
  };
  std::optional<std::vector<BufferedLogEntry>> log_buffer_;
  const LogCallback maybe_buffer_log_ = [this](fuchsia_logging::LogSeverity severity,
                                               const char* file, int line,
                                               const char* formatted_str) {
    if (log_buffer_.has_value()) {
      log_buffer_->emplace_back(BufferedLogEntry{severity, file, line, formatted_str});
      return;
    }
    GetDefaultLogCallback()(severity, file, line, formatted_str);
  };
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_LOGICAL_BUFFER_COLLECTION_H_
