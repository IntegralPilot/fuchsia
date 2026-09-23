# libconcurrent

## Overview

libconcurrent is a small library intended to be used both by kernel mode and
user mode code to assist in implementing memory sharing patterns which involve
readers observing memory concurrently with writers updating the same memory.
Unlike techniques which use exclusion (such as mutexes, reader-writer locks,
spinlocks, and so on) to ensure that reads to memory locations are never taking
place at the same time as writes, concurrent read-write patterns require some
extra care to ensure that programs are formally "data-race free" according to
the C++ specification.

## Data races in C++

Section 6.9.2.1 of the [C++20 Draft Specification](http://www.open-std.org/JTC1/SC22/WG21/docs/papers/2020/n4849.pdf)
formally defines what is considered to be a "data race" by the standard.
Paragraph 2 defines a "conflict" by saying:

```
Two expression evaluations conflict if one of them modifies a memory location
(6.7.1) and the other one reads or modifies the same memory location.
```

Paragraph 20 then defines a "data race" by saying:

```
Two actions are potentially concurrent if

(21.1) — they are performed by different threads, or
(21.2) — they are unsequenced, at least one is performed by a signal handler,
         and they are not both performed by the same signal handler invocation.

The execution of a program contains a data race if it contains two potentially
concurrent conflicting actions, at least one of which is not atomic, and neither
happens before the other, except for the special case for signal handlers
described below. Any such data race results in undefined behavior.
```

So, when sharing data between concurrently executing threads where mutual exclusion
cannot be used to ensure that data races cannot occur, extra care must be taken
when reading and modifying the shared data.  The data transfer utilities offered
by libconcurrent ensure that all load and stores executed on shared memory
regions are done using atomics, which should be sufficient to avoid introducing
any formal data races (and therefore undefined behavior) to a program.

## Transferring data

The lowest level building blocks offered by libconcurrent give the user the
ability to concurrently transfer data into and out of a share memory location
without accidentally introducing any undefined behavior as a result of
unintentional data races.  These building blocks are:

+ `concurrent::WellDefinedCopyTo<SyncOpt, Alignment>(void* dst, const void* src, size_t len)`
+ `concurrent::WellDefinedCopyFrom<SyncOpt, Alignment>(void* dst, const void* src, size_t len)`
+ `concurrent::WellDefinedCopyable<T>::Update(const T&, SyncOptType<SyncOpt>)`
+ `concurrent::WellDefinedCopyable<T>::Read(T&, SyncOptType<SyncOpt>)`

`CopyTo` operations move data from a thread's private buffer into a shared
buffer which may be accessed concurrently by readers. `CopyFrom` operations move
data from a shared buffer which may be concurrently written to into a thread's
private buffer.

Copy(To|From) both have memcpy semantics, not memmove semantics. In other
words, it is illegal for |src| or |dst| to overlap in any way.

The individual `Copy(To|From)` functions represent the absolute
lowest level building blocks of libconcurrent, and need to be used with care.
When possible, prefer using one of the higher level building blocks which help
to automate common patterns.  They have `memcpy` compatible signatures, but
include a few additional requirements.  Specifically:

1) The `src` and `dst` buffers *must* have the same alignment relative to the
   maximum atomic operation transfer granularity (currently 8 bytes).  In other
   words, `ASSERT((src & 0x7) == (dst & 0x7))`.
2) If these functions are being used to copy instances of structures or classes,
   it is required that those structs/classes be trivially copyable.
3) When accessing shared memory, it is important that all read and write
   accesses operate using the same alignment and op-width at all times.  For
   example:  Given a shared buffer located at an 8 byte aligned address,
   `shared`, the following operations may all be safely conducted concurrently:
   + `CopyTo(shared, local_1, 16);`
   + `CopyFrom(local_2, shared, 16);`
   + `CopyFrom(local_3, shared + 8, 8);`
   Since `shared` is 8 byte aligned, all of the accesses to shared will also all
   be 8 byte aligned, and of 8 bytes in length, guaranteeing that all memory
   accesses to the same address in the `shared` region will use the same width
   memory transaction.  Adding a `CopyFrom(local_4, shared + 4, 8);` to the list
   of concurrent operations, however, would _not_ be safe as it would result in
   two 4 byte read transactions being conducted against the shared buffer,
   overlapping regions where 8 byte transactions are also taking place.

### Synchronization Options

By default, `Copy(To|From)` operations to the same regions of memory attempt to
synchronize-with each other by adding `memory_order_release` semantics to each
of the atomic store operations executed during a call to `CopyTo`, and
`memory_order_acquire` semantics to each of the atomic load operations executed
during a call to `CopyFrom`.

Depending on the use case, however, it is possible that synchronizing like this
might not be as efficient as using a thread fence instead.  Users may control
the synchronization behavior of the operation using the first template parameter
of the call to `Copy(To|From)`, which must be a member of the `SyncOpt`
enumeration.  The options are as follows:

1) `SyncOpt::AcqRelOps`.  This is the default option, and causes each atomic
   load/store, to use `memory_order_acquire`/`memory_order_release`, as
   appropriate, during the transfer.
2) `SyncOpt::Fence`.  Instead of synchronizing each of the load/store operations
   individually, all atomic load/stores are executed with
   `memory_order_relaxed`, and thread fences are used instead.  `CopyTo`
   operations will be proceeded by a `memory_order_release` thread fence, while
   `CopyFrom` operations will be followed with a `memory_order_acquire` thread
   fence.
3) `SyncOpt::None`.  No synchronization will be added to the transfer operation.
   No explicit thread fences will be generated, and all atomic load/store
   operations will use `memory_order_relaxed`.

Extreme care should be taken when using `SyncOpt::None`.  It is almost always
the case that at least _some_ synchronization will be needed when publishing and
consuming data concurrently.  The `SyncOpt::None` option is offered for users
who may need to move data to/from multiple disjoint regions of shared memory,
and wish to use fences to achieve synchronization.  In this case, the first/last
(CopyTo/CopyFrom) operation in the sequence should use a fence, while other
operations would choose `SyncOpt::None` avoiding the need for superfluous
load/store or thread fence synchronization.  For example:

```c++
void PublishData(const Foo& foo1, const Foo& foo2, const Bar& bar) {
  WellDefinedCopyTo<SyncOpt::Fence>(&shared_foo1, &foo1, sizeof(foo1));
  WellDefinedCopyTo<SyncOpt::None>(&shared_foo2, &foo2, sizeof(foo2));
  WellDefinedCopyTo<SyncOpt::None>(&shared_bar1, &bar1, sizeof(bar1));
}

void ObserveData(Foo& foo1, Foo& foo2, Bar& bar) {
  WellDefinedCopyFrom<SyncOpt::None>(&foo1, &shared_foo1, sizeof(foo1));
  WellDefinedCopyFrom<SyncOpt::None>(&foo2, &shared_foo2, sizeof(foo2));
  WellDefinedCopyFrom<SyncOpt::Fence>(&bar1, &shared_bar1, sizeof(bar1));
}
```

### Alignment Optimizations

If alignment of a transfer can be compile-time guaranteed to be greater than or
equal to the maximum atomic transfer granularity of 8 bytes, a minor
optimization can be achieved during the transfer by skipping the transfer phase
which brings the operation into 8 byte alignment.  Users can access this
optimization by specifying the guaranteed alignment of their operation as the
second template parameter of the `Copy(To|From)` operation.  For example:

```c++
template <typename T>
void PublishData(const T& src, T& dst) {
  WellDefinedCopyTo<SyncOpt::AcqRelOps, alignof(T)>(&dst, &src, sizeof(T));
}

template <typename T>
void ObserveData(const T& src, T& dst) {
  WellDefinedCopyFrom<SyncOpt::AcqRelOps, alignof(T)>(&dst, &src, sizeof(T));
}
```

### `WellDefinedCopyable<T>`

In order to make life a bit easier for users who need copy data in a well
defined way into and out of structures, a helper class named
`WellDefinedCopyable<T>` is offered.

Users may wrap any trivially copyable type, `T` in one of these wrapper
instances, and the use the provided Update and Read methods to copy data
into and out of the contained `T` instance, respectively.  These methods, by
design, deliberately restrict the ways that the user can gain access to the
underlying storage, forcing them make use of the lowest level well-defined
transfer functions.

Constructor parameters are directly forwarded to the underlying `T` instance.

```c++
WellDefinedCopyable<Foo> default_constructed;
WellDefinedCopyable<Foo> explicit_construction{45};
WellDefinedCopyable<Foo> moar_args{"Foo", 45, "Bar", 34.4};
```

Just remember that `T` (and therefore all of its data members) must be trivially
copyable.

#### Explicit synchronization

The wrapper's `Update` and `Read` methods allow the user to specify the
synchronization option to use, with a default of `SyncOpt::AcqRelOps`, just like
the `WellDefinedCopy(To|From)` functions do.  Because of the somewhat awkward
dependent name rules of C++, the type of explicit synchronization desired can be
specified using a type-tagging pattern, instead of needing to specify the sync
option as an explicit template parameter.

```c++
WellDefinedCopyable<Foo> shared_foo;
Foo my_foo;

shared_foo.Read<SyncOpt::Fence>(my_foo);          // this does not work.
shared_foo.template Read<SyncOpt::Fence>(my_foo); // this is one way to make it work.
shared_foo.Read(my_foo, SyncOpt_Fence);           // this reads a bit better.
```

Aliases for the synchronization type tags are as follows.
| enum class         | type tag instance |
|--------------------|-------------------|
| SyncOpt::AcqRelOps | SyncOpt_AcqRelOps |
| SyncOpt::Fence     | SyncOpt_Fence     |
| SyncOpt::None      | SyncOpt_None      |


#### Raw storage access

User don't always _have_ to access their `T` instance's storage only by copying
data into or out of it.  Direct read-only access may be obtained using the
`WellDefinedCopyable<T>`'s `unsynchronized_get` method, however users should
exercise caution when choosing to do this.

Accessing the buffer using `unsynchronized_get` is _only_ safe if the user can
guarantee that no write operations may be concurrently performed against the
storage while the user is reading the instance.

One example of a legitimate use of this method might be when a user is operating
in the write exclusive portion of a sequence lock.  They are guaranteed to be
the only potential writer of the wrapped object, so while it is still important
that they continue to use `Update` when they wish to mutate their instance of
`T`, it is OK for them to read `T` directly without using `Read` as this
will not cause any undefined behavior when done concurrently with other readers
in the system.

## SeqLock

### Overview

Sequence locks are synchronization primitives which allow for concurrent read
access to a set of data without ever excluding write updates.  Reads are
performed as transactions, which succeed if and only if there is no concurrent
write operation which overlaps with the read transaction.  Sequence locks can be
useful in patterns where any of the following conditions hold:

+ Read operations are expected to greatly out-number write operations, and high
  levels of read concurrency are desired.
+ Write operations must never be delayed by concurrent read operations.
+ Readers have strictly read-only access to the shared state of the published
  data.  No modifications of the state are allowed, as would be required when
  using a shared synchronization building-block such as a reader/writer lock
  obtained for shared access.

To assist in implementing data sharing via a sequence lock, `libconcurrent`
offers the `SeqLock` primitive.  The `SeqLock` behaves like a spinlock when
acquired exclusively for write access, while still allowing concurrent read
transactions to take place.

### Rules

In order to properly implement a sequence lock pattern, there are a few rules
which must be obeyed at all times.

1) Data protected by a `SeqLock` may be both read and written concurrently,
therefore care must be taken to always access the data in a way which is free
from data races.
2) Reads of, and writes to the data protected by the `SeqLock` must always
properly synchronize-with the internals of the lock in order to ensure proper
behavior on architectures with weak memory ordering.
3) No decisions based on protected data should ever be made _during_ a read
transaction.  It is only after a read transaction has _successfully_ concluded
that a program is guaranteed to have made a coherent observation of the
protected data.

Given a sequence of write transactions, a read transaction has made a "coherent"
observation if all of the values of the members of the protected data is
observes came from a single write transaction.  If it ever might have observed
values written by two different write transactions, the read transaction *must*
fail, and the user may choose to either retry the operation, or give up.

### Patterns for properly using SeqLocks in the kernel.

Here are a small set of patterns which demonstrate how to properly observe and
make updates to a set of data protected by a SeqLock.  It is strongly advised
that you follow these patterns exactly as presented.  _Do not deviate from the
patterns presented here unless you are extremely knowledgable with both the
internals of the SeqLock implementation, its synchronization requirements, and
the formal C++ memory model_.  Even then, you probably don't want to deviate
from these patterns without an extremely compelling reason.

Each of these examples will be shown with the assumption that the code is being
used in the context of the Zircon kernel.  While it is possible to use SeqLock's
in user-mode, they only make sense when being used to read data from a piece of
memory shared with the kernel, where all updates to the protected data are done
from the kernel.

This is because user mode has no guaranteed way to prevent a situation where
their thread might become preempted in the middle of a write transaction.  Any
readers attempting to read data concurrently with this write transaction are
going to get stuck, spinning at the start of the transaction until either the
writer is re-scheduled and finishes, or they themselves exhaust their timeslice
and are preempted.

#### The simplest example.

Let's start with a simple setup.  We will define a class which contains an
instance of a structure (`Foo`) which is guarded by an instance of a SeqLock.
We will declare a method `Update` which takes a constant reference to a `Foo`
instance and updates the local contents of `foo_` with it.  Likewise, we will
declare an Observe method which will perform a successful observation of the
internal `foo_`, and return the observed copy of the data to the caller.

```c++
#include <lib/kconcurrent/seqlock.h>

struct Foo {
  uint32_t a, b;
};

class MyClass {
 public:
  void Update(const Foo& foo);
  Foo Observe();

 private:
  DECLARE_SEQLOCK(MyClass) seq_lock_;
  SeqLockPayload<Foo, decltype(seq_lock_)> foo_ TA_GUARDED(seq_lock_);
};
```

Note that our class:

1) Declares the `SeqLock` instance `seq_lock_`.
2) Declares the instance of `Foo` as being wrapped in a `SeqLockPayload<...>` template.
3) Declares `foo_` instance as being `TA_GUARDED` by the instance of the lock.

*Always follow these steps.*

Step #1 declares the instance of the lock using a macro which ensures that the
instance is properly instrumented by `lockdep` (when `lockdep` is enabled).
This allows us to use `lockdep` defined RAII style `Guard`s, in addition to
getting the runtime cycle analysis provided by lockdep.  Finally, this also
automatically chooses synchronization methodology that we will use to ensure
that we get proper, coherent observations of the payloads protected by the lock.
By default, this is atomic Acquire/Release operations used on individual payload
loads/stores.

Step #2 (wrapping the `Foo` instance in a `SeqLockPayload<...>`) does two things
for us.

First, it will prevent anyone from accidentally reaching in and reading or
writing the contents of the protected data directly, running the risk of failing
to access the data in a atomic fashion (which could lead to a data-race).

Second, it binds the synchronization method chosen during the declaration of the
lock, to the synchronization method implemented by the payload wrapper. This
prevents a user from accidentally accessing the data using a sync method which
does not match that implemented by the lock instance, leading to the possibility
of an incoherent observation being made by a reader.

Finally, Step #3.  Flagging the protected data as being `TA_GUARDED` by the lock
guarantees that clang static thread analysis (when enabled) will catch at
compile time any attempts to access the protected data while not inside a
`SeqLock` transaction, and well prevent all attempts to mutate the protected
data during a read transaction instead of a write transaction.

Now let's take a look at the implementation of `Update` and `Observe`

```c++
void MyClass::Update(const Foo& foo) {
  Guard<SeqLock<>, ExclusiveIrqSave> lock{&seq_lock_};
  foo_.Update(foo);
}

Foo MyClass::Observe() {
  Foo ret;

  bool success;
  do {
    Guard<SeqLock<>, SharedNoIrqSave> lock{&seq_lock_, success};
    foo_.Read(ret);
  } while (!success);

  return ret;
}
```

`Update` is extremely simple.  We simply:

1) Declare a `lockdep::Guard` with the appropriate type and access mode
   (`SeqLock<>` and `ExclusiveIrqSave`)
2) Copy our payload into the structure using the `WellDefinedCopyWrapper`'s
   `Update` methods.

The `lockdep::Guard` guarantees that we will properly disable and re-enable
interrupts, as well as acquire and release the lock exclusively, before and
after the update of the payload.  Disabling and re-enabling interrupts
guarantees that we will never be preempted during the update operation itself,
which is important to preventing readers from spinning pointlessly.  Acquiring
the lock exclusively is required in order to be able to `Update` the paylod.

`Observe` is almost as simple, but has a few extra details involved.  Read
transactions are not guaranteed to succeed, so we need a loop to try again in he
case of failure.  In this example, we don't define any timeout conditions, we
simply try repeatedly until we eventually succeed.

The `lockdep::Guard` guard we use:

1) Uses the access type `SharedNoIrqSave`.  `Shared` is the access level we use
   when reading the protected data, and we do not disable IRQs during this
   operation.  We don't need to, and we don't really _want_ to, as we don't want
   to spin for any length of time with interrupts off attempting to start the
   transaction.
2) Is declared in the scope of the `while` body, while the `success` bool is
   declared outside of the `while` body scope.  This is important because the
   end of the transaction and the determination of success or failure happens
   when the guard destructs, and the results are recorded in `success`.  The
   guard must destruct, and therefore is declared in the while body, but the
   `success` needs to be evaluated as part of the `while` predicate, so it must
   exist in a scope outside of the `while` body.

Finally, we copy our protected data into a local stack-allocated Foo instance
while inside the guard, and return that copy to the caller after we have ended
the transaction successfully.

#### Reads of data while writing

Writes protected by a `SeqLock` are exclusive, meaning no two writes can ever
take place at the same time.  They must happen sequentially with a well defined
order.  Therefore we can actually read our protected data while the lock is held
exclusively without needing to consider either memory order or data race issues.
What we can *never* do, however, is update the contents without using the
`SeqLockPayload<...>`'s `Update` method.  Let's take a quick look at two
simple examples of this:

```c++
class MyClass {
  // ...

  void Sort();
  void Inc(uitn32_t amt);
};

void MyClass::Sort() {
  Guard<SeqLock<>, ExclusiveIrqSave> lock{&seq_lock_};

  const Foo& current_foo = foo_.unsynchronized_get();
  if (current_foo.a > current_foo.b) {
    Foo new_foo{current_foo.b, current_foo.a};
    foo.Update(new_foo);
  }
}

void MyClass::Inc(uitn32_t amt) {
  Guard<SeqLock, ExclusiveIrqSave> lock{&seq_lock_};

  Foo new_foo = foo_.unsynchronized_get();
  new_foo.a += amt;
  new_foo.b += amt;
  foo_.Update(new_foo);
}
```

We declare two new methods for our class.  `Sort` (as the name implies) sorts
the elements of the protected `Foo` instance in ascending order.  It grabs a
`const Foo&` from the payload wrapper's `unsynchronized_get` method, then based on
the relationship between the protected data's `a` and `b`.  It either constructs
a new Foo instance on the stack with the values of the fields swapped, then
updates `foo_` using the `Update` method, or it simply exits because the
elements are already in the desired order.

Note that we are actually directly accessing the contends of `foo_` via the
`const Foo&` we obtained from our call to `unsynchronized_get()`.  This is OK
when we have the `SeqLock` locked exclusively because we are the only possible
writer in the lock.  No other writers can issue any stores, so it is impossible
to create a data-race, even if our loads are non-atomic.

The second example, `Inc` is going to increment both of the fields of the `foo_`
instance by `amt`.  In this case, we also read the current state of `foo_` via a
call to `unsynchronized_get()`, but this time we do it to copy-construct a local
copy `new_foo`, then increment both members, and finally update the protected
data using a call to `WellDefinedCopyWrapper<>::Update()`.

*Never* attempt to use `unsynchronised_get()` during a read transaction.  The
data being observed in the middle of a read transaction:

1) Could be getting concurrently written to by a writer, creating a data-race,
   and
2) Even if no writes were happening, there absolutely no guarantee that the
   loads performed against the data would provide a coherent view of the data.

If you have clang static thread analysis enabled (you should), and you have
annotated your protected data as being `TA_GUARDED` by your lock, the static
analysis should catch this mistake at compile time.  If you are using some other
tool-chain, or you have not enabled static analysis, this mistake will get
missed, and you will be Very Sad.

#### Partial updates and partial observations.

Users of `SeqLock`s can also protect multiple sets of data with one `SeqLock`.
The easiest way is to simply introduce more structures wrapped in
`SeqLockPayload<...>`s.

```c++
struct Foo { uint32_t a, b; };
struct Bar { uint32_t c, d; };

class MyClass {
 public:
  void Update(const Foo& foo);
  void Update(const Bar& bar);
  void Update(const Foo& foo, const Bar& bar);

  Foo ObserveFoo();
  Bar ObserveBar();
  ktl::pair<Foo, Bar> Observe();

 private:
  DECLARE_SEQLOCK(MyClass) seq_lock_;
  SeqLockPayload<Foo, decltype(seq_lock_)> foo_ TA_GUARDED(seq_lock_);
  SeqLockPayload<Bar> decltype(seq_lock_)> bar_ TA_GUARDED(seq_lock_);
};
```

We may now observe or update one, or the other, or both of these containers of
protected data, provided we follow the previously established rules for
obtaining the `SeqLock` as we do.

#### Fence-to-Fence Synchronization

As noted earlier, the default synchronization mechanism used by `SeqLock` is to
use atomic loads/stores with Acquire/Release semantics whenever accessing the
protected payload contents.  This is not the _only_ option, however.  Depending
on the specific underlying machine architecture and the size of the payload
which needs to be observed/published, it might be more efficient to use Relaxed
loads/stores, and use fence-to-fence synchronization instead.

Switching approaches should be pretty easy overall.  All we need to do is to
update how we declare our lock, as well as the lock type specified when we use a
`Guard`.  So:

```c++
class MyClass {
 public:
  void Update(const Foo& foo);

 private:
  DECLARE_SEQLOCK(MyClass) seq_lock_;
  SeqLockPayload<Foo, decltype(seq_lock_)> foo_ TA_GUARDED(seq_lock_);
};
```
becomes
```c++
class MyClass {
 public:
  void Update(const Foo& foo);

 private:
  DECLARE_SEQLOCK_FENCE_SYNC(MyClass) seq_lock_;
  SeqLockPayload<Foo, decltype(seq_lock_)> foo_ TA_GUARDED(seq_lock_);
};
```
and
```c++
void MyClass::Update(const Foo& foo) {
  Guard<SeqLock<>, ExclusiveIrqSave> lock{&seq_lock_};
  foo_.Update(foo);
}
```
becomes
```c++
// Either this...
void MyClass::Update(const Foo& foo) {
  Guard<SeqLockFenceSync, ExclusiveIrqSave> lock{&seq_lock_};
  foo_.Update(foo);
}

// .. or this
void MyClass::Update(const Foo& foo) {
  Guard<SeqLock<::concurrent::SyncOpt::Fence>, ExclusiveIrqSave> lock{&seq_lock_};
  foo_.Update(foo);
}
```

#### Blocking/Spinning behavior

Both the `BeginReadTransaction` and the `Acquire` operations have the potential
to spin-wait if there happens to be another thread which has currently
`Acquire`ed the lock for exclusive access. Technically, the operations never
result in the thread blocking in the scheduler, however they will spin waiting
for the lock to become uncontested before proceeding.

If users are executing in a time sensitive context, or a read operation is being
conducted against data which is being updated by another (potentially malicious)
process, `Try` versions of the `BeginReadTransaction` and the `Acquire` may be
used along with a timeout to limit the amount of spinning which may eventually
take place.

+ `bool TryBeginReadTransaction(ReadTransactionToken& out_token, zx_duration_mono_t timeout)`
+ `bool TryBeginReadTransactionDeadline(ReadTransactionToken& out_token, zx_instant_mono_t deadline)`
+ `bool TryAcquire(zx_duration_mono_t timeout)`
+ `bool TryAcquireDeadline(zx_instant_mono_t deadline)`

Note that there is currently no lockdep::Guard adapter defined which works with
timeouts/deadlines, so we will need to manually manage locking/unlocking and
manipulating read tokens ourselves.  Additionally, because of the fact that
lockdep instrumented SeqLocks are actually wrapped in an outer `LockDep<...>`
wrapper, it can be a bit awkward to remember how to declare the proper type for
the `ReadTransactionToken` we need to use with the `Try` methods.  The kernel
environment provides a helper called `SeqLockReadTransactionToken` which can be
used to automatically deduce the proper type for the token based on the lock,
and this is the technique used in the example below.

For example:

```c++
zx_status_t MyClass::UpdateWithTimeout(const Foo& foo, zx_duration_mono_t timeout) {
  if (!seq_lock_.lock().TryAcquire(timeout)) {
    return ZX_ERR_TIMED_OUT;
  }
  foo_.Update(foo);
  seq_lock_.lock().Release();
  return ZX_OK;
}

zx::result<Foo> MyClass::ObserveWithTimeout(zx_duration_mono_t timeout) {
  // Instead of needing to say this to decalre our token:
  // decltype(seq_lock_)::LockType::ReadTransactionToken token;
  //
  // We can say this instead:
  SeqLockReadTransactionToken token{seq_lock_};
  zx_instant_mono_t deadline = zx_deadline_after(timeout);
  Foo ret;

  do {
    if (!seq_lock_.lock().TryBeginReadTransactionDeadline(token, deadline)) {
      return zx::error(ZX_ERR_TIMED_OUT);
    }
    foo_.Read(ret);
  } while (!seq_lock_.lock().EndReadTransaction(token));

  return zx::ok(ret);
}
```
