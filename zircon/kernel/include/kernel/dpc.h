// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_DPC_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_DPC_H_

#include <lib/kconcurrent/chainlock.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/intrusive_double_list.h>
#include <kernel/event.h>
#include <kernel/thread.h>

// Deferred Procedure Calls - queue callback to invoke on the current cpu in thread context.
// Dpcs are executed with interrupts enabled, and do not ever migrate cpus while executing.
// A Dpc may not execute on the original current cpu if it is hotunplugged/offlined.
// Dpcs may block, though this may starve other queued work.

class Dpc : public fbl::DoublyLinkedListable<Dpc*, fbl::NodeOptions::AllowCopyMove> {
 public:
  using Func = void(Dpc*);

  explicit Dpc(Func* func, void* arg = nullptr) : func_(func), arg_(arg) {
    DEBUG_ASSERT(func_ != nullptr);
  }

  template <class ArgType>
  ArgType* arg() {
    return static_cast<ArgType*>(arg_);
  }

  // A single spinlock protects the data members of all DpcRunners.
  //
  // Take care to drop this lock before calling any code that might acquire a chainlock.
  DECLARE_SINGLETON_SPINLOCK(Lock);

 private:
  friend class DpcRunner;

  // Called by DpcRunner.
  void Invoke() TA_EXCL(Dpc::Lock::Get());

  Func* const func_;
  void* const arg_;
};

// A DpcRunner is responsible for running queued Dpcs.  Under the hood, a given runner may manage
// multiple independent queues of Dpcs.
//
// Each cpu maintains a DpcRunner, in its percpu structure.
class DpcRunner {
 public:
  // Initializes this DpcRunner for the current cpu.
  //
  // The calling thread must have hard-affinity to exactly one CPU.
  void InitForCurrentCpu();

  // Begins the DpcRunner shutdown process.
  //
  // Shutting down a DpcRunner is a two-phase process.  This is the first phase.  See
  // |TransitionOffCpu| for the second phase.
  //
  // This method:
  // - tells the owning cpu's DpcRunner's thread to stop servicing its queue then
  // - waits, up to |deadline|, for it to finish any in-progress DPC and join
  //
  // Because this method blocks until the thread has terminated, it is critical that the caller
  // not hold any locks that might be needed by any previously queued DPCs.  Otheriwse, deadlock may
  // occur.
  //
  // Upon successful completion, this DpcRunner may contain unexecuted DPCs and new ones may be
  // added by |Enqueue|.  However, they will not execute (on any cpu) until |TransitionOffCpu| is
  // called.
  //
  // Once |Shutdown| has completed successfully, finish the shutdown process by calling
  // |TransitionOffCpu| on some cpu other than the owning cpu.
  //
  // If |Shutdown| fails, this DpcRunner is left in an undefined state and
  // |TransitionOffCpu| must not be called.
  zx_status_t Shutdown(zx_instant_mono_t deadline);

  // Moves queued Dpcs from |source| to this DpcRunner.
  //
  // This is the second phase of Dpc shutdown.  See |Shutdown|.
  //
  // This must only be called after |Shutdown| has completed successfully.
  //
  // This must only be called on the current cpu.
  void TransitionOffCpu(DpcRunner& source);

  // Dpc are placed into different queues depending on their ServiceLevel.  One queue for those that
  // need to execute with |LowLatency| (e.g. signaling a TimerDispatcher from interrupt context).
  // The other queue (|General|) is for the rest.
  //
  // |LowLatency| Dpcs are executed by a thread with a high-bandwidth scheduler profile so take
  // care to avoid creating undesirable PI interactions.  In other words, don't use |LowLatency|
  // unless you really need to, and be sure that the Dpc won't hold likely-to-be-contested locks for
  // a significant amount of time.  See also https://fxbug.dev/395669867.
  //
  // |General| Dpcs are serviced by a thread with a default scheduler profile.
  enum class QueueType : uint32_t { General, LowLatency };

  // Enqueue |dpc| in the specified queue |type| and signal its worker thread to execute it.
  //
  // |Enqueue| will not block, but it may wait briefly for a spinlock.
  //
  // |Enqueue| may return before or after the Dpc has executed.  It is the
  // caller's responsibility to ensure that a queued Dpc object is not destroyed
  // prior to its execution.
  //
  // Note: it is important that the thread calling Enqueue() not be holding its
  // own lock if there is _any_ possibility that the DPC thread could be
  // involved in a PI interaction with the thread who is performing the enqueue
  // operation.
  //
  // Returns ZX_ERR_ALREADY_EXISTS if |dpc| is already queued.
  static zx_status_t Enqueue(Dpc& dpc, QueueType type = QueueType::General)
      TA_EXCL(chainlock_transaction_token);

 private:
  // Queue encapsulates a list of Dpc tasks and a thread that pops tasks off the list and executes
  // them in order.
  //
  // A Queue must be initialized (|Init|) prior to use.  A Queue that's been initialized must be
  // |Shutdown| before it may be reinitialized.
  class Queue {
   public:
    // Initialize |cpu|'s queue.
    //
    // Creates a thread with |name_prefix| and scheduler |profile| that will service this Queue.
    void Init(cpu_num_t cpu, const char* name_prefix, const SchedulerState::BaseProfile& profile);

    // Begins the shutdown process for this Queue.  After a successful |Shutdown|, this Queue's
    // remaining contents should be moved into elsewhere by calling |TakeFromLocked| on another
    // Queue.
    //
    // Signals the worker thread to terminate and waits until it has terminated or |deadline| is
    // reached.  If shutdown fails, the object is left in an inconsistent state.
    zx_status_t Shutdown(zx_instant_mono_t deadline);

    // Moves any queued Dpcs from |source| to |this|.  May only be called after a successful
    // |Shutdown| of |source|.
    //
    // Returns true if any dpcs were moved.
    bool TakeFromLocked(Queue& source) TA_REQ(Dpc::Lock::Get());

    void EnqueueLocked(Dpc& dpc) TA_REQ(Dpc::Lock::Get());
    void Signal() TA_EXCL(Dpc::Lock::Get());

   private:
    int DoWork();

    // Request the thread_ to stop by setting to true.
    //
    // This guarded by the static global dpc_lock.
    bool stop_ TA_GUARDED(Dpc::Lock::Get()) = false;

    fbl::DoublyLinkedList<Dpc*> list_ TA_GUARDED(Dpc::Lock::Get());

    // The thread that executes DPCs queued in list_.
    Thread* thread_ TA_GUARDED(Dpc::Lock::Get()) = nullptr;

    Event event_;
  };

  static int WorkerThread(void* unused);
  int Work();

  // The cpu that owns this DpcRunner.
  cpu_num_t cpu_ TA_GUARDED(Dpc::Lock::Get()) = INVALID_CPU;

  // Whether the DpcRunner has been initialized for the owning cpu.
  bool initialized_ TA_GUARDED(Dpc::Lock::Get()) = false;

  Queue queue_general_;
  Queue queue_low_latency_;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_DPC_H_
