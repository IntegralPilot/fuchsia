// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/owned_wait_queue.h"

#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <zircon/compiler.h>

#include <arch/mp.h>
#include <fbl/algorithm.h>
#include <fbl/enum_bits.h>
#include <ffl/string.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/preempt_disabled_token.h>
#include <kernel/scheduler.h>
#include <kernel/scheduler_state.h>
#include <kernel/wait_queue_internal.h>
#include <ktl/algorithm.h>
#include <ktl/bit.h>
#include <ktl/type_traits.h>
#include <object/thread_dispatcher.h>

#include <ktl/enforce.h>

// Notes on the defined kernel counters.
//
// Adjustments (aka promotions and demotions)
// The number of times that a thread either gained or lost inherited profile
// pressure as a result of a PI event.
//
// Note that the number of promotions does not have to equal the number of
// demotions in the system.  For example, a thread could slowly gain weight as
// fair scheduled threads join a wait queue it owns, then suddenly drop
// back down to its base profile when the thread releases ownership of the
// queue.
//
// In addition to simple promotions and demotions, the number of threads whose
// effective profile changed as a result of another thread's base profile
// changing is also tracked, although whether these changes amount to a
// promotion or a demotion is not computed.
//
// Max chain traversal.
// The maximum traversed length of a PI chain during execution of the propagation
// algorithm.
//
// The length of a propagation chain is defined as the number of nodes in an
// inheritance graph which are affected by a propagation event.  For example, if
// a thread (T1) blocks in an owned wait queue (OWQ1), adding an edge between
// them, and the wait queue has no owner, then the propagation event's chain
// length is 1. This is regardless of whether or not the blocking thread
// currently owns one or more wait queues upstream from it. The OWQ1's IPVs were
// updated, but no other nodes in the graph were affected.  If the OWQ1 had been
// owned by a running/runnable thread (T2), then the chain length of the
// operation would have been two instead, since both OWQ1 and T2 needed to be
// visited and updated.
KCOUNTER(pi_promotions, "kernel.pi.adj.promotions")
KCOUNTER(pi_demotions, "kernel.pi.adj.demotions")
KCOUNTER(pi_bp_changed, "kernel.pi.adj.bp_changed")
KCOUNTER_DECLARE(max_pi_chain_traverse, "kernel.pi.max_chain_traverse", Max)

namespace {

enum class ChainLengthTrackerOpt : uint32_t {
  None = 0,
  RecordMaxLength = 1,
  EnforceLengthGuard = 2,
};
FBL_ENABLE_ENUM_BITS(ChainLengthTrackerOpt)

// By default, we always maintain the max length counter, and we enforce the
// length guard in everything but release builds.
constexpr ChainLengthTrackerOpt kEnablePiChainGuards =
    ((LK_DEBUGLEVEL > 0) ? ChainLengthTrackerOpt::EnforceLengthGuard : ChainLengthTrackerOpt::None);

constexpr ChainLengthTrackerOpt kDefaultChainLengthTrackerOpt =
    ChainLengthTrackerOpt::RecordMaxLength | kEnablePiChainGuards;

template <ChainLengthTrackerOpt Options = kDefaultChainLengthTrackerOpt>
class ChainLengthTracker {
 public:
  using Opt = ChainLengthTrackerOpt;

  ChainLengthTracker() {
    if constexpr (Options != Opt::None) {
      nodes_visited_ = 0;
    }
  }

  ~ChainLengthTracker() {
    if constexpr ((Options & Opt::EnforceLengthGuard) != Opt::None) {
      // Note, the only real reason that this is an accurate max at all is
      // because the counter is effectively protected by the thread lock
      // (although there is no real good way to annotate that fact).  When we
      // finally remove the thread lock, we are going to need to do better than
      // this.
      auto old = max_pi_chain_traverse.ValueCurrCpu();
      if (nodes_visited_ > old) {
        max_pi_chain_traverse.Set(nodes_visited_);
      }
    }
  }

  void NodeVisited() {
    if constexpr (Options != Opt::None) {
      ++nodes_visited_;
    }

    if constexpr ((Options & Opt::EnforceLengthGuard) != Opt::None) {
      constexpr uint32_t kMaxChainLen = 2048;
      ASSERT_MSG(nodes_visited_ <= kMaxChainLen, "visited %u", nodes_visited_);
    }
  }

 private:
  uint32_t nodes_visited_ = 0;
};

using AddSingleEdgeTag = decltype(OwnedWaitQueue::AddSingleEdgeOp);
using RemoveSingleEdgeTag = decltype(OwnedWaitQueue::RemoveSingleEdgeOp);
using BaseProfileChangedTag = decltype(OwnedWaitQueue::BaseProfileChangedOp);

inline bool IpvsAreConsequential(const SchedulerState::InheritedProfileValues* ipvs) {
  return ipvs != nullptr && ipvs->is_consequential();
}

template <typename UpstreamType, typename DownstreamType>
void Propagate(UpstreamType& upstream, DownstreamType& downstream, AddSingleEdgeTag,
               ForceInheritance force_inheritance,
               const SchedulerState::EffectiveProfile* old_target_ep = nullptr)
    TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(upstream),
           ChainLockable::GetLock(downstream), preempt_disabled_token) {
  Scheduler::JoinNodeToPiGraph(upstream, downstream, force_inheritance);
  if constexpr (ktl::is_same_v<DownstreamType, Thread>) {
    pi_promotions.Add(1u);
  }
}

template <typename UpstreamType, typename DownstreamType>
void Propagate(UpstreamType& upstream, DownstreamType& downstream, RemoveSingleEdgeTag,
               ForceInheritance force_inheritance,
               const SchedulerState::EffectiveProfile* old_target_ep = nullptr)
    TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(upstream),
           ChainLockable::GetLock(downstream), preempt_disabled_token) {
  Scheduler::SplitNodeFromPiGraph(upstream, downstream, old_target_ep);
  if constexpr (ktl::is_same_v<DownstreamType, Thread>) {
    pi_demotions.Add(1u);
  }
}

template <typename UpstreamType, typename DownstreamType>
void Propagate(UpstreamType& upstream, DownstreamType& downstream, BaseProfileChangedTag,
               ForceInheritance force_inheritance,
               const SchedulerState::EffectiveProfile* old_target_ep = nullptr)
    TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(upstream),
           ChainLockable::GetLock(downstream), preempt_disabled_token) {
  Scheduler::UpstreamThreadBaseProfileChanged(upstream, downstream);
  if constexpr (ktl::is_same_v<DownstreamType, Thread>) {
    pi_bp_changed.Add(1u);
  }
}

}  // namespace

SchedulerState::EffectiveProfile OwnedWaitQueue::GetEffectiveProfile() const {
  SchedulerState::EffectiveProfile ep{};
  if (inherited_scheduler_state_storage() != nullptr) {
    const auto& iss = *inherited_scheduler_state_storage();
    iss.ipvs.AssertConsistency();
    if (iss.ipvs.uncapped_utilization > SchedUtilization{0}) {
      const SchedUtilization utilization =
          ktl::min(Scheduler::kThreadUtilizationMax, iss.ipvs.uncapped_utilization);
      const SchedDuration capacity = utilization * iss.ipvs.min_deadline;
      ep.SetDeadline({capacity, iss.ipvs.min_deadline, utilization}, iss.ipvs.critical_count > 0);
    } else {
      ep.SetFair(iss.ipvs.total_weight);
    }
  }
  return ep;
}

OwnedWaitQueue::~OwnedWaitQueue() {
  // Something is very very wrong if we have been allowed to destruct while we
  // still have an owner.
  DEBUG_ASSERT(owner_ == nullptr);
}

void OwnedWaitQueue::DisownAllQueues(Thread* t) {
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
    // We should have cleared the "can own wait queues" flag by now, which
    // should guarantee that we cannot possibly become the owner of any new
    // queues.  Our state, however, should still be "RUNNING".  Once we have
    // finally emptied the our list of owned queues, the exiting thread can
    // fully transition to the DEATH state.
    ChainLockGuard guard{t->get_lock()};
    DEBUG_ASSERT(t->can_own_wait_queues_ == false);
    DEBUG_ASSERT(t->scheduler_state().state() == THREAD_RUNNING);
    DEBUG_ASSERT(t->wait_queue_state_.blocking_wait_queue_ == nullptr);

    while (!t->owned_wait_queues_.is_empty()) {
      OwnedWaitQueue& queue = t->owned_wait_queues_.front();

      if (!queue.get_lock().AcquireOrBackoff()) {
        return ChainLockTransaction::Action::Backoff;
      }

      queue.ResetOwner(*t);
      queue.get_lock().Release();
    }

    // Finalization is a bit strange here.  Typically, we obtain all of the
    // locks we will need for an operation, finalize our transaction, and then
    // actually perform the operation.  In other words, aside from gathering our
    // locks, we are doing no work in the pre-finalization stage.
    //
    // In this case, we are unlinking |t| from all of the queues is currently
    // owns; obtaining each queue lock as we go, doing the actual work of
    // unlinking the queue, and then dropping the queue lock before proceeding
    // to the next.  So, we are doing actual work in what would have otherwise
    // been the "pre-finalization" stage of things.
    //
    // So, when looking at lock contention statistics for this transaction,
    // users need to take into account that contention times include all of the
    // actual work done in the operation, not just the lock collection phase.
    ChainLockTransaction::Finalize();

    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(IrqSaveOption, CLT_TAG("OwnedWaitQueue::DisownAllQueues"),
                                  do_transaction);
}

OwnedWaitQueue::WakeThreadsResult OwnedWaitQueue::WakeThreadsLocked(
    Thread::UnblockList threads, IWakeRequeueHook& wake_hooks, WakeOption option,
    ForceInheritance force_inheritance) {
  DEBUG_ASSERT(magic() == kOwnedMagic);
  uint32_t woken{0};

  // Start by removing any existing owner.  We will either wake a thread an make
  // it the new owner, or we are simply going to remove the owner entirely.
  // Once we have removed the old owner, we can unlock the PI chain starting
  // from the old owner (if any)
  if (Thread* const old_owner = owner_; old_owner != nullptr) {
    AssignOwnerInternal(nullptr);
    if (old_owner) {
      old_owner->get_lock().AssertAcquired();
      UnlockPiChainCommon(*old_owner);
    }
  }

  // Now wake all of the threads we had selected
  while (!threads.is_empty()) {
    Thread* const t = threads.pop_front();
    t->get_lock().AssertAcquired();

    // Call the user's hook, allowing them to maintain any internal bookkeeping
    // they need to maintain.
    wake_hooks.OnWakeOrRequeue(*t);

    // Remove this thread's contributions to our IPVs, removing it from our
    // collection in the process.
    const SchedulerState::EffectiveProfile old_target_ep = GetEffectiveProfile();
    BeginPropagate(*t, *this, RemoveSingleEdgeOp, force_inheritance, &old_target_ep);
    Dequeue(t, ZX_OK);
    UpdateSchedStateStorageThreadRemoved(*t);

    // If we still have blocked threads, and we are supposed to make this thread
    // our owner, do so now.  We removed our existing owner at the start of this
    // operation, and it is illegal to attempt to wake multiple threads and make
    // them each our owner, so our owner should still be nullptr.
    //
    // TODO(johngro): We may need a tweak to the propagation.  Our thread is in
    // an intermediate state right now.  It has been removed from its wait
    // queue, but its state is still blocked since it has not been through
    // Scheduler::Unblock yet.  The easiest thing to do might be to just
    // directly assign our IPVs from our current wait queue stats and set owner_
    // directly.
    if (!IsEmpty() && (option == WakeOption::AssignOwner)) {
      DEBUG_ASSERT(owner_ == nullptr);
      AssignOwnerInternal(t);
    }

    // Finally, unblock the thread, drop its lock in the process.
    Scheduler::Unblock(t);
    ++woken;
  }

  // If our WakeOption was None, we should no longer have an owner.  Otherwise,
  // it was AssignOwner, in which case we should only have an owner if we still
  // have blocked threads.
  DEBUG_ASSERT(((option == WakeOption::None) && (owner_ == nullptr)) ||
               ((option == WakeOption::AssignOwner) && (!IsEmpty() || (owner_ == nullptr))));

  ValidateSchedStateStorage();
  return WakeThreadsResult{.woken = woken, .still_waiting = Count(), .owner = owner_};
}

void OwnedWaitQueue::ValidateSchedStateStorageUnconditional() {
  if (inherited_scheduler_state_storage_ != nullptr) {
    bool found = false;
    for (const Thread& t : this->collection_.threads()) {
      AssertInWaitQueue(t, *this);
      if (&t.wait_queue_state().inherited_scheduler_state_storage_ ==
          inherited_scheduler_state_storage_) {
        found = true;
        break;
      }
    }
    DEBUG_ASSERT(found);
  } else {
    DEBUG_ASSERT(this->IsEmpty());
  }
}

SchedulerState::InheritedProfileValues OwnedWaitQueue::SnapshotThreadIpv(
    Thread& thread, ForceInheritance force_inheritance) {
  const SchedulerState& tss = thread.scheduler_state();
  SchedulerState::InheritedProfileValues ret = tss.inherited_profile_values_;
  const SchedulerState::BaseProfile& bp = tss.base_profile_;

  if (bp.inheritable || force_inheritance == ForceInheritance::Yes) {
    if (bp.discipline == SchedDiscipline::Fair) {
      ret.total_weight += bp.fair.weight;
    } else {
      ret.AssertConsistency();
      ret.uncapped_utilization += bp.deadline.utilization;
      ret.min_deadline = ktl::min(ret.min_deadline, bp.deadline.deadline_ns);
      if (bp.critical) {
        ret.critical_count++;
      }
    }
  }

  return ret;
}

void OwnedWaitQueue::ApplyIpvDeltaToThread(const SchedulerState::InheritedProfileValues* old_ipv,
                                           const SchedulerState::InheritedProfileValues* new_ipv,
                                           Thread& thread) {
  DEBUG_ASSERT((old_ipv != nullptr) || (new_ipv != nullptr));

  SchedWeight weight_delta = new_ipv ? new_ipv->total_weight : SchedWeight{0};
  SchedUtilization util_delta = new_ipv ? new_ipv->uncapped_utilization : SchedUtilization{0};
  int32_t critical_delta = new_ipv ? new_ipv->critical_count : 0;
  if (old_ipv != nullptr) {
    weight_delta -= old_ipv->total_weight;
    util_delta -= old_ipv->uncapped_utilization;
    critical_delta -= old_ipv->critical_count;
  }

  SchedulerState& tss = thread.scheduler_state();
  SchedulerState::InheritedProfileValues& thread_ipv = tss.inherited_profile_values_;

  tss.effective_profile_.MarkInheritedProfileChanged();
  thread_ipv.total_weight += weight_delta;
  thread_ipv.uncapped_utilization += util_delta;
  thread_ipv.critical_count += critical_delta;

  DEBUG_ASSERT(thread_ipv.total_weight >= SchedWeight{0});
  DEBUG_ASSERT(thread_ipv.uncapped_utilization >= SchedUtilization{0});
  DEBUG_ASSERT(thread_ipv.critical_count >= 0);

  // If a set of IPVs is going away, and the value which is going away was the
  // minimum, then we need to recompute the new minimum by checking the
  // minimum across all of this thread's owned wait queues.
  //
  // TODO(johngro): Consider keeping the set of owned wait queues as a WAVL
  // tree, indexed by minimum relative deadline, so that this can be
  // maintained in O(1) time instead of O(N).
  //
  // Notes on locking:
  //
  // We need to iterate over our set of owned queues, and find the minimum
  // deadline across all of the queues.  It is not immediately obvious why it is
  // OK to do this.  We are holding our thread's lock, meaning that queues we
  // own cannot leave our collection (mutating the collection requires holding
  // both the thread's lock as well as the lock of the queue which is being
  // added or removed), but we are not explicitly holding the queue's lock.
  //
  // So, why is it OK for us to examine the queue's state?  Specifically, how
  // many threads are blocked in it, and what the minimum deadline across those
  // threads is currently?  It is because, any modification to the upstream
  // queue's state which would mutate the inherited minimum deadline would also
  // require the mutator to hold the entire PI locking chain from (at least) our
  // upstream queue to the eventual target, which (since we currently own this
  // queue) must pass through us as well.
  //
  // So, any previous change made to this value (while we were the queue's
  // owner) had to be done holding this thread's lock as well (since the entire
  // path is locked).  This include any change which adds or removes us as the
  // queue's owner.  Since we are now holding this thread's lock, any of those
  // previous mutations must have happened-before us, and no new mutations can
  // take place until we drop our lock.
  //
  if ((new_ipv != nullptr) && (new_ipv->min_deadline <= thread_ipv.min_deadline)) {
    thread_ipv.min_deadline = ktl::min(thread_ipv.min_deadline, new_ipv->min_deadline);
  } else {
    if ((old_ipv != nullptr) && (old_ipv->min_deadline <= thread_ipv.min_deadline)) {
      SchedDuration new_min_deadline{SchedDuration::Max()};

      for (auto& other_queue : thread.owned_wait_queues_) {
        AssertOwnsWaitQueue(thread, other_queue);
        if (!other_queue.IsEmpty()) {
          DEBUG_ASSERT(other_queue.inherited_scheduler_state_storage_ != nullptr);
          const SchedulerState::InheritedProfileValues& other_ipvs =
              other_queue.inherited_scheduler_state_storage_->ipvs;
          new_min_deadline = ktl::min(new_min_deadline, other_ipvs.min_deadline);
        }
      }

      thread_ipv.min_deadline = new_min_deadline;
    }

    if (new_ipv != nullptr) {
      thread_ipv.min_deadline = ktl::min(thread_ipv.min_deadline, new_ipv->min_deadline);
    }
  }

  thread_ipv.AssertConsistency();
}

void OwnedWaitQueue::ApplyIpvDeltaToOwq(const SchedulerState::InheritedProfileValues* old_ipv,
                                        const SchedulerState::InheritedProfileValues* new_ipv,
                                        OwnedWaitQueue& owq) {
  SchedWeight weight_delta = new_ipv ? new_ipv->total_weight : SchedWeight{0};
  SchedUtilization util_delta = new_ipv ? new_ipv->uncapped_utilization : SchedUtilization{0};
  int32_t critical_delta = new_ipv ? new_ipv->critical_count : 0;

  if (old_ipv != nullptr) {
    weight_delta -= old_ipv->total_weight;
    util_delta -= old_ipv->uncapped_utilization;
    critical_delta -= old_ipv->critical_count;
  }

  DEBUG_ASSERT(!owq.IsEmpty());
  DEBUG_ASSERT(owq.inherited_scheduler_state_storage_ != nullptr);
  SchedulerState::WaitQueueInheritedSchedulerState& iss = *owq.inherited_scheduler_state_storage_;

  iss.ipvs.total_weight += weight_delta;
  iss.ipvs.uncapped_utilization += util_delta;
  iss.ipvs.critical_count += critical_delta;
  iss.ipvs.min_deadline = owq.collection_.MinInheritableRelativeDeadline();

  iss.ipvs.AssertConsistency();
}

template <OwnedWaitQueue::PropagateOp OpType>
void OwnedWaitQueue::BeginPropagate(Thread& upstream_node, OwnedWaitQueue& downstream_node,
                                    PropagateOpTag<OpType> op, ForceInheritance force_inheritance,
                                    const SchedulerState::EffectiveProfile* old_target_ep) {
  // When needed, base profile changes will directly call FinishPropagate.
  static_assert(OpType != PropagateOp::BaseProfileChanged);
  SchedulerState::InheritedProfileValues ipv_snapshot;

  ipv_snapshot = SnapshotThreadIpv(upstream_node, force_inheritance);

  if constexpr (OpType == PropagateOp::RemoveSingleEdge) {
    FinishPropagate(upstream_node, downstream_node, nullptr, &ipv_snapshot, op, force_inheritance,
                    old_target_ep);
  } else if constexpr (OpType == PropagateOp::AddSingleEdge) {
    FinishPropagate(upstream_node, downstream_node, &ipv_snapshot, nullptr, op, force_inheritance,
                    old_target_ep);
  }
}

template <OwnedWaitQueue::PropagateOp OpType>
void OwnedWaitQueue::BeginPropagate(OwnedWaitQueue& upstream_node, Thread& downstream_node,
                                    PropagateOpTag<OpType> op) {
  // When needed, base profile changes will directly call FinishPropagate.
  static_assert(OpType != PropagateOp::BaseProfileChanged);

  if constexpr (OpType == PropagateOp::AddSingleEdge) {
    upstream_node.SetOwner(downstream_node);
  } else if constexpr (OpType == PropagateOp::RemoveSingleEdge) {
    upstream_node.ResetOwner(downstream_node);
  }

  // If the OWQ we are starting from has no active waiters, then there are no
  // IPV deltas to propagate.  After updating the links, we are finished.
  if (upstream_node.IsEmpty()) {
    return;
  }

  DEBUG_ASSERT(upstream_node.inherited_scheduler_state_storage_ != nullptr);
  SchedulerState::InheritedProfileValues& ipvs =
      upstream_node.inherited_scheduler_state_storage_->ipvs;
  ipvs.AssertConsistency();

  if constexpr (OpType == PropagateOp::RemoveSingleEdge) {
    FinishPropagate(upstream_node, downstream_node, nullptr, &ipvs, op);
  } else if constexpr (OpType == PropagateOp::AddSingleEdge) {
    FinishPropagate(upstream_node, downstream_node, &ipvs, nullptr, op);
  }
}

template <OwnedWaitQueue::PropagateOp OpType, typename UpstreamNodeType,
          typename DownstreamNodeType>
void OwnedWaitQueue::FinishPropagate(UpstreamNodeType& upstream_node,
                                     DownstreamNodeType& downstream_node,
                                     const SchedulerState::InheritedProfileValues* added_ipv,
                                     const SchedulerState::InheritedProfileValues* lost_ipv,
                                     PropagateOpTag<OpType> op, ForceInheritance force_inheritance,
                                     const SchedulerState::EffectiveProfile* old_target_ep) {
  // Propagation must start from a(n) (OWQ|Thread) and proceed to a(n) (Thread|OWQ).
  static_assert((ktl::is_same_v<UpstreamNodeType, OwnedWaitQueue> &&
                 ktl::is_same_v<DownstreamNodeType, Thread>) ||
                    (ktl::is_same_v<UpstreamNodeType, Thread> &&
                     ktl::is_same_v<DownstreamNodeType, OwnedWaitQueue>),
                "Bad types for FinishPropagate.  Must be either OWQ -> Thread, or Thread -> OWQ");

  constexpr bool kStartingFromThread = ktl::is_same_v<UpstreamNodeType, Thread>;

  if constexpr (OpType == PropagateOp::AddSingleEdge) {
    DEBUG_ASSERT(added_ipv != nullptr);
    DEBUG_ASSERT(lost_ipv == nullptr);
  } else if constexpr (OpType == PropagateOp::RemoveSingleEdge) {
    DEBUG_ASSERT(added_ipv == nullptr);
    DEBUG_ASSERT(lost_ipv != nullptr);
  } else if constexpr (OpType == PropagateOp::BaseProfileChanged) {
    static_assert(
        kStartingFromThread,
        "Base profile propagation changes may only start from Threads, not OwnedWaitQueues");
    DEBUG_ASSERT(added_ipv != nullptr);
    DEBUG_ASSERT(lost_ipv != nullptr);
  } else {
    // This has to be something template-dependent so it only fails when
    // instantiated and not ignored by the `if constexpr` logic, and nontrivial
    // enough to avoid tautological test sorts of warnings that are generated
    // for e.g. `OpType != OpType`.
    static_assert(ktl::is_void_v<PropagateOpTag<OpType>>, "Unrecognized propagation operation");
  }

  // If neither the IPVs we are adding, nor the IPVs we are removing, are
  // "consequential" (meaning, the have either some fair weight, or some
  // deadline capacity, or both), then we can just get out now.  There are no
  // effective changes to propagate.
  if (!IpvsAreConsequential(added_ipv) && !IpvsAreConsequential(lost_ipv)) {
    return;
  }

  // When we have finally finished updating everything, make sure to update
  // our max traversal statistic.
  ChainLengthTracker len_tracker;

  // Set up the pointers we will use as iterators for traversing the inheritance
  // graph.  Snapshot the starting node's current inherited profile values which
  // we need to propagate.
  OwnedWaitQueue* owq_iter;
  Thread* thread_iter;
  bool skip_phase1 = false;

  if constexpr (kStartingFromThread) {
    thread_iter = &upstream_node;
    owq_iter = &downstream_node;

    // Is this a base profile changed operation?  If so, we should already have
    // a link between our thread and the downstream owned wait queue.  Go ahead
    // and ASSERT this.  We don't need to bother to check the other
    // combinations; those have already been asserted during
    // BeginPropagate.
    if constexpr (OpType == PropagateOp::BaseProfileChanged) {
      thread_iter->get_lock().AssertHeld();
      DEBUG_ASSERT_MSG(thread_iter->wait_queue_state().blocking_wait_queue_ ==
                           static_cast<WaitQueueBase*>(owq_iter),
                       "blocking wait queue %p owq_iter %p",
                       thread_iter->wait_queue_state().blocking_wait_queue_,
                       static_cast<WaitQueueBase*>(owq_iter));
    }

    if constexpr (OpType == PropagateOp::RemoveSingleEdge) {
      // Are we starting from a thread during an edge remove operation?  If so,
      // and if we were the last thread to leave our wait queue, then we don't
      // need to bother to update its IPVs anymore (it cannot have any IPVs if it
      // has no waiters), so we can just skip it an move on to its owner thread.
      //
      // Additionally, we know that it must have an owner thread at this point in
      // time.  If if didn't, BeginPropagate would have already bailed out.
      owq_iter->get_lock().AssertHeld();
      if (owq_iter->IsEmpty()) {
        thread_iter = owq_iter->owner_;
        if (thread_iter == nullptr) {
          return;
        }
        Propagate(upstream_node, *owq_iter, op, force_inheritance, old_target_ep);
        skip_phase1 = true;
      }
    }
  } else {
    // Base profile changes should never start from OWQs.
    static_assert(OpType != PropagateOp::BaseProfileChanged);
    owq_iter = &upstream_node;
    thread_iter = &downstream_node;
    owq_iter->get_lock().AssertHeld();
    DEBUG_ASSERT(!owq_iter->IsEmpty());
    skip_phase1 = true;
  }

  while (true) {
    if (!skip_phase1) {
      // We should not be here if this OWQ has no waiters, or if we have not
      // found a place to store our ISS.  That special case was handled above.
      owq_iter->get_lock().AssertHeld();
      DEBUG_ASSERT(owq_iter->Count() > 0);
      DEBUG_ASSERT(owq_iter->inherited_scheduler_state_storage_ != nullptr);

      // Record what our deadline pressure was before we accumulate the upstream
      // pressure into this node.  We will need it to reason about what to do
      // with our dynamic scheduling parameters after IPV accumulation.
      //
      // OWQs which are receiving deadline pressure have defined dynamic
      // scheduler parameters (start_time, end_time, time_slice_ns) finish
      // times, which they inherited from their upstream deadline threads.  Fair
      // threads do not have things like a defined start time while they are
      // blocked, they will get a new set of dynamic parameters the next time
      // they unblock and are scheduled to run.
      //
      // After we have finished accumulating the IPV deltas, we a few different
      // potential situations:
      //
      // 1) The utilization (deadline pressure) has not changed.  Therefore,
      //    nothing about the dynamic parameters needs to change either.
      // 2) The utilization has changed, and it was 0 before.  This is the first
      //    deadline thread to join the queue, so we can just copy its dynamic
      //    parameters.
      // 3) The utilization has changed, and it is now 0.  The final deadline
      //    thread has left this queue, and our dynamic params are now
      //    undefined.  Strictly speaking, we don't have to do anything, but in
      //    builds with extra checks enabled, we reset the dynamic parameters
      //    so that they have a deterministic value.
      // 4) The utilization has changed, but it was not zero before, and isn't
      //    zero now either.  We call into the scheduler code to compute what
      //    the new dynamic parameters should be.
      //
      SchedulerState::WaitQueueInheritedSchedulerState& owq_iss =
          *owq_iter->inherited_scheduler_state_storage_;
      owq_iss.ipvs.AssertConsistency();
      const SchedUtilization utilization_before = owq_iss.ipvs.uncapped_utilization;
      const SchedulerState::EffectiveProfile old_owq_ep =
          old_target_ep ? *old_target_ep : owq_iter->GetEffectiveProfile();
      ApplyIpvDeltaToOwq(lost_ipv, added_ipv, *owq_iter);
      const SchedUtilization utilization_after = owq_iss.ipvs.uncapped_utilization;

      if (utilization_before != utilization_after) {
        if (utilization_before == SchedUtilization{0}) {
          // First deadline thread just arrived, copy its parameters.
          thread_iter->get_lock().AssertHeld();
          const SchedulerState& ss = thread_iter->scheduler_state();
          owq_iss.start_time = ss.start_time_;
          owq_iss.finish_time = ss.finish_time_;
          owq_iss.time_slice_ns = ss.time_slice_ns_;
          owq_iss.time_slice_used_ns = ss.time_slice_used_ns_;
        } else if (utilization_after == SchedUtilization{0}) {
          // Last deadline thread just left, propagate before resetting.
          Propagate(upstream_node, *owq_iter, op, force_inheritance, &old_owq_ep);
          owq_iss.ResetDynamicParameters();
        } else {
          // The overall utilization has changed, but there was deadline
          // pressure both before and after.  We need to recompute the dynamic
          // scheduler parameters.
          Propagate(upstream_node, *owq_iter, op, force_inheritance, &old_owq_ep);
        }
      }

      // If we no longer have any deadline pressure, our parameters should now
      // be reset to initialization defaults.
      if (utilization_after == SchedUtilization{0}) {
        owq_iss.AssertDynamicParametersAreReset();
      }

      // Advance to the next thread, if any.  If there isn't another thread,
      // then we are finished, simply break out of the propagation loop.
      len_tracker.NodeVisited();
      thread_iter = owq_iter->owner_;
      if (thread_iter == nullptr) {
        break;
      }
    }
    skip_phase1 = false;

    {
      // Propagate from the current owq_iter to the current thread_iter.
      // Apply the change in pressure to the next thread in the chain.
      thread_iter->get_lock().AssertHeld();
      ApplyIpvDeltaToThread(lost_ipv, added_ipv, *thread_iter);
      Propagate(upstream_node, *thread_iter, op, force_inheritance);
      len_tracker.NodeVisited();

      owq_iter = DowncastToOwq(thread_iter->wait_queue_state().blocking_wait_queue_);
      if (owq_iter == nullptr) {
        break;
      }
    }
  }
}

bool OwnedWaitQueue::LockForWakeOperationOrBackoff(uint32_t max_wake, IWakeRequeueHook& wake_hooks,
                                                   Thread::UnblockList& unblock_list_out) {
  // Grab our lock first.
  get_lock().AcquireFirstInChain();

  // Try to lock all of the other things we need to lock.
  ktl::optional<Thread::UnblockList> maybe_unblock_list =
      LockForWakeOperationLocked(max_wake, wake_hooks);
  if (!maybe_unblock_list.has_value()) {
    // Failure; unlock, back off, and try again.
    get_lock().Release();
    return false;
  }

  // Success!  Return the list of locked threads ready to be woken.
  unblock_list_out = ktl::move(maybe_unblock_list).value();
  return true;
}

ktl::optional<Thread::UnblockList> OwnedWaitQueue::LockForWakeOperationLocked(
    uint32_t max_wake, IWakeRequeueHook& wake_hooks) {
  // TODO(https://fxbug.dev/333747818): Find a good way to assert that
  // `max_wake` is greater than zero while still supporting a zero count coming
  // from the futex APIs (zx_futex_wake and zx_futex_requeue).

  // If we have an owner, locking the path starting from it.  We are either going
  // to set the ownership of this queue to (singular) thread that we wake, or
  // we are going to be removing ownership entirely.  Either way, we need to
  // hold the locks along the path of the existing owner (if any).
  const ChainLock::Result lock_res = LockPiChain(*this);
  if (lock_res == ChainLock::Result::Backoff) {
    return ktl::nullopt;
  }
  DEBUG_ASSERT(lock_res == ChainLock::Result::Ok);

  // Lock as many threads as we can, moving them out of our wait collection
  // and onto a temporary unblock list as we go.
  const zx_instant_mono_t now = current_mono_time();
  ktl::optional<Thread::UnblockList> maybe_unblock_list =
      LockAndMakeWaiterListLocked(now, max_wake, wake_hooks);

  // If we didn't get a list back (even an empty one), then we need to back off
  // and try again.  Drop the locks we obtained starting from our owner (if
  // any).
  if (!maybe_unblock_list.has_value()) {
    if (owner_ != nullptr) {
      owner_->get_lock().AssertAcquired();
      UnlockPiChainCommon(*owner_);
    }
    return ktl::nullopt;
  }

  // Success!  We now have all of the locks we need; return the list of
  // threads we have locked and now need to wake.
  return ktl::move(maybe_unblock_list).value();
}

bool OwnedWaitQueue::LockForRequeueOperationOrBackoff(
    OwnedWaitQueue& requeue_target, Thread* new_requeue_target_owner, uint32_t max_wake,
    uint32_t max_requeue, IWakeRequeueHook& wake_hooks, IWakeRequeueHook& requeue_hooks,
    RequeueLockingDetails& requeue_locking_details_out) {
  // Start by locking the this queue and the requeue target.
  if (!AcquireBothOrBackoff(get_lock(), requeue_target.get_lock())) {
    return false;
  }

  // Next, the threads we plan to wake/requeue, placing them on two different lists.
  RequeueLockingDetails res;
  {
    ktl::optional<WakeRequeueThreadDetails> maybe_threads = LockAndMakeWakeRequeueThreadListsLocked(
        current_mono_time(), max_wake, wake_hooks, max_requeue, requeue_hooks);

    if (!maybe_threads.has_value()) {
      get_lock().Release();
      requeue_target.get_lock().Release();
      return false;
    }

    res.threads = ktl::move(maybe_threads).value();
  }

  // Helpers which will release the locks for the threads we just locked,
  // taking them off their unblock lists in the process, if we have to backoff
  // and retry.
  auto UnlockAndClearThreadList = [](Thread::UnblockList list) TA_REQ(chainlock_transaction_token) {
    while (!list.is_empty()) {
      Thread* t = list.pop_front();
      t->get_lock().AssertAcquired();
      t->get_lock().Release();
    }
  };

  auto UnlockLockedThreads = [&]() TA_REQ(chainlock_transaction_token) {
    UnlockAndClearThreadList(ktl::move(res.threads.wake_threads));
    UnlockAndClearThreadList(ktl::move(res.threads.requeue_threads));
  };

  // A helper which we can use to unlock owner chains we may be holding during
  // a backoff situation.
  auto UnlockOwnerChain = [](Thread* owner, const void* stop) TA_REQ(chainlock_transaction_token) {
    if ((owner != nullptr) && (owner != stop)) {
      owner->get_lock().AssertAcquired();
      UnlockPiChainCommon(*owner, stop);
    }
  };

  // Next, lock the proposed new owner of the requeue target (if any),
  // validating the choice of owner in the process.  We need to reject this
  // new owner selection if it is not allowed to own wait queues (in the
  // process of exiting) or if it would form a cycle after this operation.  A
  // cycle can be formed if the new owner chain passes through a thread
  // already in the requeue target (stopping at the requeue target queue
  // itself), or if it passes through any of the threads we intend to requeue.
  if (new_requeue_target_owner) {
    const fit::result result =
        LockPiChainCommon<LockingBehavior::StopAtCycle>(*new_requeue_target_owner);

    // Since we chose the "StopAtCycle" behavior, if we got a ChainLock::Result back,
    // then we either succeeded locking the chain, or we need to back off.
    if (result.is_error()) {
      if (result.error_value() == ChainLock::Result::Backoff) {
        UnlockLockedThreads();
        get_lock().Release();
        requeue_target.get_lock().Release();
        return false;
      }
      res.new_requeue_target_owner = new_requeue_target_owner;
      DEBUG_ASSERT(result.error_value() == ChainLock::Result::Ok);
    } else {
      // if we found a stopping point, and that stopping point is not the
      // wake-queue, or a thread we plan to wake, we need to deny the new
      // owner, dropping the locks we just obtained in the process.
      const void* const stop = result.value();
      const bool accept = [&]() {
        // If we hit nothing, or we hit the wake-queue, we can accept this owner.
        if ((stop == nullptr) || (stop == this)) {
          return true;
        }

        // If we anything in the set of threads to wake, we can accept this
        // owner.
        for (const Thread& t : res.threads.wake_threads) {
          if (stop == &t) {
            return true;
          }
        }

        // We hit a lock we already hold, but it was not the wake-queue or a
        // thread to be woken.  It must have been either a thread being
        // requeued or the requeue target itself, and we cannot accept this
        // new requeue owner.
        return false;
      }();

      // If we can accept this new owner, stash it and its stop point in the
      // result we plan to return.  Otherwise, unlock what we have locked and
      // stash nothing.  Note: We don't want to unlock anything if the stop
      // point is the same as the proposed new owner.  This can happen if the
      // proposed new owner is one of the threads which is being requeued.
      if (accept) {
        res.new_requeue_target_owner = new_requeue_target_owner;
        res.new_requeue_target_owner_stop_point = stop;
      } else {
        UnlockOwnerChain(new_requeue_target_owner, stop);
      }
    }

    // Reject the proposed new owner if it isn't allowed to own wait queues.
    if (res.new_requeue_target_owner) {
      res.new_requeue_target_owner->get_lock().AssertHeld();
      if (res.new_requeue_target_owner->can_own_wait_queues_ == false) {
        UnlockOwnerChain(res.new_requeue_target_owner, res.new_requeue_target_owner_stop_point);
        res.new_requeue_target_owner = nullptr;
        res.new_requeue_target_owner_stop_point = nullptr;
      }
    }
  }

  // Now, if the requeue-target has a current owner, and that owner is
  // changing, lock its chain. It is OK if this chain hits any of the locks we
  // already hold.
  const bool locked_current_requeue_target_owner =
      (requeue_target.owner_ != nullptr) && (requeue_target.owner_ != res.new_requeue_target_owner);
  if (locked_current_requeue_target_owner) {
    const fit::result result =
        LockPiChainCommon<LockingBehavior::StopAtCycle>(*requeue_target.owner_);

    // If we have a result, we either succeeded or need to back off.
    if (result.is_error()) {
      if (result.error_value() == ChainLock::Result::Backoff) {
        UnlockOwnerChain(res.new_requeue_target_owner, res.new_requeue_target_owner_stop_point);
        UnlockLockedThreads();
        get_lock().Release();
        requeue_target.get_lock().Release();
        return false;
      }
      DEBUG_ASSERT(result.error_value() == ChainLock::Result::Ok);
    } else {
      // Record the stopping point.
      res.old_requeue_target_owner_stop_point = result.value();
    }
  }

  // Finally, lock the exiting owner chain for the wake-queue.  It is OK if
  // this chain hits any of the locks we already hold.
  if (this->owner_ != nullptr) {
    const fit::result result = LockPiChainCommon<LockingBehavior::StopAtCycle>(*this->owner_);

    // If we have a result, we either succeeded or need to back off.
    if (result.is_error()) {
      if (result.error_value() == ChainLock::Result::Backoff) {
        if (locked_current_requeue_target_owner) {
          UnlockOwnerChain(requeue_target.owner_, res.old_requeue_target_owner_stop_point);
        }
        UnlockOwnerChain(res.new_requeue_target_owner, res.new_requeue_target_owner_stop_point);
        UnlockLockedThreads();
        get_lock().Release();
        requeue_target.get_lock().Release();
        return false;
      }
      DEBUG_ASSERT(result.error_value() == ChainLock::Result::Ok);
    } else {
      // Record the stopping point.
      res.old_wake_queue_owner_stop_point = result.value();
    }
  }

  // Success!
  requeue_locking_details_out = ktl::move(res);
  return true;
}

bool OwnedWaitQueue::LockForBAAOOperationOrBackoff(Thread* const current_thread, Thread* new_owner,
                                                   BAAOLockingDetails& details_out) {
  DEBUG_ASSERT(current_thread == Thread::Current::Get());
  // Start by locking this queue.
  get_lock().AcquireFirstInChain();

  // Now try to lock the rest of the things we need to lock, adjusting the
  // proposed owner as needed to prevent any cycles which might otherwise form.
  // If we got details back, then we are done.  We should be holding all of the
  // locks we need to hold (including the current thread's lock).
  if (TryLockForBAAOOperationLocked(current_thread, new_owner, details_out)) {
    return true;
  }

  // We encountered some form of backoff error.  Drop the queue lock and try again.
  get_lock().Release();
  return false;
}

bool OwnedWaitQueue::TryLockForBAAOOperationLocked(
    Thread* const current_thread, Thread* new_owner,
    OwnedWaitQueue::BAAOLockingDetails& details_out) {
  DEBUG_ASSERT(current_thread == Thread::Current::Get());

  // Start by trying to obtain the current thread's lock.
  if (!current_thread->get_lock().AcquireOrBackoff()) {
    return false;
  }

  // Now attempt to lock the rest of the old owner/new owner chain, backing
  // off if we have to.  This common code path with automatically deal with
  // nack'ing the proposed new owner when appropriate.
  const fit::result result = LockForOwnerReplacement(new_owner, current_thread);
  if (result.is_error()) {
    // The only valid lock result we can get back from this operation is
    // Backoff.  LockForOwnerReplacement should handle any cycles, and if the
    // locking operation succeeds, it is going to return locking details
    // instead of an "OK" ChainLock::Result.
    DEBUG_ASSERT(result.error_value() == ChainLock::Result::Backoff);
    current_thread->get_lock().Release();
    return false;
  }

  // Success, return the locking details.
  details_out = result.value();
  return true;
}

fit::result<ChainLock::Result, OwnedWaitQueue::ReplaceOwnerLockingDetails>
OwnedWaitQueue::LockForOwnerReplacement(Thread* _new_owner, const Thread* blocking_thread,
                                        bool propagate_new_owner_cycle_error,
                                        bool new_owner_is_locked) {
  // We are holding the queue lock, and we have a proposed new owner.  We need
  // to hold all of appropriate locks, and potentially nak the new owner
  // proposal.
  //
  // Start with the most trivial reason we might disallow a chosen new owner.
  // The new owner of a wait queue cannot be the same as the thread who is
  // blocking as this would obviously generate an immediate cycle.  We can
  // trivially check for that and rule it out first thing.
  //
  // After that, we need to consider a few different special cases.
  //
  // In each of these cases, it is possible for us to detect a cycle which would
  // be created if the operation were allowed to proceed as requested.
  // Currently, we do not allow these operations to fail (users must always be
  // able to block their threads), and we do not have a defined job policy which
  // would allow us to terminate a process which attempts to create a PI cycle.
  // So, for now, instead we simply change the new owner to become "nothing",
  // preventing the cycle from forming in the first place.
  //
  // Case 1 : new owner == old owner
  //
  // If the old owner and the new owner are the same, we simply attempt to
  // lock the path starting from the current owner.  If this detects a cycle,
  // we will start again, but this time with new owner == nullptr != old
  // owner.  This can happen if there is a thread blocking which the new owner
  // is currently blocked upstream from.  For example, in the diagram below, if
  // T3 attempts to block in Q1, declaring T1 to be the queue owner.
  //
  // +----+     +----+     +----+     +----+     +----+     +----+
  // | Q1 | --> | T1 | --> | Q2 | --> | T2 | --> | Q3 | --> | T3 |
  // +----+     +----+     +----+     +----+     +----+     +----+
  //
  // Case 2 : new owner != old owner && new owner != nullptr
  //
  // We are either adding a new owner, or replacing an exiting owner with a
  // new one.  Start by locking the new owner path.  If this detects a cycle,
  // we set the newly proposed owner to nullptr and start again.  Otherwise,
  // once the new owner path is locked, we proceed as in case #2 but with a
  // small modification.  If following the old owner path leads to a detected
  // cycle, it could be for one of two reasons.  If this is a BAAO operation, it
  // may be that the addition of the blocking thread to the target wait queue
  // would introduce a cycle (as in case #2), or it could be that the old owner
  // path converges with the new owner path.  Neither one of these cases is
  // illegal; we just need to stop locking the old path when we hit this
  // intersection point, and remember that when it comes time to unlock the old
  // path after removing the old owner, we need to stop when we encounter this
  // intersection point.
  //
  // Case 3 : new owner != old owner && new owner == nullptr
  //
  // If we are removing an existing owner, then we can simply lock the old owner
  // path, tolerating any cycles we encounter (but marking where they are) in
  // the process.  If we end up detecting a cycle, it can only be because we are
  // involved in a BAAO operation, and the old owner is was blocked behind the
  // thread who is currently blocking.  All we need to do is assert this, and
  // make sure that when we are unlocking the old owner path that we stop at the
  // blocking thread and do not go any further.
  //
  // If we detect a cycle for any other reason, there must be something wrong.
  // The old owner cannot be involved in a path which does not pass through a
  // thread which is blocking.  If it were, it would imply that there was
  // already a cycle in the graph before the proposed BAAO operation (which
  // would violate the PI graph invariants).

  ReplaceOwnerLockingDetails details{_new_owner};
  Thread*& new_owner = details.new_owner;
  Thread* const old_owner = owner_;
  const bool old_owner_is_blocking = (blocking_thread == old_owner);

  // Disallow the trivial case of "the new owner cannot also be the blocking
  // thread" before getting in handling the more complicated cases.
  if (new_owner == blocking_thread) {
    new_owner = nullptr;
  }

  // Case 1, try to validate our existing owner (if any)
  if (new_owner == old_owner) {
    // If we have no owner, and we are not assigning a new owner, then we are
    // done.
    if (old_owner == nullptr) {
      return fit::ok(details);
    }
    // If the new owner is locked, then we are done.
    if (new_owner_is_locked) {
      new_owner->get_lock().AssertHeld();
      return fit::ok(details);
    }

    // Lock the chain starting from the current owner.  This is case 1 where the old
    // owner and proposed new owner are the same.  Since the new owner cannot be
    // the blocking thread, this means that we can be sure that the old owner is
    // also not the blocking thread.
    DEBUG_ASSERT(old_owner_is_blocking == false);
    const fit::result result = LockPiChainCommon<LockingBehavior::StopAtCycle>(*new_owner);
    const void* stop_point = nullptr;

    // If we didn't detect a cycle, and the existing owner is allowed to own
    // wait queues (not in the process of exiting) we are finished.  We either
    // succeeded and can simply get out holding the locks we have, or we failed
    // an need to back-off (which we can do immediately since we didn't obtain
    // any new locks).
    if (result.is_error()) {
      ChainLock::Result lock_res = result.error_value();
      if (lock_res != ChainLock::Result::Ok) {
        DEBUG_ASSERT(lock_res == ChainLock::Result::Backoff);
        return fit::error(lock_res);
      }

      new_owner->get_lock().AssertHeld();
      if (new_owner->can_own_wait_queues_ == true) {
        return fit::ok(details);
      }
    } else {
      // We found a cycle which would have been formed by attempting to block a
      // thread who is currently downstream of this queue's owner.  Assert this,
      // then drop the locks we acquired and force the new owner to be nullptr in
      // order to break the cycle.  Then proceed to the new_owner != old_owner
      // logic (since, they no longer match).
      //
      // Note: we may not have locked anything at all.  It is possible that
      // someone attempted to block this queue's current owner in the queue
      // itself.  We need to make sure to check for this special case before
      // calling UnlockPiChainCommon.
      stop_point = result.value();
      DEBUG_ASSERT_MSG(
          stop_point == static_cast<const void*>(blocking_thread),
          "Detected cycle point is not the blocking thread.  (BT %p this %p stop_point %p)",
          blocking_thread, this, stop_point);
    }

    // If we made it to this point, we either detected an unacceptable cycle, or
    // our new owner was rejected because it is exiting and is not allowed to
    // take ownership of the queue.  Unlock any locks that we held, reject the
    // new owner proposal, and fall into case 3.
    if (new_owner->get_lock().MarkNeedsReleaseIfHeld()) {
      UnlockPiChainCommon(*new_owner, stop_point);
    }
    new_owner = nullptr;
  }

  // If we made it this far, we must be in case 2 or 3 (the owner is changing)
  DEBUG_ASSERT(new_owner != old_owner);

  // Case 2, we are replacing the old owner (if any) with a new one;
  if (new_owner != nullptr) {
    // Start by attempting to lock the new owner path.
    const ChainLock::Result new_owner_lock_res =
        !new_owner_is_locked
            ? LockPiChainCommon<LockingBehavior::RefuseCycle>(*new_owner).error_value()
            : ChainLock::Result::Ok;

    if (new_owner_lock_res == ChainLock::Result::Ok) {
      // Things went well and we got the locks.  Check to make sure that our new
      // owner is actually allowed to own wait queues before proceeding.
      const bool can_own_wait_queues = [new_owner]() TA_REQ(chainlock_transaction_token) {
        new_owner->get_lock().AssertHeld();
        return new_owner->can_own_wait_queues_;
      }();
      if (can_own_wait_queues == false) {
        // Our new owner is not allowed to own queues.  Unlock it, and reject the
        // proposal.
        if (!new_owner_is_locked) {
          new_owner->get_lock().MarkNeedsRelease();
          UnlockPiChainCommon(*new_owner);
        }
        new_owner = nullptr;
        if (old_owner == nullptr) {
          return fit::ok(details);
        }
      }

      // We now either hold the new owner chain, or have rejected the new owned
      // because it is not allowed to own queues.  If there is no current owner
      // we are finished.
      if (old_owner == nullptr) {
        return fit::ok(details);
      }

      // If we didn't reject the new owner, we now need to lock the old owner
      // chain.
      if (new_owner != nullptr) {
        // Lock the old owner path, but tolerate a detected cycle.  There are
        // two special edge cases to consider here:
        //
        // First, the old owner may be the thread who is blocking (and is
        // declaring a different owner in the process).  The blocking thread is
        // currently running, and therefore cannot itself be blocked (there is
        // nothing downstream of it), and we already hold its lock.  So, if the
        // blocking thread is the same as the old owner, there are no new locks
        // to obtain. We just need to specify the blocking thread as the unlock
        // stopping point for the operation.
        //
        // Second, while locking the old_owner path (old owner != blocking
        // thread), we may run into the blocking thread (as in case 2), or we
        // may run into a node which is also on the new owner path.  Either is
        // OK since we are replacing the old owner with a (now validated) new
        // one, we just need to remember where to stop unlocking after we have
        // replaced the old owner with a new one.
        const fit::result result = !old_owner_is_blocking
                                       ? LockPiChainCommon<LockingBehavior::StopAtCycle>(*old_owner)
                                       : fit::ok(static_cast<const void*>(blocking_thread));
        if (result.is_error()) {
          const ChainLock::Result old_owner_lock_res = result.error_value();

          // Since we chose StopAtCycle behavior for locking the old_owner path,
          // we know the result cannot be CycleDetected.  It is either OK (and
          // we are done) or Backoff (and we need to try again).
          DEBUG_ASSERT(old_owner_lock_res != ChainLock::Result::Cycle);
          if (old_owner_lock_res == ChainLock::Result::Ok) {
            // Things went well, we are done.
            return fit::ok(details);
          }

          // Need to back off and try again.  Drop the locks we were holding
          // before restarting.
          DEBUG_ASSERT(old_owner_lock_res == ChainLock::Result::Backoff);
          if (!new_owner_is_locked) {
            new_owner->get_lock().AssertAcquired();
            UnlockPiChainCommon(*new_owner);
          }
          return fit::error(old_owner_lock_res);
        }

        // Success (although our paths intersect in some way).  Record our
        // stopping point and get out.
        DEBUG_ASSERT(result.is_ok());
        details.stop_point = result.value();
        DEBUG_ASSERT(details.stop_point != static_cast<const void*>(this));
        return fit::ok(details);
      }
    } else if (new_owner_lock_res == ChainLock::Result::Backoff) {
      // Need to back off and try again.  We should not be holding any new
      // locks, so we can just propagate the error up and out.
      return fit::error(new_owner_lock_res);
    } else {
      // We found a cycle, so we have to nack this new owner.  If the caller asked us to propagate
      // the error up the stack, we can just do so now.  We are not holding any extra locks (yet).
      // Otherwise, reject the new owner proposal (storing nullptr in the details structure we are
      // going to return) and proceed as if the caller is going remove the current owner entirely,
      // instead of replacing it. If we don't have a current owner, then we are finished.  If we do,
      // we can simply drop into the Case 2 handler (below).
      DEBUG_ASSERT(new_owner_lock_res == ChainLock::Result::Cycle);
      if (propagate_new_owner_cycle_error) {
        return fit::error(ChainLock::Result::Cycle);
      }

      new_owner = nullptr;
      if (old_owner == nullptr) {
        return fit::ok(details);
      }
    }
  }

  // The only option left is that we are now in case 3; we are removing the
  // existing owner. We either started in that situation, or managed to get
  // there via one of our cycle detection mitigations.  Check the old owner path
  // to see if there is a temporary cycle that we need to deal with.
  DEBUG_ASSERT((new_owner == nullptr) && (old_owner != nullptr));

  // See case 2 (above).  If the old owner is the blocking thread, there is
  // nothing more to lock, and we need to remember to stop unlocking at the old
  // owner during owner reassignment.
  const fit::result result = !old_owner_is_blocking
                                 ? LockPiChainCommon<LockingBehavior::StopAtCycle>(*old_owner)
                                 : fit::ok(static_cast<const void*>(blocking_thread));
  if (result.is_error()) {
    const ChainLock::Result lock_res = result.error_value();

    // Since we chose StopAtCycle behavior for locking the old_owner path, we
    // know the result cannot be CycleDetected.  It is either OK (and we are
    // done) or Backoff (and we need to try again).
    DEBUG_ASSERT(lock_res != ChainLock::Result::Cycle);
    if (lock_res == ChainLock::Result::Ok) {
      // Things went well, we are done.
      return fit::ok(details);
    }

    // Need to back off and try again.  We are not holding any new locks, so we
    // can just propagate the error.
    DEBUG_ASSERT(lock_res == ChainLock::Result::Backoff);
    return fit::error(lock_res);
  }

  // We must have found a cycle.  If we did, we successfully locked up until
  // the point in the graph where that cycle was detected, and marked that as
  // our stop point.  Since we are removing the old owner, we know that there
  // must be a blocking thread, and the cycle we detected must have been found
  // at that point.  ASSERT this, then Record our stop point in the results
  // and we are done.
  DEBUG_ASSERT(result.is_ok());
  details.stop_point = result.value();
  DEBUG_ASSERT_MSG(
      details.stop_point == static_cast<const void*>(blocking_thread),
      "Detected cycle point is not the blocking thread.  (BT %p this %p stop_point %p)",
      blocking_thread, this, details.stop_point);
  return fit::ok(details);
}

ktl::optional<Thread::UnblockList> OwnedWaitQueue::LockAndMakeWaiterListLocked(
    zx_instant_mono_t now, uint32_t max_count, IWakeRequeueHook& hooks) {
  // Try to lock a set of threads to wake.  If we succeeded, make sure we place
  // them back into the collection before returning the list.
  ktl::optional<Thread::UnblockList> res = TryLockAndMakeWaiterListLocked(now, max_count, hooks);
  if (res.has_value()) {
    for (Thread& t : res.value()) {
      t.get_lock().AssertHeld();
      collection_.Insert(&t);
    }
  }

  return res;
}

ktl::optional<OwnedWaitQueue::WakeRequeueThreadDetails>
OwnedWaitQueue::LockAndMakeWakeRequeueThreadListsLocked(zx_instant_mono_t now,
                                                        uint32_t max_wake_count,
                                                        IWakeRequeueHook& wake_hooks,
                                                        uint32_t max_requeue_count,
                                                        IWakeRequeueHook& requeue_hooks) {
  // Start by trying to lock the set of threads to wake.
  ktl::optional<Thread::UnblockList> wake_res =
      TryLockAndMakeWaiterListLocked(now, max_wake_count, wake_hooks);
  if (!wake_res.has_value()) {
    return ktl::nullopt;
  }

  // Success!  The threads we have selected for waking have been temporarily
  // removed from the wake-queue, allowing us to now select the set of threads
  // to requeue.  If we fail, make sure we unlock the threads we selected for
  // wake and return them to the collection.
  ktl::optional<Thread::UnblockList> requeue_res =
      TryLockAndMakeWaiterListLocked(now, max_requeue_count, requeue_hooks);
  if (!requeue_res.has_value()) {
    UnlockAndClearWaiterListLocked(ktl::move(wake_res).value());
    return ktl::nullopt;
  }

  // Success, we have managed to lock both sets of threads.  Add them back to
  // the collection before proceeding.
  WakeRequeueThreadDetails ret{
      .wake_threads = ktl::move(wake_res).value(),
      .requeue_threads = ktl::move(requeue_res).value(),
  };

  for (Thread::UnblockList* list : ktl::array{&ret.wake_threads, &ret.requeue_threads}) {
    for (Thread& t : *list) {
      t.get_lock().AssertHeld();
      collection_.Insert(&t);
    }
  }

  return ret;
}

ktl::optional<Thread::UnblockList> OwnedWaitQueue::TryLockAndMakeWaiterListLocked(
    zx_instant_mono_t now, uint32_t max_count, IWakeRequeueHook& hooks) {
  // Lock as many threads as we can, placing them on a temporary unblock list as
  // we go.  We remove the threads from the collection as we go, but we do not
  // remove the bookkeeping (see the note on optimization below).
  Thread::UnblockList list;

  // TODO(johngro): Figure out a way to optimize this.
  //
  // We need to go over the wait queue and select the "next N best threads to
  // wake up".  The total order of the wait queue, however, is complicated.
  // Which thread is the "best" thread depends on what time it is, if there are
  // deadline threads in the queue, and whether or not that thread's deadline is
  // in the future or the past.
  //
  // The queue is arranged to make it easy to find the "best thread to wake"
  // quickly, but is not arranged to easily iterate in the proper order (best to
  // worst to wake) based on a given time.
  //
  // One way around this is to simply remove the threads from the collection as
  // we lock then, and then peek again using the same `now` time as before.
  // There are two annoying issues with this, however.  If we fail to grab the
  // locks, we now have to drop all of the locks and put the threads back into
  // the collection as we unwind.  This is a lot of pointless re-balancing of
  // the tree structure for no good reason.
  //
  // The second issue is currently that OWQ code involved in waking and
  // requeueing threads expects those threads to be a proper member of the
  // collection, right up until the point that the threads are actually woken or
  // transferred to another queue.  This allows for some optimizations ("I don't
  // need to propagate X because there are now no more waiters") as well as some
  // heavy-duty debug checks (I can add up the IPVs of all the threads in the
  // queue, and it should equal my current IPV totals) which become confused if
  // we have speculatively removed threads from the collection before actually
  // removing them and their bookkeeping (something which requires all of the
  // locks to be held).
  //
  // So, for now, we have a solution which is basically the worst-of-all-worlds.
  // During the locking, we _do_ actually remove the threads from the wait queue
  // collection as we add them into our temporary unblock list.  If we fail and
  // need to unwind, we need to unlock and put the threads back into the
  // collection. But, _additionally_, if we succeed, we need to go over the list
  // again and put the threads back into the collection, only so the rest of the
  // OWQ can take them back out again at the proper point in time.
  //
  // Ideally, we would find an efficient wait to optimize the iteration of the
  // queue given a specific time so that we didn't have to fall back on such
  // heavy handed approaches.  Failing that, it might be best to special case
  // various scenarios.  The most common scenarios are ones where we wake either
  // a single thread (in which case the search works fine), or all of the
  // threads, in which case the enumeration order is less important, although
  // could cause some scheduler thrash if done in the wrong order.
  // Additionally, the total order is just the enumeration order of the tree if
  // all of the deadline threads have absolute deadlines in the future (this
  // includes where there are no deadline threads in the queue), so that is a
  // simple case as well.  The only tricky case is when there exist deadline
  // threads in the queue whose deadline is in the past, and we want to wake
  // more than one thread.
  //
  for (uint32_t count = 0; count < max_count; ++count) {
    Thread* t = PeekFront();
    if (t == nullptr) {
      break;
    }

    // Does our caller approve of this thread?  If not, stop here.
    const bool stop = [&]() TA_REQ(chainlock_transaction_token, this->get_lock()) {
      AssertInWaitQueue(*t, *this);
      // This lock is not actually held, but hooks.Allow requires the lock to be
      // held for shared (i.e. read only) access. There is a special rule that
      // allows read only access without holding the lock, provided another lock
      // (i.e. the owned wait queue lock that IS held) prevents mutations and
      // provides memory synchronization. There is an improved annotation
      // strategy under development that will replace this kludge.
      t->get_lock().MarkHeld();
      return !hooks.Allow(*t);
    }();

    if (stop) {
      break;
    }

    // We need to back off.  Drop any locks we have acquired so far.
    if (!t->get_lock().AcquireOrBackoff()) {
      if (!list.is_empty()) {
        UnlockAndClearWaiterListLocked(ktl::move(list));
      }
      return ktl::nullopt;
    }

    // Cycles should be impossible at this point, so assert that we are holding
    // the lock, then add the thread to the list of threads we have locked.
    // Then, move on to locking more threads if we need to.
    collection_.Remove(t);
    list.push_back(t);
    t->get_lock().MarkReleased();
  }

  // Success!  Do not attempt to put our threads back just yet.  If we are
  // making two lists for a WakeAndRequeue operation, we need to keep the
  // threads we have selected for wake out of the collection while we select the
  // threads for requeue.
  return list;
}

void OwnedWaitQueue::UnlockAndClearWaiterListLocked(Thread::UnblockList list) {
  // If we encountered a back-off error, we need to unlock each thread we had on
  // our list, placing threads back into the collection and clearing the list as
  // we go.
  while (!list.is_empty()) {
    Thread* t = list.pop_front();
    t->get_lock().AssertAcquired();
    collection_.Insert(t);
    t->get_lock().Release();
  }
}

template <OwnedWaitQueue::LockingBehavior Behavior, typename StartNodeType>
fit::result<ChainLock::Result, const void*> OwnedWaitQueue::LockPiChainCommon(
    StartNodeType& start) {
  Thread* next_thread{nullptr};
  WaitQueueBase* next_wq{nullptr};
  bool skip_thread_lock = false;

  if constexpr (ktl::is_same_v<StartNodeType, Thread>) {
    next_thread = &start;
  } else {
    static_assert(ktl::is_base_of_v<WaitQueueBase, StartNodeType>);
    next_wq = &start;
    skip_thread_lock = true;
  }

  while (true) {
    if (!skip_thread_lock) {
      // If we hit the end of the chain, we are done.
      if (next_thread == nullptr) {
        return fit::error(ChainLock::Result::Ok);
      }

      // Try to lock the next thread; if we fail, implement the specified locking
      // behavior.  We always propagate Backoff error codes.  In the case of a
      // detected cycle, we either propagate the error, or we leave the current
      // path locked and report the location of the cycle in our return code.
      if (ChainLock::Result result; !next_thread->get_lock().AcquireOrResult(result)) {
        if constexpr (Behavior == LockingBehavior::StopAtCycle) {
          if (result == ChainLock::Result::Cycle) {
            return fit::ok(static_cast<const void*>(next_thread));
          }
        }

        if (start.get_lock().MarkNeedsReleaseIfHeld()) {
          UnlockPiChainCommon(start, next_thread);
        }

        return fit::error(result);
      }
      // We just checked for success, skip the assert and just mark this lock as
      // held for the static analyzer's sake.
      next_wq = next_thread->wait_queue_state().blocking_wait_queue_;
      next_thread->get_lock().MarkReleased();
      next_thread = nullptr;
    }
    skip_thread_lock = false;

    // If we hit the end of the chain, we are done.
    if (next_wq == nullptr) {
      return fit::error(ChainLock::Result::Ok);
    }

    if (ChainLock::Result result; !next_wq->get_lock().AcquireOrResult(result)) {
      if constexpr (Behavior == LockingBehavior::StopAtCycle) {
        if (result == ChainLock::Result::Cycle) {
          return fit::ok(static_cast<const void*>(next_wq));
        }
      }

      if (start.get_lock().MarkNeedsReleaseIfHeld()) {
        UnlockPiChainCommon(start, next_wq);
      }

      return fit::error(result);
    }

    next_thread = GetQueueOwner(next_wq);
    next_wq->get_lock().MarkReleased();
    next_wq = nullptr;
  }
}

template <typename StartNodeType>
void OwnedWaitQueue::UnlockPiChainCommon(StartNodeType& start, const void* stop_point) {
  Thread* next_thread{nullptr};
  WaitQueueBase* next_wq{nullptr};

  auto ShouldStopThread = [stop_point](const Thread* t) TA_REQ(chainlock_transaction_token) {
    if (stop_point != nullptr) {
      const bool stop_point_match = (stop_point == t);
      DEBUG_ASSERT(t != nullptr);
      DEBUG_ASSERT(stop_point_match || t->get_lock().is_held());
      return stop_point_match;
    } else {
      return (t == nullptr) || (t->get_lock().is_held() == false);
    }
  };

  auto ShouldStopWaitQueue = [stop_point](const WaitQueueBase* wq)
                                 TA_REQ(chainlock_transaction_token) {
                                   if (stop_point != nullptr) {
                                     const bool stop_point_match = (stop_point == wq);
                                     DEBUG_ASSERT(wq != nullptr);
                                     DEBUG_ASSERT(stop_point_match || wq->get_lock().is_held());
                                     return stop_point_match;
                                   } else {
                                     return (wq == nullptr) || (wq->get_lock().is_held() == false);
                                   }
                                 };

  // We must currently be holding the starting node's lock.  If we found our
  // stopping point, have no next node, or we are not currently holding the next
  // node's lock, we are done.
  DEBUG_ASSERT(&start != stop_point);
  if constexpr (ktl::is_same_v<StartNodeType, Thread>) {
    next_wq = start.wait_queue_state().blocking_wait_queue_;

    // Make sure that we check our stop condition before drop our current node
    // lock (here and below).  Otherwise, it is possible that the next node is
    // not locked, and can go away as soon as we drop the lock on our upstream
    // node (here and below).
    const bool stop = ShouldStopWaitQueue(next_wq);
    start.get_lock().Release();
    if (stop) {
      return;
    }
  } else {
    static_assert(ktl::is_base_of_v<WaitQueueBase, StartNodeType>);
    next_thread = GetQueueOwner(&start);
    const bool stop = ShouldStopThread(next_thread);
    start.get_lock().Release();
    if (stop) {
      return;
    }
  }

  while (true) {
    // The first time entering the loop, the next thread may be nullptr if the
    // initial node was a wait queue. After that, there should always be a next
    // thread, and it should be locked (as we always check `is_held` before
    // releasing the previous node's lock).
    if (next_thread != nullptr) {
      next_thread->get_lock().MarkNeedsRelease();

      // If we do not have a next node, or that next node is not locked, we can
      // drop the thread's lock and get out.  Note: it is important to make the
      // check to see if the next node is locked before dropping the thread's
      // lock.  Otherwise, it would be possible for us to realize that we have a
      // next node, drop the thread's lock, then have that next node destroyed
      // before we are able to check to see if we have it locked.
      next_wq = next_thread->wait_queue_state().blocking_wait_queue_;
      {
        const bool stop = ShouldStopWaitQueue(next_wq);
        next_thread->get_lock().Release();
        next_thread = nullptr;
        if (stop) {
          return;
        }
      }
    }

    // At this point, there should always be a next wait queue, and it should
    // always be locked (as we always check `is_held` before releasing the
    // previous node's lock).
    next_wq->get_lock().MarkNeedsRelease();

    // See the note above, we need to check to see if there is a next node, and
    // that we have it locked _before_ dropping the locked WQ's lock.
    next_thread = GetQueueOwner(next_wq);
    {
      const bool stop = ShouldStopThread(next_thread);
      next_wq->get_lock().Release();
      next_wq = nullptr;
      if (stop) {
        return;
      }
    }
  }
}

void OwnedWaitQueue::SetThreadBaseProfileAndPropagate(Thread& thread,
                                                      const SchedulerState::BaseProfile& profile) {
  // To change this thread's base profile, we need to start by locking the
  // entire PI chain starting from this thread.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    thread.get_lock().AcquireFirstInChain();
    WaitQueueBase* wq = thread.wait_queue_state().blocking_wait_queue_;
    OwnedWaitQueue* owq = DowncastToOwq(wq);

    // Lock the rest of the PI chain, restarting if we need to.
    if (LockPiChain(thread) == ChainLock::Result::Backoff) {
      thread.get_lock().Release();
      return ChainLockTransaction::Action::Backoff;
    }

    // Finished obtaining locks.  We can now propagate.
    ChainLockTransaction::Finalize();

    // If our thread is blocked in an owned wait queue, we need observe the
    // thread's transmitted IPVs before and after the base profile change in order
    // to properly handle propagation.
    SchedulerState::InheritedProfileValues old_ipvs;
    if (owq != nullptr) {
      old_ipvs = SnapshotThreadIpv(thread);
    }

    // Regardless of the state of the thread whose base profile has changed, we
    // need to update the base profile and let the scheduler know.  The scheduler
    // code will handle:
    // 1) Updating our effective profile
    // 2) Repositioning us in our wait queue (if we are blocked)
    // 3) Updating our dynamic scheduling parameters (if we are either runnable or
    //    a blocked deadline thread)
    // 4) Updating the scheduler's state (if we happen to be a runnable thread).
    SchedulerState& state = thread.scheduler_state();
    state.base_profile_ = profile;
    state.effective_profile_.MarkBaseProfileChanged();
    Scheduler::ThreadBaseProfileChanged(thread);

    // Now, if we are blocked in an owned wait queue, propagate the consequences
    // of the base profile change downstream.
    if (owq != nullptr) {
      owq->get_lock().AssertHeld();
      SchedulerState::InheritedProfileValues new_ipvs = SnapshotThreadIpv(thread);
      FinishPropagate(thread, *owq, &new_ipvs, &old_ipvs, BaseProfileChangedOp);
    }

    // Finished, drop the locks we are holding and make sure we break out of the
    // retry loop.
    UnlockPiChainCommon(thread);
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                  CLT_TAG("OwnedWaitQueue::SetThreadBaseProfileAndPropagate"),
                                  do_transaction);
}

ChainLockTransaction::Result<zx_status_t> OwnedWaitQueue::AssignOwnerCommon(
    Thread* new_owner, bool new_owner_is_locked) {
  // If the owner is not changing, we are already done.
  if (owner_ == new_owner) {
    return ZX_OK;
  }

  // Obtain the locks needed to change owner.  If, while obtaining locks, we
  // detect that the new owner proposal would generate a cycle, we nak the
  // entire operation with a BAD_STATE error and change nothing.
  const fit::result<ChainLock::Result, ReplaceOwnerLockingDetails> result =
      LockForOwnerReplacement(new_owner, nullptr, true, new_owner_is_locked);
  if (result.is_error()) {
    const ChainLock::Result lock_res = result.error_value();
    if (lock_res == ChainLock::Result::Cycle) {
      return ZX_ERR_BAD_STATE;
    }

    // The only other option here is that we are supposed to back off and try
    // again.  If we had succeeded, we would have gotten back locking details
    // instead.
    DEBUG_ASSERT(lock_res == ChainLock::Result::Backoff);
    return ChainLockTransaction::Action::Backoff;
  }

  // Go ahead and perform the assignment.  Start by removing any current
  // owner we have, dropping the locks along that path immediately after we
  // are done.
  if (Thread* const old_owner = owner_; old_owner != nullptr) {
    old_owner->get_lock().AssertHeld();
    BeginPropagate(*this, *old_owner, RemoveSingleEdgeOp);
    UnlockPiChainCommon(*old_owner, result->stop_point);
  }

  // Now, if we have a new owner to assign, perform the assignment, then
  // drop the locks along the new owner path.
  DEBUG_ASSERT(result->new_owner == new_owner);
  if (new_owner != nullptr) {
    new_owner->get_lock().AssertHeld();
    BeginPropagate(*this, *new_owner, AddSingleEdgeOp);
    if (!new_owner_is_locked) {
      UnlockPiChainCommon(*new_owner);
    }
  }

  // We are finished.  We will drop our queue lock and re-enable preemption as
  // we unwind.
  return ZX_OK;
}

zx_status_t OwnedWaitQueue::AssignOwner(Thread* new_owner) {
  DEBUG_ASSERT(magic() == kOwnedMagic);
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token,
                   preempt_disabled_token) -> ChainLockTransaction::Result<zx_status_t> {
    ChainLockGuard guard{get_lock()};
    return AssignOwnerCommon(new_owner);
  };
  return ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                         CLT_TAG("OwnedWaitQueue::AssignOwner"), do_transaction);
}

zx_status_t OwnedWaitQueue::AssignOwnerLocked(Thread* new_owner) {
  ChainLockGuard guard(get_lock(), ChainLockGuard::Defer);
  if (!guard.AcquireOrBackoff()) {
    return ZX_ERR_SHOULD_WAIT;
  }
  auto result = AssignOwnerCommon(new_owner, /*new_owner_is_locked=*/true);
  if (result.is_action()) {
    // If there is an action to perform, we expect to be Backoff.
    DEBUG_ASSERT(result.action() == ChainLockTransaction::Action::Backoff);
    return ZX_ERR_SHOULD_WAIT;
  }
  // If there is a value, we expect ZX_OK. Anything else suggests a cycle was detected.
  DEBUG_ASSERT(result.take_value() == ZX_OK);
  return result.take_value();
}

void OwnedWaitQueue::ResetOwnerIfNoWaiters() {
  DEBUG_ASSERT(magic() == kOwnedMagic);
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    ChainLockGuard guard{get_lock()};

    // If we have no owner, or we have waiters, we are finished.
    if (!IsEmpty() || (owner_ == nullptr)) {
      return ChainLockTransaction::Done;
    }

    // We have an owner, but no waiters behind us.  We need to lock our owner in
    // order to clear out its bookkeeping, but there is nothing to propagate
    // down the PI chain since we are not inheriting anything.
    Thread* owner = owner_;
    if (!owner->get_lock().AcquireOrBackoff()) {
      return ChainLockTransaction::Action::Backoff;
    }

    ChainLockTransaction::Finalize();

    ResetOwner(*owner);
    owner->get_lock().Release();

    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                  CLT_TAG("OwnedWaitQueue::ResetOwnerIfNoWaiters"), do_transaction);
}

void OwnedWaitQueue::AssignOwnerInternal(Thread* new_owner) {
  // If there is no change, then we are done already.
  if (owner_ == new_owner) {
    return;
  }

  // Start by releasing the old owner (if any) and propagating the PI effects.
  if (owner_ != nullptr) {
    owner_->get_lock().AssertHeld();
    BeginPropagate(*this, *owner_, RemoveSingleEdgeOp);
  }

  // If there is a new owner to assign, do so now and propagate the PI effects.
  if (new_owner != nullptr) {
    new_owner->get_lock().AssertHeld();
    DEBUG_ASSERT(new_owner->can_own_wait_queues_);
    BeginPropagate(*this, *new_owner, AddSingleEdgeOp);
  }

  ValidateSchedStateStorage();
}

zx_status_t OwnedWaitQueue::BlockAndAssignOwner(const Deadline& deadline, Thread* new_owner,
                                                ResourceOwnership resource_ownership,
                                                Interruptible interruptible) {
  Thread* const current_thread = Thread::Current::Get();
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token) -> ChainLockTransaction::Result<zx_status_t> {
    preempt_disabled_token.AssertHeld();
    if (BAAOLockingDetails details;
        LockForBAAOOperationOrBackoff(current_thread, new_owner, details)) {
      ChainLockTransaction::Finalize();
      const zx_status_t result = BlockAndAssignOwnerLocked(current_thread, deadline, details,
                                                           resource_ownership, interruptible);
      // After the block operation, our thread is going to be locked.  Make sure to
      // drop the lock before exiting.
      current_thread->get_lock().Release();
      return result;
    }
    return ChainLockTransaction::Action::Backoff;
  };
  return ChainLockTransaction::UntilDone(
      IrqSaveOption, CLT_TAG("OwnedWaitQueue::BlockAndAssignOwner"), do_transaction);
}

zx_status_t OwnedWaitQueue::BlockAndAssignOwnerLocked(Thread* const current_thread,
                                                      const Deadline& deadline,
                                                      const BAAOLockingDetails& details,
                                                      ResourceOwnership resource_ownership,
                                                      Interruptible interruptible,
                                                      ForceInheritance force_inheritance) {
  DEBUG_ASSERT(current_thread == Thread::Current::Get());
  DEBUG_ASSERT(magic() == kOwnedMagic);
  DEBUG_ASSERT(current_thread->state() == THREAD_RUNNING);
  const ChainLockTransaction& active_clt = ChainLockTransaction::ActiveRef();

  // TODO(johngro):
  //
  // Locking for this is more tricky than it seems.  If we have an original
  // owner (A), and a new owner (B), and both of them are blocked, and both of
  // them share the same PI target, simply attempting to lock both PI chains
  // would result in deadlock detection.
  //
  // What really needs to happen here is that, as we lock for a BAAO operation
  // which involves changing owners, we need to lock the current owner chain
  // first (before the new owner chain) and remove the current existing owner.
  // Once that is finished, we can attempt to lock the new-owner chain (again,
  // if any).  If this fails, we need to be prepared to back-off and try again,
  // including the possibility that we might need to drop the queue lock, and
  // displace yet another new owner in the process.
  //
  // We should not need to check for any cycles at this point, locking should
  // have taken care of that for us. If the there is a change of owner
  // happening, start by releasing the current owner.  After this, the PI chain
  // starting from our previous owner (if any) should be released.
  const bool owner_changed = (owner_ != details.new_owner);
  if (owner_changed && (owner_ != nullptr)) {
    // Remove our old owner, then release the locks which had been held on the
    // old owner path, stopping early if we need to do to (see
    // LockForBAAOOperation for details).  Note, if the old owner being removed
    // is the same as the thread which is currently blocking, there is nothing
    // to unlock.  We need to continue to hold the current thread's lock until
    // it blocks, and there cannot be anything downstream of the current thread
    // since it is currently running.
    Thread* old_owner = owner_;
    AssignOwnerInternal(nullptr);
    if (current_thread != old_owner) {
      // Do not unlock the old owner chain if the old owner is the same as the
      // stop point.  This can happen if the new owner chain intersects the old
      // owner chain start at the old owner itself. In this case, the new owner
      // chain is a superset of the old owner chain, and all of the locks will
      // be properly released as we finish propagation down the new owner chain.
      if (old_owner != details.stop_point) {
        old_owner->get_lock().AssertAcquired();
        UnlockPiChainCommon(*old_owner, details.stop_point);
      } else {
        old_owner->get_lock().AssertHeld();
      }
    } else {
      DEBUG_ASSERT(details.stop_point == current_thread);
    }
  }

  // Perform the first half of the BlockEtc operation.  This will attempt to add
  // an edge between the thread which is blocking, and the OWQ it is blocking in
  // (this).  We know that this cannot produce a cycle in the graph because we
  // know that this OWQ does not currently have an owner.
  //
  // If the block preamble fails, then the state of the actual wait queue is
  // unchanged and we can just get out now.
  zx_status_t res =
      BlockEtcPreamble(current_thread, deadline, 0u, resource_ownership, interruptible);
  if (res != ZX_OK) {
    // There are only three reasons why the pre-wait operation should ever fail.
    //
    // 1) ZX_ERR_TIMED_OUT            : The timeout has already expired.
    // 2) ZX_ERR_INTERNAL_INTR_KILLED : The thread has been signaled for death.
    // 3) ZX_ERR_INTERNAL_INTR_RETRY  : The thread has been signaled for suspend.
    //
    // No matter what, we are not actually going to block in the wait queue.
    // Even so, however, we still need to assign the owner to what was
    // requested by the thread.  Just because we didn't manage to block does
    // not mean that ownership assignment gets skipped.
    ZX_DEBUG_ASSERT((res == ZX_ERR_TIMED_OUT) || (res == ZX_ERR_INTERNAL_INTR_KILLED) ||
                    (res == ZX_ERR_INTERNAL_INTR_RETRY));

    // If the owner was changing, the existing owner should already be nullptr
    // at this point.  If we have a new owner to assign, do so now, and unlock
    // the rest of the PI chain.
    //
    // Otherwise, if the owner has not changed, and we still have an old owner,
    // don't forget to unlock the old owner chain before propagating our error
    // out.
    if (owner_changed) {
      DEBUG_ASSERT(owner_ == nullptr);
      if (details.new_owner != nullptr) {
        details.new_owner->get_lock().AssertAcquired();
        AssignOwnerInternal(details.new_owner);
        UnlockPiChainCommon(*details.new_owner);
      }
    } else if (owner_ != nullptr) {
      owner_->get_lock().AssertAcquired();
      UnlockPiChainCommon(*owner_);
    }

    // Now drop our lock and get out.  We should be holding only the current
    // thread's lock as we exit.
    get_lock().Release();
    active_clt.AssertNumLocksHeld(1);
    return res;
  }

  // We succeeded in placing our thread into our wait collection.  Make sure we
  // update the scheduler state storage location if needed, then propagate the
  // effects down the chain.
  UpdateSchedStateStorageThreadAdded(*current_thread);
  BeginPropagate(*current_thread, *this, AddSingleEdgeOp, force_inheritance);

  // If we have an new_owner, we need to do one of two things.
  //
  // 1) If the owner has not changed, then the PI consequences were propagated
  //    when we added the edge from the blocking thread to this OWQ.  We just need
  //    to unlock the PI chain starting from the already existing owner and we
  //    should be finished.
  // 2) If the owner changed, then we should have unassigned the original owner
  //    at the start of this routine.  We need to assign the new owner now, and
  //    then unlock the chain starting from the new owner.
  //
  if (owner_changed) {
    DEBUG_ASSERT(owner_ == nullptr);
    if (details.new_owner != nullptr) {
      details.new_owner->get_lock().AssertAcquired();
      AssignOwnerInternal(details.new_owner);
      UnlockPiChainCommon(*details.new_owner);
    }
  } else {
    if (owner_ != nullptr) {
      owner_->get_lock().AssertAcquired();
      UnlockPiChainCommon(*owner_);
    }
  }

  // OK, we are almost done at this point in time.  We have dealt with all of
  // the ownership and PI related bookkeeping, and we have added the blocking
  // thread to this owned wait queue.  We can now drop the OWQ lock and finish
  // the BAAO operation by calling BlockEtcPostable.  When we return from the
  // block operation, we should have re-acquired the current thread's lock
  // (however, it may be with a different lock token).
  get_lock().Release();
  active_clt.AssertNumLocksHeld(1);

  // Finally, go ahead and run the second half of the BlockEtc operation.
  // This will actually block our thread and handle setting any timeout timers
  // in the process.

  // DANGER!! DANGER!! DANGER!! DANGER!! DANGER!! DANGER!! DANGER!! DANGER!!
  //
  // It is very important that no attempts to access |this| are made after
  // either of the calls to BlockEtcPostamble (below). When someone eventually
  // comes along and unblocks us from the queue, they have already taken care of
  // removing us from the this wait queue.  It it totally possible that the wait
  // queue we were blocking in has been destroyed by the time we make it out of
  // BlockEtcPostable, making |this| no longer a valid pointer.
  //
  // DANGER!! DANGER!! DANGER!! DANGER!! DANGER!! DANGER!! DANGER!! DANGER!!
  //
  res = BlockEtcPostamble(current_thread, deadline);
  DEBUG_ASSERT(&active_clt == ChainLockTransaction::Active());
  active_clt.AssertNumLocksHeld(1);
  return res;
}

void OwnedWaitQueue::CancelBAAOOperationLocked(Thread* const current_thread,
                                               const BAAOLockingDetails& details) {
  // If there is a current owner, handle unlocking the current owner chain.
  // There are a few special cases to consider here.
  //
  // 1) We don't need to unlock starting from here if the owner wasn't going to
  //    change.  If the existing owner == the proposed new owner, we are going
  //    to unconditionally unlock the new owner chain anyway.
  // 2) If the new owner is the same as the current thread (trying to block)
  //    thread, then the owner must have changed (the blocking thread cannot be the
  //    owning thread), but we don't want to drop the locks yet.  There cannot
  //    be anything downstream of the current thread (it is currently running),
  //    and we are going to unconditionally drop the current thread's lock on
  //    our way out of this method.
  // 3) If we are going to unlock starting from the existing owner's chain, we
  //    need to remember to stop at the stop-point provided by our details.  It
  //    is possible that our old owner, and a proposed new owner, have chains
  //    that intersect down-the-line, and we don't want to try to double release
  //    any of the locks held on the shared path between the two.
  //
  const bool owner_changed = (owner_ != details.new_owner);
  if (owner_changed && (owner_ != nullptr) && (current_thread != owner_)) {
    owner_->get_lock().AssertAcquired();
    UnlockPiChainCommon(*owner_, details.stop_point);
  }

  // If we had a newly proposed owner, unlock its path.  This will also handle
  // unlocking the current owner path in the case that the current owner was not
  // changing.
  if (details.new_owner != nullptr) {
    details.new_owner->get_lock().AssertAcquired();
    UnlockPiChainCommon(*details.new_owner);
  }

  // We should be holding exactly 2 locks at this point in time.  The queue's
  // lock and the current thread's lock.  Drop these and we should be done.
  ChainLockTransaction::Active()->AssertNumLocksHeld(2);
  get_lock().Release();
  current_thread->get_lock().Release();
}

OwnedWaitQueue::WakeThreadsResult OwnedWaitQueue::WakeThreads(uint32_t wake_count,
                                                              IWakeRequeueHook& wake_hooks) {
  WakeThreadsResult result;
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    if (Thread::UnblockList threads;
        LockForWakeOperationOrBackoff(wake_count, wake_hooks, threads)) {
      ChainLockTransaction::Finalize();
      result = WakeThreadsLocked(ktl::move(threads), wake_hooks);
      get_lock().Release();
      return ChainLockTransaction::Done;
    }
    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                  CLT_TAG("OwnedWaitQueue::WakeThreadsResult"), do_transaction);
  return result;
}

OwnedWaitQueue::WakeThreadsResult OwnedWaitQueue::WakeThreadAndAssignOwner(
    IWakeRequeueHook& wake_hooks) {
  WakeThreadsResult result;
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    if (Thread::UnblockList threads; LockForWakeOperationOrBackoff(1, wake_hooks, threads)) {
      ChainLockTransaction::Finalize();
      result = WakeThreadsLocked(ktl::move(threads), wake_hooks, WakeOption::AssignOwner);
      get_lock().Release();
      return ChainLockTransaction::Done;
    }
    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                  CLT_TAG("OwnedWaitQueue::WakeThreadsResult"), do_transaction);
  return result;
}

OwnedWaitQueue::WakeThreadsResult OwnedWaitQueue::WakeAndRequeue(
    OwnedWaitQueue& requeue_target, Thread* new_requeue_owner, uint32_t wake_count,
    uint32_t requeue_count, IWakeRequeueHook& wake_hooks, IWakeRequeueHook& requeue_hooks,
    WakeOption wake_option) {
  WakeThreadsResult result;
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    if (RequeueLockingDetails details;
        LockForRequeueOperationOrBackoff(requeue_target, new_requeue_owner, wake_count,
                                         requeue_count, wake_hooks, requeue_hooks, details)) {
      ChainLockTransaction::Finalize();
      result =
          WakeAndRequeueInternal(details, requeue_target, wake_hooks, requeue_hooks, wake_option);
      return ChainLockTransaction::Done;
    }
    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                  CLT_TAG("OwnedWaitQueue::WakeAndRequeue"), do_transaction);
  return result;
}

OwnedWaitQueue::WakeThreadsResult OwnedWaitQueue::WakeAndRequeueInternal(
    RequeueLockingDetails& details, OwnedWaitQueue& requeue_target, IWakeRequeueHook& wake_hooks,
    IWakeRequeueHook& requeue_hooks, WakeOption wake_option) {
  DEBUG_ASSERT(magic() == kOwnedMagic);
  DEBUG_ASSERT(requeue_target.magic() == kOwnedMagic);

  // Step 1, remove the target of the wait-queue (this) and unlock its chain,
  // assuming that it has any unique nodes in it.
  if (owner_ != nullptr) {
    Thread* const old_owner = owner_;
    AssignOwnerInternal(nullptr);

    // Don't attempt to unlock this chain if the old owner of the wake-queue is
    // the same as the old owner stop-point.  The implication here is that the
    // old-wake-queue-owner locking chain overlaps 100% with one of the other
    // chains currently held, but which has yet to be dealt with.  For example,
    // the old owner of the wake-queue could be the same as the old owner of the
    // requeue-target, or the proposed new owner of the requeue-target.  We need
    // to keep holding those chains until later on in the process (after we have
    // dealt with their effects).
    if (old_owner != details.old_wake_queue_owner_stop_point) {
      old_owner->get_lock().AssertAcquired();
      UnlockPiChainCommon(*old_owner, details.old_wake_queue_owner_stop_point);
    }
  }

  // Step 2: If the requeue target has an owner, and that owner is changing,
  // remove the existing owner and unlock the unique nodes in its chain.
  const bool requeue_owner_changed = requeue_target.owner_ != details.new_requeue_target_owner;
  if (requeue_owner_changed && requeue_target.owner_) {
    Thread* const old_requeue_owner = requeue_target.owner_;
    requeue_target.AssignOwnerInternal(nullptr);
    if (old_requeue_owner != details.old_requeue_target_owner_stop_point) {
      old_requeue_owner->get_lock().AssertAcquired();
      UnlockPiChainCommon(*old_requeue_owner, details.old_requeue_target_owner_stop_point);
    }
  }

  // Step #3, take any of the threads we need to requeue and move them over into
  // the requeue target, transferring the bookkeeping from the wake queue over
  // to the requeue_target, and dropping each of the threads' locks, in the
  // process.
  while (!details.threads.requeue_threads.is_empty()) {
    Thread* const t = details.threads.requeue_threads.pop_front();
    t->get_lock().AssertAcquired();

    // Call the user's hook, allowing them to maintain any internal bookkeeping
    // they need to maintain.
    requeue_hooks.OnWakeOrRequeue(*t);

    // Actually move the thread from this to the requeue_target.
    this->collection_.Remove(t);
    t->wait_queue_state().blocking_wait_queue_ = nullptr;
    this->UpdateSchedStateStorageThreadRemoved(*t);
    BeginPropagate(*t, *this, RemoveSingleEdgeOp);

    requeue_target.collection_.Insert(t);
    t->wait_queue_state().blocking_wait_queue_ = &requeue_target;
    requeue_target.UpdateSchedStateStorageThreadAdded(*t);
    BeginPropagate(*t, requeue_target, AddSingleEdgeOp);

    t->get_lock().Release();
  }

  // Step #4, if we have a new owner for the requeue target, assign it now and
  // drop the locking chain starting from the new owner.  Otherwise, if our
  // owner didn't change, we still need to drop the locking chain starting from
  // the old owner (we had been holding it in order to propagate the PI
  // consequences of adding the newly requeued threads).
  if (requeue_owner_changed) {
    DEBUG_ASSERT(requeue_target.owner_ == nullptr);
    if (details.new_requeue_target_owner != nullptr) {
      // If there is no one waiting in the requeue target, then it is not
      // allowed to have an owner.
      if (!requeue_target.IsEmpty()) {
        details.new_requeue_target_owner->get_lock().AssertHeld();
        requeue_target.AssignOwnerInternal(details.new_requeue_target_owner);
      }

      // Only unlock the new requeue-target owner chain if the new owner != the
      // chain's unlock stopping point.  This can happen when the new requeue
      // target owner specified is a thread which already exists in the
      // wake-queue.  These threads (and the wake queue itself) are already
      // locked, and the locks will be dropped during the final wake operation
      // (below).
      if (details.new_requeue_target_owner != details.new_requeue_target_owner_stop_point) {
        details.new_requeue_target_owner->get_lock().AssertAcquired();
        UnlockPiChainCommon(*details.new_requeue_target_owner,
                            details.new_requeue_target_owner_stop_point);
      }
    }
  } else if (requeue_target.owner_ != nullptr) {
    DEBUG_ASSERT(requeue_target.owner_ == details.new_requeue_target_owner);
    details.new_requeue_target_owner->get_lock().AssertAcquired();
    UnlockPiChainCommon(*details.new_requeue_target_owner,
                        details.new_requeue_target_owner_stop_point);
  }

  // We are finished with the requeue target and can drop its lock now.
  requeue_target.ValidateSchedStateStorage();
  requeue_target.get_lock().Release();

  // Finally, step #5.  Wake the threads we identified as needing to be woken
  // during the locking operation, dealing with ownership assignment in the
  // process.
  WakeThreadsResult wtr =
      WakeThreadsLocked(ktl::move(details.threads.wake_threads), wake_hooks, wake_option);
  get_lock().Release();
  return wtr;
}

// Explicit instantiation of a variant of the generic BeginPropagate method used in
// wait.cc during thread unblock operations.
template void OwnedWaitQueue::BeginPropagate(Thread&, OwnedWaitQueue&, RemoveSingleEdgeTag,
                                             ForceInheritance,
                                             const SchedulerState::EffectiveProfile*);

// Explicit instantiation of the common lock/unlock routines.
template fit::result<ChainLock::Result, const void*>
OwnedWaitQueue::LockPiChainCommon<OwnedWaitQueue::LockingBehavior::RefuseCycle>(Thread& start);
template fit::result<ChainLock::Result, const void*>
OwnedWaitQueue::LockPiChainCommon<OwnedWaitQueue::LockingBehavior::RefuseCycle>(WaitQueue& start);
template void OwnedWaitQueue::UnlockPiChainCommon(Thread& start, const void* stop_point);
template void OwnedWaitQueue::UnlockPiChainCommon(WaitQueue& start, const void* stop_point);
