# ChainLocks and ChainLockTransactions in the kernel

## What is a "ChainLock"?

ChainLocks are a type of synchronization building block similar to a SpinLock.

Exclusive locks generally have ordering requirements which prevent
deadlock.  For example, if there exists a place in code where two locks (`A`,
and `B`) are acquired in the order `A` then `B`, then there cannot exist any
place in code where the locks are acquired in the order `B` then `A` which can
execute concurrently with the first sequence.  Failure to follow this
requirement leads to deadlock potential.

ChainLocks are exclusive locks whose design includes a built in protocol which
can be used to obtain a set of locks in an arbitrary order, even when there
might exist other places in the code where an overlapping set of locks might be
obtained in a different order (which could traditionally lead to deadlock).
They were introduced as part of the effort to remove the kernel's global
`thread_lock` and reduce overall SpinLock contention.

## How do they work?

Internally, the state of a ChainLock is represented by a 64 bit unsigned
integer, referred to as the lock "Token".  A reserved sentinel value of 0 is
used to indicate that the lock is in the unlocked state.

To acquire a set of ChainLocks (`S`), a user must first generate a Token from a
global monotonically increasing token generator.  This Token can be thought of
as a type of priority or ticket.  Lower internal integer values (Tokens
generated earlier in the sequence) have a "higher priority" then Tokens with a
higher internal integer value.  This generated token must be the token used to
obtain *all* of the locks in `S`.

When a thread `A` attempts to obtain a ChainLock `L` with a token `T`, a compare
and exchange of the lock's state with `T` is performed, expecting the state of
the lock to be the unlocked sentinel value.  If this succeeds (the state value
was "unlocked"), the lock has been acquired and `A` can continue to execute.

If the `CMPX` operation fails, it implies that some other thread, `B`, currently
holds the lock using a different Token, `X`.  In this situation, there are three
possibilities.

1) `T` < `X` : `A` is "higher priority" than `B`.  `A` should simply continue to
   attempt to obtain the lock, as it would have done with a SpinLock, until it
   finally succeeds.
2) `T` > `X` : `A` is "lower priority" than `B`.  `A` now needs to back-off,
   dropping _all_ of the ChainLocks it currently holds.  It should then wait a
   short amount of time before re-attempting the operation.  This prevents
   deadlock in the `B` is concurrently attempting to obtain a member of `S`
   which is already held by `A` when `A` is attempting to acquire `L`.
3) `T` == `X` : `A` is attempting to re-acquire `L`, which it already holds, and
   `A` == `B`. This indicates that `A` is following a lock cycle, and would have
   otherwise deadlocked.  Typically, this would be considered to be a fatal
   error, however there are places in kernel code where this can happen because
   of a request made by user-mode code, and the kernel needs to detect the
   situation and implement proper policy to deal with the errant user-mode
   program instead of triggering a panic.

See also //zircon/system/ulib/concurrent/models/chainlock for a TLA+ model of
the locking protocol.  This model shows how the protocol avoids deadlock while
also guaranteeing that all threads will eventually obtain their sets of locks,
regardless of how the sets overlap, or the order in which the locks are
obtained.

## How are they used in the kernel?

With the removal of the global `thread_lock`, new locks needed to be introduced
in order to replace the global locks functionality.  While a number of new
classes of locks needed to be introduced, the primary new lock classes are:

1) A ChainLock for each `Thread` instance (the "thread's lock").
2) A ChainLock for each `WaitQueue` instance.
3) A SpinLock for each Scheduler instance (the Scheduler's "queue_lock")

Because of profile inheritance, `Thread`s and `WaitQueue`s have a relationships
which form "profile inheritance graphs".  Adding and removing edges to the
global profile inheritance graphs requires holding sets of locks at the same
time, and user requests which cause these graph manipulations have the potential
to produce concurrent sequences of acquisition which could result in an
ephemeral cycle and deadlock were it not for the back-off protocol implemented
by the ChainLocks.  This was the primary motivation for the initial introduction
of the ChainLock, however other reasons for using them have show up during the
process of breaking the GTL.

### Read-Only vs. Read-Write access.

A general rule for the use of ChainLocks in the kernel goes something like this.

1) In order to perform some operation `X`, a thread may need to hold some set of
   locks `S`.
2) `S` may not be initially completely known.  For example, if a thread `T` is to
   block in an owned wait queue `W`, we know that we must hold both `T` and
   `W`'s locks, but we do not yet know if `W` has an owner which also needs to
   be locked.
3) For operations involving a set of locks `S`, the general rule is that the
   state of an object `O` protected by `O.lock` may be examined if `O.lock` is
   held, but *no* mutations of any of the objects involved in `X` may take place
   until *all* of the locks in `S` have been obtained exclusively.

So, if we wanted to block a thread `T` in a wait queue `W`, we would:

1) Generate a token `A`.
2) Obtain `T.lock` using `A`.
3) Obtain `W.lock` using `A`.
4) Check to see if `W` has an owner, and if so, obtain `W.owner.lock` using `A`
5) Check to see if `W.owner` is blocked in a WaitQueue, and if so, obtain
   `W.owner.blocking_queue.lock` using `A`.
6) Repeat this process until we hold all of the locks in the priority
   inheritance chain.
7) Now that all of the locks are held, add the edge between `T` and `W`, and
   propagate the profile inheritance consequences through the PI chain to the
   target node.
8) Drop all of the locks.

If, at any point in time, attempting to obtain a lock results in a back-off
result, all of the current locks must be dropped and the sequence restarted from
step #2.  Note that we *do not* want to regenerate `A` when we back-off and
retry.  Keeping our original token `A` guarantees that eventually we become the
most important lock operation in the system, and be able to obtain all of our
locks without needing to back off.

## ChainLockTransactions

While ChainLocks can be a generalized tool, in the kernel, they are used to solve
a specific class of problems where operations on objects protected by a set of
locks need to be performed, but the order in which the locks are obtained is not
always guaranteed to be the same.  The ergonomics of attempting to manually
manage ChainLocks and Tokens can become burdensome in general, and also lead to
situations where it can be difficult to implement common static and dynamic
assertions related to lock ownership.

In order to make this easier to use and less error prone overall, the kernel
introduces a new helper object called the ChainLockTransaction (or CLT).

### What is a ChainLockTransaction, logically speaking?

Logically speaking, a ChainLockTransaction represents a sequence of operations
similar to the "block `T` in `W`" sequence previously described.  At the start
of the transaction, `A` is generated, and the transaction is in the
"non-finalized" state.  The transaction becomes the "active transaction", and
its generated token `A` is used to acquire the set of required locks.  Once all
of the locks are acquired, the transaction becomes "finalized" (more on this
later) and no new chain locks may be obtained.  Finally, any required mutations
take place, the locks are dropped, and the transaction is complete.

### What is a ChainLockTransaction, practically speaking?

In code, CLTs are RAII style objects which define the scope of an operation
involving a set of locks (henceforth, a transaction).  They are always
instantiated on the stack at the start of a transaction.  It is an error to ever
instantiate one in a global scope, a static local scope, or on the heap.  For a
given thread, there may only ever be one active CLT at a time, and a pointer to
this CLT is stored in the CPU's `percpu` data structure.

There are a number of rules in the kernel which need to be followed when using
ChainLocks, and CLTs are meant to provide a way to both easily follow those
rules, as well as to enforce that the rules are being followed, either
statically or dynamically (using DEBUG_ASSERTs).  These rules include:

1) The token, `A` used during a transaction must be the same token used for all
   of the locks involved in the transaction.  CLTs and the kernel's
   specialization guarantee this by having the CLT generate `A`, and not
   allowing any modification of this token after it was generated.  Additionally,
   the kernel version of a ChainLock does not provide a way to pass a token
   during an `Acquire` operation, instead demanding that the token be provided
   by a currently active CLT registered in the percpu data structure.
2) Like SpinLocks, no interrupt processing or context switching can take place
   while any ChainLocks are held.  CLTs help to enforce these requirements by
   providing specialized version which can automatically disable interrupts and
   preemption as appropriate based on the scope of the transaction.  The fact
   that no context switching or IRQ handling can take place during a CLT is
   also what makes it possible to store the pointer to the currently active
   transaction in the percpu data structure, making the transaction available
   outside of the immediate scope the transaction was instantiated in.
3) There may only ever be a single CLT active at once.  Overlapping transactions
   are illegal.  CLTs help enforce this rule two different ways, both statically
   and dynamically.  A global static analysis token (the
   `chainlock_transaction_token`) is introduced, and the instantiation of a CLT
   will `TA_ACQUIRE` this token. Note that this token is not actually a lock,
   and "acquiring" it is a no-op from an instruction execution perspective.  That
   said, it gives the static analyzer the ability to understand that we are in a
   currently active CLT, and any attempt to start another transaction will
   result in a compile time error, as it would look to the analyzer like we are
   attempting to acquire the same lock twice.  Additionally, methods can use
   this token to statically demand that we either are or are not involved in a
   CLT, as appropriate.  Second, we sometimes may need to dynamically enforce
   that we are or are not in an active CLT.  The percpu registration of the CLT
   allows us to check this at runtime, and additionally allows us to implement
   an `AssertHeld` style check on the `chainlock_transaction_token`, meaning
   that we can use the dynamic check to satisfy the static analyzer in
   situations where it would otherwise not be possible.
4) ChainLocks may only be obtained in the pre-finalized stage of a CLT.  After
   the CLT becomes finalized, no new locks may be obtained.  CLTs can be used to
   enforce this requirement by tracking internally whether or not the have
   entered the finalized state, and triggering a panic in the case that someone
   attempts to acquire a ChainLock after the transaction was finalized.  Note
   that this is additional CLT state which is not strictly needed, and can be
   disabled in production builds if desired.

### Other practical uses of ChainLockTransactions
#### Dynamic assertion that a ChainLock is currently held.

There are several places in the kernel where we need to be able to demonstrate
to the static analyzer that a particular lock is held, but where it is not
possible to do so from existing static annotations.  Kernel SpinLocks can
currently do this by invoking a method on the lock called `AssertHeld` which is
annotated with a `TA_ASSERT` static annotations.  The state of a kernel SpinLock
is an integer whose value is zero of the lock is not held, and the 1s-indexed
CPU id of the CPU which holds the lock in the case that it is held.  A
SpinLock's `AssertHeld` method simply needs to `DEBUG_ASSERT` that its state
equals `current_cpu_number + 1`.  The current CPU (when interrupts are off and
context switching is disabled) is a constant property of the thread of
execution, and can be fetched from any context to establish whether or not a
given SpinLock instance is currently held.

The state of a ChainLock, however, is the Token which was used to obtain it.
Generally speaking, this is not available to every scope unless the token was
explicitly plumbed into the scope, passed either by parameter or lambda capture.

The CLT, as used in the kernel, removes this restriction.  To hold any
ChainLocks at all requires that we have an active CLT, and that active CLT holds
our current token.  This allows us to implement our own AssertHeld from any
scope, even those where the token would not otherwise be immediately available.

Common places where this pattern shows up include:

1) Places where lambda functions are used, especially where the are injected into
   a method in functional programming pattern.  The "migrate" function for a
   thread is a good example of this.
2) Places involved in context switching, especially when the context switch is
   to a newly created thread's Trampoline function.  Lock handoff from the old
   thread to the new thread needs to take place at this point in time (which is
   a bit of a tricky process to begin with), but the Trampoline function has no
   static way to demonstrate that it holds the locks which need to be traded.
   CLTs provide the way to do this, even though no Tokens were explicitly
   plumbed.

#### Quantitative contention analysis.

Quantitative analysis of how much time was lost to contention on a SpinLock
instance is relatively straightforward.  If an attempt to acquire a SpinLock
fails the first time, the current time is observed and stored.  When the lock is
finally obtained, the amount of time from the first contention to the eventual
acquisition can be computed, and the amount of time spent spinning for the lock
instance can be recorded (typically via kernel tracing).

ChainLocks are a bit different.  Talking about the "time spent spinning" on a
ChainLock is difficult, because when a lock is contended, the result is
sometimes that the back-off protocol needs to be invoked.  All of the locks
involved in the transaction need to be dropped and reacquired.  During
re-acquisition, we may end up contending on a different lock involved in the
transaction, or the same lock, or no lock at all.  None of this time would be
properly accounted for if we simply attempted to track the time from first
conflict to subsequent acquisition of the same individual lock.

In general, it does not make a lot of sense to talk about the "time spent
contending" an individual ChainLock.  The CLT, however, gives us a useful analog
of this measurement.  Instead of talking about the contention time for an
individual lock, we can talk about the time spent obtaining all of the locks
involved in a transaction if _any_ contention event was experienced along the
way.

CLTs are be integrated with the kernel's version of ChainLocks to record a
contention start time the very first time any ChainLock contention event happens
during a non-finalized transaction.  When all of the locks are finally obtained,
the transaction becomes finalized, and the time between when the first
contention event happened and when the transaction was finally finalized can be
recorded as the overall "spin contention time" for the transaction.

This functionality can be enabled/disabled at compile time, dropping completely
out of the build when not needed.

#### Implementing proper `Relax` behavior.

As noted earlier, when a CLT encounters a situation where a back-off is
required, it must drop all locks and wait a small amount of time before trying
again.  At a minimum, we want to `arch::Yield` at this point in time, but
sometimes we should go further.

For example, if we had to disable both preemption and interrupts at the start of
our transaction, we probably do not want to keep them disabled during our back
off period as this risks holding off the servicing of time critical interrupts
and threads during acquisition.

The specialized versions of the CLT (instantiated on the stack by users)
provides us a better option.  CLT instances provide a `Relax()` method which can
be used to implement the proper behavior automatically.  In the simplest case,
where interrupts and preemption where already appropriately disabled, all the
CLT needs to do is `arch::Yield`.  In cases where the CLT instance is managing
the IRQ/Preempt state, it can:

1) Unregister the current transaction as the active transaction.
2) Restore the preempt/IRQ state.  If we are preempted or take an interrupt at
   this point in time, our per-cpu state properly reflects the fact that we are
   not in a currently active transaction and hold no ChainLocks.
3) `arch::Yield()`
4) Undo step #2, disabling preemption/IRQs as appropriate.
5) Re-register the CLT as the active transaction.

Users do not need to explicitly implement the proper behavior, the type of the
instantiated transaction handles this for them.

#### Enforcing lock count consistency checks.

There are places in code where we would like to DEBUG_ASSERT things about the
number of ChainLocks currently held.  For example, at the start of a
`RescheduleCommon` operation, we expect exactly one ChainLock to be held, that
of the currently running thread.  The dynamic assertion capability of CLTs
allows us to assert that we hold the current thread's lock, but we'd like to go
further and assert that this is the _only_ chain lock held.

Other examples of where we want to make a check like this is during a `Relax`
operation, and at the end of a transaction (when the CLT instance destructs).
At both of these points in time, we want to be able to assert that we hold
no ChainLocks at all.

CLTs give us a tool to perform this style of consistency check.  They are hooked
into the kernel specific implementation of ChainLocks in a way which allows them
to monitor Acquire/Release operations, and can maintain a count of the number of
locks currently held.  This takes extra state in the CLT (the count of locks
held), but like all of the debug checks made possible by CLTs, they can be
disabled at compile time and completely drop out of the build when not needed.

### Types of ChainLockTransactions.

As noted earlier, multiple different versions of ChainLockTransaction are
provided to make it easier for users to satisfy preemption and IRQ
enabled/disabled requirements.  All of these specific types of CLT are derived
from a common `ChainLockTransaction` class.  This base class defines the common
CLT interface, and a `ChainLockTransaction*` is the type of pointer which is
stored in the percpu data structure when a transaction is active.  Users,
however, may never instantiate a base ChainLockTransaction object.  Instead, the
following specific types of transactions are available for use instead.

1) `ChainLockTransactionNoIrqSave`.  This transaction does not manipulate either
   preempt or IRQ enabled/disabled state.  Instead, it simply asserts that IRQs
   are disabled when it is generated, and implements a simple `arch::Yield`
   if/when the transaction is `Relax`ed.
2) `ChainLockTransactionIrqSave`.  Automatically saves and restores the IRQ
   disabled state.  Also automatically restores IRQ state and un/re-registers
   the active transaction during a `Relax` operation.
3) `ChainLockTransactionPreemptDisableIrqAndSave`.  The same as
   `ChainLockTransactionIrqSave`, but goes one step further, automatically
   disabling preemption, and then IRQs.
4) `ChainLockTransactionEagerReschedDisableIrqAndSave`.  The same as
   `ChainLockTransactionPreemptDisableIrqAndSave`, but goes even further,
   disabling eager rescheduling in addition to preemption.

### A usage example.

Let's take a look at a simple (but heavily commented) example of using
ChainLockTransactions and ChainLocks.  For our simplified scenario, let's
pretend that we want to block a `Thread` in an `OwnedWaitQueue` but we do not
have to deal with ownership reassignment.

```c++
extern ChainLock::LockResult AcquireDownstream(OwnedWaitQueue& owq)
  TA_REQ(chainlock_transaction_token, owq.lock_);

extern void LinkAndPropagate(Thread& t, OwnedWaitQueue& owq)
  TA_REQ(chainlock_transaction_token, t.lock_, owq.lock_);

extern void ReleaseDownstream(OwnedWaitQueue& owq)
  TA_REQ(chainlock_transaction_token) TA_REL(owq.lock_);

extern ChainLock::LockResult BlockInScheduler(Thread& t)
  TA_REQ(chainlock_transaction_token, t.lock_);

void BlockInQueue(OwnedWaitQueue& owq) {
  using LockResult = ChainLock::LockResult;
  Thread* const current_thread = Thread::Current::Get();

  // Create our transaction.  This will "acquire" the chainlock_transaction
  // token, which is a static requirement for acquiring any ChainLocks.  It will
  // also handle disabling preemption and interrupts for us.  Additionally, it
  // will ensure that a consistent token is used throughout the transaction.
  ChainLockTransactionPreemptDisableAndIrqSave clt("BlockInQueue");

  // Enter into our retry loop.  We will cycle through this loop any time we
  // need to back off, automatically relaxing the transaction as we do.  The
  // relax operation will dynamically assert that we have properly released all
  // of our locks before we relax.
  for (;; clt.Relax()) {
    // "Unconditionally" acquire the current thread's lock.  An "unconditional"
    // acquisition of a ChainLock means that the acquire operation will continue
    // to attempt to obtain the lock, even if it is held by a higher priority
    // transaction.  This is legal _only_ because we know that at this point in
    // time that we hold no other locks, therefore we cannot be "in the way" of
    // any other concurrently executing CLTs.  IOW - we have no locks to drop,
    // and can simply keep trying.
    //
    // The advantage of unconditional acquisition here that we can acquire the
    // lock using a traditional RAII style guard which will handle both
    // releasing the guard for us, as well as establishing that we hold the lock
    // statically.
    UnconditionalChainLockGuard guard{current_thread->lock_};

    if (const LockResult res = owq.lock_.Acquire(); res != LockResult::kOk) {
      // Cycles should be impossible here.  The only possible non-ok result
      // should be "Backoff".
      DEBUG_ASSERT(res == LockResult::kBackoff);

      // All we need to do here is continue.  We only hold the current thread's
      // lock, and our guard will automatically drop it before our for loop
      // automatically relaxes the transaction for us.
      continue;
    }

    // Now that we have successfully obtained the lock, assert that we just
    // acquired it. AssertAcquired is similar to AssertHeld, but goes a step
    // further.  Not only are we asserting that we hold it for static analysis
    // purposes, we are asserting that we just _acquired_ the lock as well,
    // meaning that we _must_ release the lock in our scope as well.
    owq.lock_.AssertAcquired();

    // If this lock has an owner, grab the downstream locks as well.  If this
    // fails, drop our locks and start over.
    if (const LockResult res = AcquireDownstream(owq); res != LockResult::kOk) {
      // Same as before, cycles should be impossible here.
      DEBUG_ASSERT(res == LockResult::kBackoff);

      // Drop the queue lock, and continue which will drop the thread lock and
      // relax the transaction for us.  We cannot forget to drop the queue lock
      // because of the AssertAcquired we executed before.
      owq.lock_.Release();
      continue;
    }

    // We have all of the locks we need!  Finalize our transaction (which will
    // automatically handle logging any contention statistics if enabled).
    clt.Finalize();

    // Link the thread and the queue, and propagate the PI consequences.  The
    // requirement that we hold both the thread's lock and the queue's lock is
    // statically enforced, while the fact that the downstream lock chain needs
    // to be held can be dynamically enforced thanks to our active CLT.
    LinkAndPropagate(*current_thread, owq);

    // Now that we have the bookkeeping handled, we can release all of the locks
    // at and downstream of the queue we are blocking in.
    ReleaseDownstream(owq);

    // Finally, block in the scheduler. This requires that our current thread's
    // lock be held, but no other ChainLocks.  We statically require that the
    // current thread's lock be held, and the scheduler's code will dynamically
    // assert that this is the only lock held.  Our thread's lock will
    // automatically be release and reacquired before and after the block
    // operation.
    BlockInScheduler(*current_thread);

    // Finally, break out of our loop.  Our guard will automatically release our
    // (now re-acquired) thread's lock.
    break;
  }

  // Exit the scope, destroying our CLT in the process.  During destruction, we
  // will automatically:
  //
  // 1) Check to make sure we hold no longer hold ChainLocks.
  // 2) Check to make sure that the transaction was properly finalized.
  // 3) Unregister the transaction as the active transaction.
  // 4) Restore the previous preempt/IRQ enabled/disabled state.
}
```
