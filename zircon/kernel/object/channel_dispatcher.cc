// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/channel_dispatcher.h"

#include <assert.h>
#include <lib/counters.h>
#include <platform.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <kernel/event.h>
#include <ktl/algorithm.h>
#include <object/handle.h>
#include <object/message_packet.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>

#define LOCAL_TRACE 0

KCOUNTER(channel_packet_depth_1, "channel.depth.1")
KCOUNTER(channel_packet_depth_4, "channel.depth.4")
KCOUNTER(channel_packet_depth_16, "channel.depth.16")
KCOUNTER(channel_packet_depth_64, "channel.depth.64")
KCOUNTER(channel_packet_depth_256, "channel.depth.256")
KCOUNTER(channel_packet_depth_unbounded, "channel.depth.unbounded")
KCOUNTER(channel_full, "channel.full")
KCOUNTER(dispatcher_channel_create_count, "dispatcher.channel.create")
KCOUNTER(dispatcher_channel_destroy_count, "dispatcher.channel.destroy")

namespace {

// Temporary hack to chase down bugs like https://fxbug.dev/42123699 where upwards of 250MB of ipc
// memory is consumed. The bet is that even if each message is at max size there
// should be one or two channels with thousands of messages. If so, this check adds
// no overhead to the existing code. See https://fxbug.dev/42124465.
// TODO(cpu): This limit can be lower but mojo's ChannelTest.PeerStressTest sends
// about 3K small messages. Switching to size limit is more reasonable.
constexpr size_t kMaxPendingMessageCount = 3500;
constexpr size_t kWarnPendingMessageCount = kMaxPendingMessageCount / 2;

// This value is part of the zx_channel_call contract.
constexpr uint32_t kMinKernelGeneratedTxid = 0x80000000u;

// The maximum number of handles to include in channel message traces.
constexpr uint32_t kMaxTraceHandles = 4;

bool IsKernelGeneratedTxid(zx_txid_t txid) { return txid >= kMinKernelGeneratedTxid; }

// Randomly generated multilinear hash coefficients. These should be sufficient for non-user builds
// where tracing syscalls are enabled. In the future, if we elect to enable tracing facilities in
// user builds, this can be strengthened by generating the coefficients during boot.
constexpr uint64_t kHashCoefficients[] = {
    0xa573c3ccbd7e2010ULL, 0x165cbcf3a0de8544ULL, 0x8b975f576f025514ULL,
    0xabc406ce862c9a1dULL, 0xf292bea1a3fe6bedULL, 0x1c7c06b8b02b4585ULL,
};

// 64bit to 32bit hash using the multilinear hash family ax + by + c.
inline uint32_t HashValue(uint64_t a, uint64_t b, uint64_t c, uint64_t value) {
  const uint32_t x = static_cast<uint32_t>(value);
  const uint32_t y = static_cast<uint32_t>(value >> 32);
  return static_cast<uint32_t>((a * x + b * y + c) >> 32);
}

// Two hash functions using different randomly generated coefficients.
inline uint32_t HashA(uint64_t value) {
  return HashValue(kHashCoefficients[0], kHashCoefficients[1], kHashCoefficients[2], value);
}
inline uint32_t HashB(uint64_t value) {
  return HashValue(kHashCoefficients[3], kHashCoefficients[4], kHashCoefficients[5], value);
}
inline uint32_t HashB(uint32_t high, uint32_t low) {
  return HashB(static_cast<uint64_t>(high) << 32 | low);
}

// Generates a flow id using a universal hash function of the minimum endpoint koid and the txid or
// message packet address, depending on whether the txid is non-zero.
//
// In general, koids are guaranteed to be unique over the lifetime of a particular system boot.
// Using the min endpoint koid ensures both endpoints use the same hash input. A txid is shared
// between sender and receiver is expected to be unique (guaranteed for kernel-generated txids)
// among the set of txids for messages pending in a particular channel. Likewise, the message packet
// address is shared between the sender and receiver and is guaranteed to be unique among the set of
// pointers to pending messages.
//
// Given that the (koid, txid) or (koid, &msg) pair is likely to be unique over the span of the
// flow, the likelihood of id confusion is equivalent to the likelihood of hash collisions by
// temporally overlapping flows.
uint64_t ChannelMessageFlowId(const MessagePacket& msg, const ChannelDispatcher* channel) {
  const zx_koid_t min_koid = ktl::min(channel->get_koid(), channel->get_related_koid());

  // Use the top bit of the message id to indicate whether the input was a txid, which can be used
  // to correlate a later response message, or a message pointer, which cannot. The 32 bit txid is
  // combined with the bottom 32 bits of the channel koid as inputs to HashB to improve the
  // uniqueness of the message id.
  const uint32_t is_txid_mask = 1u << 31;
  const uint32_t message_id =
      !IsKernelGeneratedTxid(msg.fidl_header().txid)
          ? HashB(reinterpret_cast<uint64_t>(&msg)) & ~is_txid_mask
          : HashB(msg.fidl_header().txid, static_cast<uint32_t>(min_koid)) | is_txid_mask;

  const uint64_t high = HashA(min_koid);
  const uint64_t low = message_id;
  return high << 32 | low;
}

enum class MessageOp : uint8_t {
  Write,
  Read,
  ChannelCallWriteRequest,
  ChannelCallReadResponse,
};

inline void TraceMessage(const MessagePacket& msg, const ChannelDispatcher* channel,
                         MessageOp message_op) {
  // We emit these trace events non-standardly to work around some compatibility issues:
  //
  // 1) We partially inline the trace macro so that we can purposely emit 0-length durations.
  //
  //    chrome://tracing requires flow events to be contained in a duration. Perfetto requires flows
  //    events to be attached to a "slice". However, the Perfetto viewer treats instant events as
  //    0-length slices. This means that we can assign flows to them, and they get a special easy to
  //    click on arrow instead of a tiny duration bar. Using a 0-length duration gets us nice
  //    instant events in the Perfetto viewer, while still supporting flows in chrome://tracing.
  //
  // 2) Even though we know exactly when the duration ends, we emit a Begin/End pair instead of
  //    using a duration-complete event.
  //
  //    Because we do so little work between creating the duration-complete scope and then emitting
  //    the flow event, if we emit a duration-complete event, the two events may be created with the
  //    same timestamp. Since the duration-complete event is only written when the scope ends, it is
  //    written _after_ the flow event in the trace, causing the flow to not be associated with the
  //    previous event, not it. By using a Begin/End pair, we ensure that though the events have the
  //    same timestamp, they will be read in the correct order and the flow events will be
  //    associated correctly.

  uint64_t ts;
  const auto get_timestamp = [&ts] { return ts = KTrace::Timestamp(); };

  if constexpr (CHANNEL_MESSAGE_BODY_TRACING_ENABLED) {
    zx_info_handle_basic_t handle_info[kMaxTraceHandles];
    const auto get_handle_info = [&handle_info, &msg]() -> ktl::span<const uint8_t> {
      size_t num_handles = ktl::min(msg.num_handles(), kMaxTraceHandles);
      for (size_t i = 0; i < num_handles; ++i) {
        const auto& handle = msg.handles()[i];
        handle_info[i] = {
            .koid = handle->dispatcher()->get_koid(),
            .rights = handle->rights(),
            .type = handle->dispatcher()->get_type(),
            .related_koid = handle->dispatcher()->get_related_koid(),
            .reserved = 0u,
            .padding1 = {},

        };
      }
      return ktl::span<const uint8_t>(reinterpret_cast<const uint8_t*>(handle_info),
                                      sizeof(zx_info_handle_basic_t) * num_handles);
    };

    if (message_op == MessageOp::Write || message_op == MessageOp::ChannelCallWriteRequest) {
      // Record message body when sending.
      KTRACE_DURATION_BEGIN_TIMESTAMP(
          "kernel:ipc", "ChannelMessage", get_timestamp(), ("ordinal", msg.fidl_header().ordinal),
          ("bytes", msg.start_of_payload()), ("handles", get_handle_info()));
    } else {
      // Don't record message body when receiving.
      KTRACE_DURATION_BEGIN_TIMESTAMP("kernel:ipc", "ChannelMessage", get_timestamp(),
                                      ("ordinal", msg.fidl_header().ordinal));
    }
  } else {
    KTRACE_DURATION_BEGIN_TIMESTAMP("kernel:ipc", "ChannelMessage", get_timestamp(),
                                    ("ordinal", msg.fidl_header().ordinal));
  }

  // When the txid is kernel-generated, Read and Write message ops are just steps in the overall
  // flow that is bounded by ChannelCallWriteRequest and ChannelCallReadResponse message ops.
  switch (message_op) {
    case MessageOp::Write:
      if (IsKernelGeneratedTxid(msg.fidl_header().txid)) {
        KTRACE_FLOW_STEP_TIMESTAMP("kernel:ipc", "ChannelFlow", ts,
                                   ChannelMessageFlowId(msg, channel));
        break;
      }
      [[fallthrough]];
    case MessageOp::ChannelCallWriteRequest:
      KTRACE_FLOW_BEGIN_TIMESTAMP("kernel:ipc", "ChannelFlow", ts,
                                  ChannelMessageFlowId(msg, channel));
      break;
    case MessageOp::Read:
      if (IsKernelGeneratedTxid(msg.fidl_header().txid)) {
        KTRACE_FLOW_STEP_TIMESTAMP("kernel:ipc", "ChannelFlow", ts,
                                   ChannelMessageFlowId(msg, channel));
        break;
      }
      [[fallthrough]];
    case MessageOp::ChannelCallReadResponse:
      KTRACE_FLOW_END_TIMESTAMP("kernel:ipc", "ChannelFlow", ts,
                                ChannelMessageFlowId(msg, channel));
      break;
  }

  KTRACE_DURATION_END_TIMESTAMP("kernel:ipc", "ChannelMessage", ts);
}

}  // namespace

// static
int64_t ChannelDispatcher::get_channel_full_count() { return channel_full.SumAcrossAllCpus(); }

// static
zx_status_t ChannelDispatcher::Create(KernelHandle<ChannelDispatcher>* handle0,
                                      KernelHandle<ChannelDispatcher>* handle1,
                                      zx_rights_t* rights) {
  fbl::AllocChecker ac;
  auto holder0 = fbl::AdoptRef(new (&ac) PeerHolder<ChannelDispatcher>());
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  auto holder1 = holder0;

  KernelHandle new_handle0(fbl::AdoptRef(new (&ac) ChannelDispatcher(ktl::move(holder0))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  KernelHandle new_handle1(fbl::AdoptRef(new (&ac) ChannelDispatcher(ktl::move(holder1))));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  new_handle0.dispatcher()->InitPeer(new_handle1.dispatcher());
  new_handle1.dispatcher()->InitPeer(new_handle0.dispatcher());

  *rights = default_rights();
  *handle0 = ktl::move(new_handle0);
  *handle1 = ktl::move(new_handle1);

  return ZX_OK;
}

ChannelDispatcher::ChannelDispatcher(fbl::RefPtr<PeerHolder<ChannelDispatcher>> holder)
    : PeeredDispatcher(ktl::move(holder), ZX_CHANNEL_WRITABLE) {
  kcounter_add(dispatcher_channel_create_count, 1);
}

ChannelDispatcher::~ChannelDispatcher() {
  kcounter_add(dispatcher_channel_destroy_count, 1);

  // At this point the other endpoint no longer holds
  // a reference to us, so we can be sure we're discarding
  // any remaining messages safely.

  // It's not possible to do this safely in on_zero_handles()

  messages_.clear();

  switch (max_message_count_) {
    case 0 ... 1:
      kcounter_add(channel_packet_depth_1, 1);
      break;
    case 2 ... 4:
      kcounter_add(channel_packet_depth_4, 1);
      break;
    case 5 ... 16:
      kcounter_add(channel_packet_depth_16, 1);
      break;
    case 17 ... 64:
      kcounter_add(channel_packet_depth_64, 1);
      break;
    case 65 ... 256:
      kcounter_add(channel_packet_depth_256, 1);
      break;
    default:
      kcounter_add(channel_packet_depth_unbounded, 1);
      break;
  }
}

void ChannelDispatcher::RemoveWaiter(MessageWaiter* waiter) {
  Guard<CriticalMutex> guard{get_lock()};
  if (!waiter->InContainer()) {
    return;
  }
  waiters_.erase(*waiter);
}

void ChannelDispatcher::CancelMessageWaitersLocked(zx_status_t status) {
  while (!waiters_.is_empty()) {
    MessageWaiter* waiter = waiters_.pop_front();
    waiter->Cancel(status);
  }
}

void ChannelDispatcher::on_zero_handles_locked() {
  canary_.Assert();

  // (3A) Abort any waiting Call operations because we've been canceled by reason of our local
  // handle going away.
  CancelMessageWaitersLocked(ZX_ERR_CANCELED);
}

void ChannelDispatcher::set_owner(zx_koid_t new_owner) {
  // Testing for ZX_KOID_INVALID is an optimization so we don't
  // pay the cost of grabbing the lock when the endpoint moves
  // from the process to channel; the one that we must get right
  // is from channel to new owner.
  if (new_owner == ZX_KOID_INVALID) {
    return;
  }

  Guard<CriticalMutex> get_lock_guard{get_lock()};
  Guard<CriticalMutex> messages_guard{&channel_lock_};
  owner_ = new_owner;
}

// This requires holding the shared channel lock. The thread analysis
// can reason about repeated calls to get_lock() on the shared object,
// but cannot reason about the aliasing between left->get_lock() and
// right->get_lock(), which occurs above in on_zero_handles.
void ChannelDispatcher::OnPeerZeroHandlesLocked() {
  canary_.Assert();

  {
    Guard<CriticalMutex> messages_guard{&channel_lock_};
    peer_has_closed_ = true;
  }

  UpdateStateLocked(ZX_CHANNEL_WRITABLE, ZX_CHANNEL_PEER_CLOSED);
  // (3B) Abort any waiting Call operations because we've been canceled by reason of the opposing
  // endpoint going away.
  CancelMessageWaitersLocked(ZX_ERR_PEER_CLOSED);
}

// This method should never acquire |get_lock()|.  See the comment at |channel_lock_| for details.
zx_status_t ChannelDispatcher::Read(zx_koid_t owner, uint32_t* msg_size, uint32_t* msg_handle_count,
                                    MessagePacketPtr* msg, bool may_discard) {
  canary_.Assert();

  auto max_size = *msg_size;
  auto max_handle_count = *msg_handle_count;

  Guard<CriticalMutex> guard{&channel_lock_};

  if (owner != owner_) {
    return ZX_ERR_BAD_HANDLE;
  }

  if (messages_.is_empty()) {
    return peer_has_closed_ ? ZX_ERR_PEER_CLOSED : ZX_ERR_SHOULD_WAIT;
  }

  *msg_size = messages_.front().data_size();
  *msg_handle_count = messages_.front().num_handles();
  zx_status_t status = ZX_OK;
  if (*msg_size > max_size || *msg_handle_count > max_handle_count) {
    if (!may_discard) {
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    status = ZX_ERR_BUFFER_TOO_SMALL;
  }

  *msg = messages_.pop_front();
  if (messages_.is_empty()) {
    ClearSignals(ZX_CHANNEL_READABLE);
  }
  if (status == ZX_OK) {
    // If status is OK then we popped a non-null message from messages_.
    TraceMessage(**msg, this, MessageOp::Read);
  }

  return status;
}

zx_status_t ChannelDispatcher::Write(zx_koid_t owner, MessagePacketPtr msg) {
  canary_.Assert();

  Guard<CriticalMutex> guard{get_lock()};

  DEBUG_ASSERT(msg);
  TraceMessage(*msg, this, MessageOp::Write);

  // Failing this test is only possible if this process has two threads racing:
  // one thread is issuing channel_write() and one thread is moving the handle
  // to another process.
  if (owner != owner_) {
    return ZX_ERR_BAD_HANDLE;
  }

  if (!peer()) {
    return ZX_ERR_PEER_CLOSED;
  }

  AssertHeld(*peer()->get_lock());

  if (peer()->TryWriteToMessageWaiter(msg)) {
    return ZX_OK;
  }

  peer()->WriteSelf(ktl::move(msg), nullptr);

  return ZX_OK;
}

zx_txid_t ChannelDispatcher::GenerateTxid() {
  // Values 1..kMinKernelGeneratedTxid are reserved for userspace.
  return (++txid_) | kMinKernelGeneratedTxid;
}

zx_status_t ChannelDispatcher::Call(zx_koid_t owner, MessagePacketPtr msg,
                                    zx_instant_mono_t deadline, MessagePacketPtr* reply) {
  canary_.Assert();

  ChannelDispatcher::MessageWaiter* waiter = ThreadDispatcher::GetCurrent()->GetMessageWaiter();
  if (unlikely(waiter->BeginWait(fbl::RefPtr(this)) != ZX_OK)) {
    // If a thread tries BeginWait'ing twice, the VDSO contract around retrying
    // channel calls has been violated.  Shoot the misbehaving process.
    ProcessDispatcher::GetCurrent()->Kill(ZX_TASK_RETCODE_VDSO_KILL);
    return ZX_ERR_BAD_STATE;
  }

  {
    // Use time limited preemption deferral while we hold this lock.  If our
    // server is running with a deadline profile, (and we are not) then after we
    // queue the message and signal the server, it is possible that the server
    // thread:
    //
    // 1) Gets assigned to our core.
    // 2) It reads the message we just sent.
    // 3) It processes the message and responds with a write to this channel
    //    before we get a chance to drop the lock.
    //
    // This will result in an undesirable thrash sequence where:
    //
    // 1) The server thread contests the lock we are holding.
    // 2) It suffers through the adaptive mutex spin (but it is on our CPU, so
    //    it will never discover that the lock is available)
    // 3) It will then drop into a block transmitting its profile pressure, and
    //    allowing us to run again.
    // 4) we will run for a very short time until we finish our notifications.
    // 5) As soon as we drop the lock, we will immediately bounce back to the
    //    server thread which will complete its operation.
    //
    // Hard disabling preemption helps to avoid this thrash, but comes with a
    // caveat.  It may be that the observer list we need to notify is Very Long
    // and takes a significant amount of time to filter and signal.  We _really_
    // do not want to be running with preemption disabled for very long as it
    // can hold off time critical tasks.  So instead of hard disabling
    // preemption we use CriticalMutex and rely on it to provide time-limited
    // preemption deferral.
    //
    // TODO(johngro): Even with time-limited preemption deferral, this
    // mitigation is not ideal.  We would much prefer an approach where we do
    // something like move the notification step outside of the lock, or break
    // the locks protecting the two message and waiter queues into two locks
    // instead of a single shared lock, so that we never have to defer
    // preemption.  Such a solution gets complicated however, owning to
    // lifecycle issues for the various SignalObservers, and the common locking
    // structure of PeeredDispatchers.  See https://fxbug.dev/42050802.  TL;DR - someday, when
    // we have had the time to carefully refactor the locking here, come back
    // and remove the use of CriticalMutex.
    //
    Guard<CriticalMutex> guard{get_lock()};

    // See Write() for an explanation of this test.
    if (owner != owner_) {
      waiter->EndWait(reply);
      return ZX_ERR_BAD_HANDLE;
    }

    if (!peer()) {
      waiter->EndWait(reply);
      return ZX_ERR_PEER_CLOSED;
    }

  alloc_txid:
    const zx_txid_t txid = GenerateTxid();

    // If there are waiting messages, ensure we have not allocated a txid
    // that's already in use.  This is unlikely.  It's atypical for multiple
    // threads to be invoking channel_call() on the same channel at once, so
    // the waiter list is most commonly empty.
    for (ChannelDispatcher::MessageWaiter& w : waiters_) {
      if (w.get_txid() == txid) {
        goto alloc_txid;
      }
    }

    // Install our txid in the waiter and the outbound message
    waiter->set_txid(txid);
    msg->set_txid(txid);

    TraceMessage(*msg, this, MessageOp::ChannelCallWriteRequest);

    // (0) Before writing the outbound message and waiting, add our
    // waiter to the list.
    waiters_.push_back(waiter);

    // (1) Write outbound message to opposing endpoint.
    AssertHeld(*peer()->get_lock());
    peer()->WriteSelf(ktl::move(msg), waiter->get_wait_queue());
  }

  auto process = ProcessDispatcher::GetCurrent();
  const TimerSlack slack = process->GetTimerSlackPolicy();
  const Deadline slackDeadline(deadline, slack);

  // Reuse the code from the half-call used for retrying a Call after thread
  // suspend.
  return ResumeInterruptedCall(waiter, slackDeadline, reply);
}

zx_status_t ChannelDispatcher::ResumeInterruptedCall(MessageWaiter* waiter,
                                                     const Deadline& deadline,
                                                     MessagePacketPtr* reply) {
  canary_.Assert();

  // (2) Wait for notification via waiter's event or for the
  // deadline to hit.
  {
    ThreadDispatcher::AutoBlocked by(ThreadDispatcher::Blocked::CHANNEL);

    zx_status_t status = waiter->Wait(deadline);
    if (status == ZX_ERR_INTERNAL_INTR_RETRY) {
      // If we got interrupted, return out to usermode, but
      // do not clear the waiter.
      return status;
    }
  }

  // (3) see (3A), (3B) above or (3C) below for paths where
  // the waiter could be signaled and removed from the list.
  //
  // If the deadline hits, the waiter is not removed
  // from the list *but* another thread could still
  // cause (3A), (3B), or (3C) before the lock below.
  {
    Guard<CriticalMutex> guard{get_lock()};

    // (4) If any of (3A), (3B), or (3C) have occurred,
    // we were removed from the waiters list already
    // and EndWait() returns a non-ZX_ERR_TIMED_OUT status.
    // Otherwise, the status is ZX_ERR_TIMED_OUT and it
    // is our job to remove the waiter from the list.
    zx_status_t status = waiter->EndWait(reply);
    if (status == ZX_ERR_TIMED_OUT) {
      waiters_.erase(*waiter);
    }

    if (*reply) {
      TraceMessage(**reply, this, MessageOp::ChannelCallReadResponse);
    }

    return status;
  }
}

bool ChannelDispatcher::TryWriteToMessageWaiter(MessagePacketPtr& msg) {
  canary_.Assert();

  if (waiters_.is_empty()) {
    return false;
  }

  // If the far side has "call" waiters waiting for replies, see if this message's txid matches one
  // of them.  If so, deliver it.  Note, because callers use a kernel generated txid we can skip
  // checking the list if this message's txid isn't kernel generated.
  const zx_txid_t txid = msg->get_txid();
  if (!IsKernelGeneratedTxid(txid)) {
    return false;
  }

  for (auto& waiter : waiters_) {
    // (3C) Deliver message to waiter.
    // Remove waiter from list.
    if (waiter.get_txid() == txid) {
      waiters_.erase(waiter);
      waiter.Deliver(ktl::move(msg));
      return true;
    }
  }

  return false;
}

void ChannelDispatcher::WriteSelf(MessagePacketPtr msg, OwnedWaitQueue* queue_to_own) {
  canary_.Assert();

  // Once we've acquired the channel_lock_ we're going to make a copy of the previously active
  // signals and raise the READABLE signal before dropping the lock.  After we've dropped the lock,
  // we'll notify observers using the previously active signals plus READABLE.
  //
  // There are several things to note about this sequence:
  //
  // 1. We must hold channel_lock_ while updating the stored signals (RaiseSignalsLocked) to
  // synchronize with thread adding, removing, or canceling observers otherwise we may create a
  // spurious READABLE signal (see NoSpuriousReadableSignalWhenRacing test).
  //
  // 2. We must release the channel_lock_ before notifying observers to ensure that Read can execute
  // concurrently with NotifyObserversLocked, which is a potentially long running call.
  //
  // 3. We can skip the call to NotifyObserversLocked if the previously active signals contained
  // READABLE (because there can't be any observers still waiting for READABLE if that signal is
  // already active).
  zx_signals_t previous_signals;
  {
    Guard<CriticalMutex> guard{&channel_lock_};

    messages_.push_back(ktl::move(msg));
    previous_signals = RaiseSignalsLocked(ZX_CHANNEL_READABLE);
    const size_t size = messages_.size();
    max_message_count_ = ktl::max(size, max_message_count_);
    // TODO(cpu): Remove this hack. See comment in kMaxPendingMessageCount definition.
    if (size >= kWarnPendingMessageCount) {
      if (size == kWarnPendingMessageCount) {
        const auto* process = ProcessDispatcher::GetCurrent();
        char pname[ZX_MAX_NAME_LEN];
        [[maybe_unused]] zx_status_t status = process->get_name(pname);
        DEBUG_ASSERT(status == ZX_OK);
        printf("KERN: warning! channel (%zu) has %zu messages (%s) (peer: %zu) (write).\n",
               get_koid(), size, pname, peer()->owner_);
      } else if (size > kMaxPendingMessageCount) {
        const auto* process = ProcessDispatcher::GetCurrent();
        char pname[ZX_MAX_NAME_LEN];
        [[maybe_unused]] zx_status_t status = process->get_name(pname);
        DEBUG_ASSERT(status == ZX_OK);
        printf(
            "KERN: channel (%zu) has %zu messages (%s) (peer: %zu) (write). Raising exception.\n",
            get_koid(), size, pname, peer()->owner_);
        Thread::Current::SignalPolicyException(ZX_EXCP_POLICY_CODE_CHANNEL_FULL_WRITE, 0u);
        kcounter_add(channel_full, 1);
      }
    }
  }

  // Don't bother waking observers if ZX_CHANNEL_READABLE was already active.
  if ((previous_signals & ZX_CHANNEL_READABLE) == 0) {
    NotifyObserversLocked(previous_signals | ZX_CHANNEL_READABLE, queue_to_own);
  }
}

ChannelDispatcher::MessageWaiter::~MessageWaiter() {
  if (unlikely(channel_)) {
    channel_->RemoveWaiter(this);
  }
  wait_queue_.ResetOwnerIfNoWaiters();
  DEBUG_ASSERT(!InContainer());
}

zx_status_t ChannelDispatcher::MessageWaiter::BeginWait(fbl::RefPtr<ChannelDispatcher> channel) {
  if (unlikely(channel_)) {
    return ZX_ERR_BAD_STATE;
  }
  DEBUG_ASSERT(!InContainer());

  status_ = ZX_ERR_TIMED_OUT;
  channel_ = ktl::move(channel);
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token) -> ChainLockTransaction::Result<> {
    ChainLockGuard guard(wait_queue_.get_lock());
    signaled_ = false;
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                  CLT_TAG("ChannelDispatcher::MessageWaiter::BeginWait"),
                                  do_transaction);
  return ZX_OK;
}

void ChannelDispatcher::MessageWaiter::Signal() {
  // TODO(https://fxbug.dev/477068635): Consider merging this logic back into OwnedWaitQueue.
  auto& wake_hooks = OwnedWaitQueue::default_wake_hooks();
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    if (Thread::UnblockList threads;
        wait_queue_.LockForWakeOperationOrBackoff(UINT32_MAX, wake_hooks, threads)) {
      ChainLockTransaction::Finalize();
      signaled_ = true;
      // TODO(https://fxbug.dev/42182908): Once fair-to-fair priority inheritance is implemented,
      // change this to `ForceInheritance::Yes`.
      wait_queue_.WakeThreadsLocked(ktl::move(threads), wake_hooks,
                                    OwnedWaitQueue::WakeOption::AssignOwner, ForceInheritance::No);
      wait_queue_.get_lock().Release();
      return ChainLockTransaction::Done;
    }
    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                  CLT_TAG("ChannelDispatcher::MessageWaiter::Signal"),
                                  do_transaction);
}

void ChannelDispatcher::MessageWaiter::Deliver(MessagePacketPtr msg) {
  DEBUG_ASSERT(channel_);

  msg_ = ktl::move(msg);
  status_ = ZX_OK;
  Signal();
}

void ChannelDispatcher::MessageWaiter::Cancel(zx_status_t status) {
  DEBUG_ASSERT(!InContainer());
  DEBUG_ASSERT(channel_);
  status_ = status;
  Signal();
}

zx_status_t ChannelDispatcher::MessageWaiter::Wait(const Deadline& deadline) {
  if (unlikely(!channel_)) {
    return ZX_ERR_BAD_STATE;
  }
  // TODO(https://fxbug.dev/477068635): Consider merging this logic back into OwnedWaitQueue.
  Thread* current_thread = Thread::Current::Get();
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token,
                   preempt_disabled_token) -> ChainLockTransaction::Result<zx_status_t> {
    wait_queue_.get_lock().AcquireFirstInChain();
    if (signaled_) {
      wait_queue_.get_lock().Release();
      return ZX_OK;
    }
    Thread* new_owner = wait_queue_.owner();
    if (OwnedWaitQueue::BAAOLockingDetails details;
        wait_queue_.TryLockForBAAOOperationLocked(current_thread, new_owner, details)) {
      ChainLockTransaction::Finalize();
      // TODO(https://fxbug.dev/42182908): Once fair-to-fair priority inheritance is implemented,
      // change this to `ForceInheritance::Yes`.
      const zx_status_t result = wait_queue_.BlockAndAssignOwnerLocked(
          current_thread, deadline, details, ResourceOwnership::Normal, Interruptible::Yes,
          ForceInheritance::No);
      current_thread->get_lock().Release();
      return result;
    }
    wait_queue_.get_lock().Release();
    return ChainLockTransaction::Action::Backoff;
  };
  zx_status_t status = ChainLockTransaction::UntilDone(
      EagerReschedDisableAndIrqSaveOption, CLT_TAG("ChannelDispatcher::MessageWaiter::Wait"),
      do_transaction);
  if (status != ZX_OK) {
    return status;
  }
  return status_;
}

// Returns any delivered message via out and the status.
zx_status_t ChannelDispatcher::MessageWaiter::EndWait(MessagePacketPtr* out) {
  if (unlikely(!channel_)) {
    return ZX_ERR_BAD_STATE;
  }
  *out = ktl::move(msg_);
  channel_ = nullptr;
  // TODO(https://fxbug.dev/513440159): Resetting the owner due to an interrupted channel call
  // breaks the PI chain in a way that cannot be re-connected when the call is resumed. Figure out
  // a way to preserve the PI chain, while address https://fxbug.dev/512083099.
  wait_queue_.ResetOwnerIfNoWaiters();
  return status_;
}
