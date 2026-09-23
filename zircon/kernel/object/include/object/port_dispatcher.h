// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PORT_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PORT_DISPATCHER_H_

#include <lib/object_cache.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/syscalls/port.h>
#include <zircon/types.h>

#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <kernel/mutex.h>
#include <kernel/semaphore.h>
#include <kernel/spinlock.h>
#include <ktl/unique_ptr.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/signal_observer.h>

// PortObserver lifetime and locking notes.
//
// PortDispatcher is responsible for destroying PortObservers (MaybeReap
// or on_zero_handles), however, their destruction may be initiated by
// either Dispatcher or PortDispatcher.
//
// PortObservers can exist on up to two doubly linked lists:
// - PortObserver::List on PortDispatcher
// - DoublyLinkedList<SignalObserver*> on Dispatcher
//
// The diagrams below show the *relevant* pointers on different
// states of the system. The pure header view is really the
// union of all these pointer which can be confusing.
//
// rc = ref counted
// p  = raw pointer
// o =  owning pointer
//
// 1) Situation after object_wait_async(port, handle) is issued:
//
//
//                                   list   +--------+
//          +------p------+      +----p-----+  Port  |
//          |             v      v          |        |
//  +-------+--+        +-----------+       +-+------+
//  | object   |        | Port      |         ^
//  |          | <--p---+ Observer  |         |
//  +----------+        |           +---rc----+
//                      |           |
//                      +-----------+
//                      |  Port     |
//                      |  Packet   |
//                      +-----------+
//
//   State changes of the object are propagated from the object
//   to the port via |p| --> observer --> |rc| calls.
//
// 2) Situation after the packet is queued on signal match or the wait
//    is canceled through the Dispatcher.
//
//                                          +--------+
//                                          |  Port  |
//                                          |        |
//  +----------+        +-----------+       +-+---+--+
//  | object   |        | Port      |         ^   |
//  |          |        | Observer  |         |   |
//  +----------+        |           +---rc----+   |
//                +---> |           |             |
//                |     +-----------+             | list
//                |     |  Port     |             |
//                +-rc--|  Packet   | <-----o-----+
//                      +-----------+
//
//   Note that the object no longer has a |p| to the observer
//   but the observer still owns the port via |rc|.
//
//   The |o| pointer is used to destroy the port observer only
//   when cancellation happens and the port still owns the packet.
//
//   The PortObserver's raw pointer to the Dispatcher is a weak reference
//   guarded by the port's lock. It is cleared either in MaybeReap or
//   in PortDispatcher::on_zero_handles.
//
// Cancelation ordering
//
// Observers can be canceled by way of the object or the port. Observers are
// canceled by the object when the object's handle count reaches zero and when
// waits are canceled by port_cancel. In this path the Dispatcher's lock is
// acquired first and the observer is removed from the Dispatcher's observer
// list, then the PortDispatcher lock is acquired and the observer is removed
// from the PortDispatcher's observer list and may be cleaned up.
//
// When an observer is canceled through the port by way of port_cancel_key there
// is a lock ordering issue. The PortDispatcher lock must be held to iterate
// over pending observers but the locks for Dispatchers associated with these
// observers cannot be acquired while holding the PortDispatcher lock. In this
// case, the PortDispatcher first cancels each pending observer with the
// PortDispatcher lock held by assigning the observer's packet's type to a
// sentinel value and removing them from the PortDispatcher's list to a
// temporary list. Then the PortDispatcher lock is released and the observers
// are deregistered from their Dispatchers (acquiring the Dispatcher lock) one
// at a time. If an observer fires in this state and attempts to queue a
// packet PortDispatcher will check the type field and not queue the packet.
//
// Locking
//
// The PortDispatcher's lock guards PortDispatcher's list of observers, the
// PortObserver's reference to its Dispatcher, and the packet type field on the
// PortObserver's stored zx_port_packet_t in pending observers. The
// PortDispatcher lock can be acquired while holding the Dispatcher's lock.
//
// The Dispatcher's lock guards the Dispatcher's list of observers and the
// other fields on PortObserver, most notably the packet itself. The
// dispatcher lock cannot be acquired while holding the PortDispatcher lock.
//
// Both the PortDispatcher and Dispatcher locks are held when delivering a
// packet and when deciding whether to destroy it in MaybeReap.

class PortDispatcher;
class PortObserver;
struct PortPacket;

struct PortAllocator {
  virtual ~PortAllocator() = default;

  virtual PortPacket* Alloc() = 0;
  virtual void Free(PortPacket* port_packet) = 0;
};

constexpr zx_packet_type_t kPortPacketTypeCanceled = 0xffffffff;

struct PortPacket final : public fbl::DoublyLinkedListable<PortPacket*> {
  using List = fbl::DoublyLinkedList<PortPacket*>;
  zx_port_packet_t packet;
  const void* const handle;
  object_cache::UniquePtr<const PortObserver> observer;
  PortAllocator* const allocator;

  PortPacket(const void* handle, PortAllocator* allocator);
  PortPacket(const PortPacket&) = delete;
  void operator=(PortPacket) = delete;

  uint64_t key() const { return packet.key; }
  bool is_ephemeral() const { return allocator != nullptr; }
  void Free() { allocator->Free(this); }
  void Cancel() { packet.type = kPortPacketTypeCanceled; }
  bool is_canceled() const { return packet.type == kPortPacketTypeCanceled; }
};

struct PortInterruptPacket final : public fbl::DoublyLinkedListable<PortInterruptPacket*> {
  zx_instant_boot_t timestamp;
  uint64_t key;
};

// Observers are weakly contained in Dispatchers.
class PortObserver final : public SignalObserver {
 public:
  using ListNodeState = fbl::DoublyLinkedListNodeState<PortObserver*>;

  // ListTraits allows PortObservers to be placed on a PortObserver::List.
  struct ListTraits {
    static ListNodeState& node_state(PortObserver& obj) { return obj.observer_list_node_state_; }
  };

  using List = fbl::DoublyLinkedListCustomTraits<PortObserver*, PortObserver::ListTraits,
                                                 fbl::SizeOrder::Constant>;

  PortObserver(uint32_t options, const Handle* handle, fbl::RefPtr<PortDispatcher> port,
               uint64_t key, zx_signals_t signals);

  ~PortObserver() final;

  // May only be called while holding PortDispatcher lock.
  Dispatcher* UnlinkDispatcherLocked() {
    DEBUG_ASSERT(DispatcherLockIsHeld());
    Dispatcher* dispatcher = dispatcher_;
    dispatcher_ = nullptr;
    return dispatcher;
  }

  // Manually increment the reference count of our dispatcher.
  // May only be called while holding PortDispatcher lock.
  void AddDispatcherRefLocked() {
    DEBUG_ASSERT(DispatcherLockIsHeld());
    dispatcher_->AddRef();
  }

  // May only be called while holding PortDispatcher lock.
  void Cancel() {
    DEBUG_ASSERT(DispatcherLockIsHeld());
    packet_.Cancel();
  }

 private:
  PortObserver(const PortObserver&) = delete;
  PortObserver& operator=(const PortObserver&) = delete;

  // |SignalObserver| implementation.
  void OnMatch(zx_signals_t signals, OwnedWaitQueue* queue_to_own) final;
  void OnCancel(zx_signals_t signals) final;
  bool MatchesKey(const void* port, uint64_t key) final;

  // Defined below after PortDispatcher.
  inline bool DispatcherLockIsHeld() const;

  PortPacket packet_;

  fbl::RefPtr<PortDispatcher> const port_;

  // Guarded by port_->get_lock();
  ListNodeState observer_list_node_state_;

  // Guarded by port_->get_lock();
  // This is a raw pointer as the PortObserver does not have an ownership stake in the dispatcher.
  Dispatcher* dispatcher_;
};

// The PortDispatcher implements the port kernel object which is the cornerstone
// for waiting on object changes in Zircon. The PortDispatcher handles 3 usage
// cases:
//  1- Object state change notification: zx_object_wait_async()
//  2- Manual queuing: zx_port_queue()
//  3- Interrupt change notification: zx_interrupt_bind()
//
// This makes the implementation non-trivial. Cases 1 and 2 use the |packets_|
// linked list and case 3 uses |interrupt_packets_| linked list.
//
// The threads that wish to receive notifications block on Dequeue() (which
// maps to zx_port_wait()) and will receive packets from any of the four sources
// depending on what kind of object the port has been 'bound' to.
//
// When a packet from any of the sources arrives to the port, one waiting
// thread unblocks and gets the packet. In all cases |sema_| is used to signal
// and manage the waiting threads.

class PortDispatcher final : public SoloDispatcher<PortDispatcher, ZX_DEFAULT_PORT_RIGHTS> {
 public:
  static PortAllocator* DefaultPortAllocator();
  static zx_status_t Create(uint32_t options, KernelHandle<PortDispatcher>* handle,
                            zx_rights_t* rights);

  ~PortDispatcher() final;
  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_PORT; }

  bool can_bind_to_interrupt() const { return options_ & ZX_PORT_BIND_TO_INTERRUPT; }
  void on_zero_handles() final;

  // Queues a packet on the port.
  zx_status_t Queue(PortPacket* port_packet);

  // Queues a packet based on a signal observation and deregisters the observer from the port.
  // If |observer| is non-null, also removes it from the observer list.
  zx_status_t QueueAndRemoveObserver(PortPacket* port_packet, zx_signals_t observed,
                                     PortObserver* observer = nullptr,
                                     OwnedWaitQueue* queue_to_own = nullptr);

  // Queues a user packet.
  zx_status_t QueueUser(const zx_port_packet_t& packet);

  // Queues an interrupt packet.
  bool QueueInterruptPacket(PortInterruptPacket* port_packet, zx_instant_boot_t timestamp);
  zx_status_t Dequeue(const Deadline& deadline, zx_port_packet_t* packet);
  bool RemoveInterruptPacket(PortInterruptPacket* port_packet);

  // This method determines the observer's fate. Upon return, one of the following will have
  // occurred:
  //
  // 1. The observer is destroyed.
  //
  // 2. The observer is linked to an alreadyed queued packet and will be destroyed when the packet
  // is destroyed (Queued or CancelQueued).
  //
  // 3. The observer is left for on_zero_handles to destroyed.
  void MaybeReap(PortObserver* observer, PortPacket* port_packet);

  // Creates an observer registered with this port.
  //
  // Requires that a read lock is held on the handle table containing |handle|.
  zx::result<SignalObserver*> MakeObserver(uint32_t options, const Handle* handle, uint64_t key,
                                           zx_signals_t signals);

  // Returns true if at least one packet was removed from the queue.
  // Called under the handle table lock when |handle| is not null.
  // When |handle| is null, ephemeral PortPackets are removed from the queue but not freed.
  bool CancelQueued(const void* handle, uint64_t key);

  // Removes |port_packet| from this port's queue. Returns false if the packet was
  // not in this queue. It is undefined to call this with a packet queued in another port.
  bool CancelQueued(PortPacket* port_packet);

  // Cancel all pending waits and packets registered with |key|.
  // Returns ZX_OK if any waits or packets were canceled and ZX_ERR_NOT_FOUND if
  // no matching waits or packets were found.
  zx_status_t CancelKey(uint64_t key);

  // Init hook that sets up the cache allocators used by this dispatcher.
  static void InitializeCacheAllocators(uint32_t level);

  // Returns true if the dispatcher lock |get_lock()| is currently held.
  bool DispatcherLockIsHeld() const { return get_lock()->lock().IsHeld(); }

 private:
  explicit PortDispatcher(uint32_t options);

  // Cancel all queued port packets matching |handle| (if not nullptr) and |key|.
  // Any canceled ephemeral packets are moved to |free_list|; the caller must drain and Free()
  // these with get_lock() released.
  // Returns true if any packets were canceled.
  bool CancelQueuedPacketsLocked(const void* handle, uint64_t key, PortPacket::List* free_list)
      TA_REQ(get_lock());

  zx_status_t QueuePacketLocked(PortPacket* port_packet, zx_signals_t observed) TA_REQ(get_lock());

  // Unlinks an observer from its dispatcher and remove it from our observer
  // list if it is not already removed. This may place the observer into
  // |destroyer| for the caller to destroy after releasing get_lock().
  void MaybeReapLocked(PortObserver* observer, PortPacket* port_packet,
                       object_cache::UniquePtr<PortObserver>& destroyer) TA_REQ(get_lock());

  const uint32_t options_;
  Semaphore sema_;
  bool zero_handles_ TA_GUARDED(get_lock());

  // Next three members handle the object and manual notifications.
  size_t num_ephemeral_packets_ TA_GUARDED(get_lock());
  PortPacket::List packets_ TA_GUARDED(get_lock());
  // Next two members handle the interrupt notifications.
  DECLARE_SPINLOCK(PortDispatcher) spinlock_;
  fbl::DoublyLinkedList<PortInterruptPacket*> interrupt_packets_ TA_GUARDED(spinlock_);

  // Keeps track of outstanding observers so they can be removed from dispatchers once handle
  // count drops to zero.
  PortObserver::List observers_ TA_GUARDED(get_lock());
};

inline bool PortObserver::DispatcherLockIsHeld() const { return port_->DispatcherLockIsHeld(); }

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_PORT_DISPATCHER_H_
