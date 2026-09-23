# Memory order models

## Overview

This directory contains a collection of documentation and models of various
algorithms used to implement members of the `libconcurrent` library.  The
intention is to explain why the choice of various memory orders at the various
steps of the algorithm results in proper behavior across all architectures,
regardless of the details architecture's underlying memory model.

If you are not familiar at all with the C++ memory model, and are still
interested in this topic, it would probably be a good idea to familiarize
yourself some of the core concepts.  Having at least some idea of what it means
for an architecture (think, x64, ARM64, or RISC-V) to have a "strong" or a
"weak" memory model is very helpful.  Additionally, it would also be helpful to
take some time to understand how the C++ language attempts to present a memory
model which is compatible with all of these various architectural memory models,
and what the rules of the common C++ memory model are.

One good reference for this can be found
[here](https://en.cppreference.com/w/cpp/atomic/memory_order).  While it is a
bit formal, and can be difficult to follow at times, it does a pretty good job
of introducing the concepts.  There are also other blogs and lectures out there
which can help to introduce the concepts in a more explanatory fashion.

The models provided here are simplified implementations of the core algorithms
meant to be run against
[`CDSChecker`](http://plrg.ics.uci.edu/software_page/42-2), an open source model
checker which helps to test that an algorithm obeys its invariants regardless of
whatever execution order a hypothetical architecture could see within the
degrees of freedom offered by the formal C++ memory model.

Both "good" and "bad" examples are given.  The "good" examples show the
algorithms as implemented, or as they could be implemented if there were
slightly different requirements.  The "bad" attempt to show versions of the same
algorithm which _seem_ like they might work, but actually don't.  The "bad"
examples come with an example of a failure as discovered by `CDSChecker` along
with an explanation of how the example demonstrates that the implementation of
the algorithm fails to work.

Interested users are encouraged to checkout the latest version of the
`CDSChecker` project and run the examples through them in order to gain insights
into how the algorithms are constructed and why they work.

## SeqLock

### Definition and Requirements

A `SeqLock` or "Sequence Lock" is a construct which allows some amount of
protected data to be observed coherently by multiple readers executing
concurrently along with with a serialized sequence of write transactions in a
way which never results in writers blocking in order to wait for readers to
complete.  "Coherently" in this context means:

1) Given a sequence of write transaction, (`W1`, `W2`, `W3` ...)
2) And a observation of the protected data made by a read transaction `R`
3) The observation of the protected data made by `R` is coherent if-and-only if
the value of every word of the protected data observed by `R` came from the
same, single, write transaction `Wx`.

Read transactions are not guaranteed to succeed, and may need to be executed
multiple times in order to obtain a successful, coherent, observation.

Write transactions *must* happen exclusively, and with a well defined order from
the view point of any read-transaction.  This can be because the write
transactions all happen in the same thread, meaning that ordering is determined
by the sequence-order of the thread, or because a separate mechanism is
implemented to provide both exclusion (eg, no two write transactions can
actually execute concurrently), and a defined order from the perspective of a
properly synchronized external observer.

### Terminology

+ We assume that there exists an arbitrary sequence of write transactions, denoted
  as `(W(1), W(2), W(3), ...)`.  For a given pair of write transactions, `W(x)`
  and `W(y)`, we say that `W(x)` happens-before `W(y)` if-and-only-if `x < y`.
+ We examine the behavior of a single read transaction, denoted as `R` and all
  of the ways it can possibly interact with the write sequence.
+ The state of a `SeqLock` consists of a single unsigned integer sequence number
  denoted as `Seq`.  It is assumed that the size of `Seq` contains sufficient
  distinct state so as to never roll-over for the lifetime of the system using
  the sequence number.
+ The protected data used in our examples consists of two words, denoted as
  `p[0], p[1]` which will be known as the "payload".  While the size of these
  words is undefined, they must be words which can be atomically loaded and
  stored on the architecture which the algorithm is being run.  The number of
  words must be at least two (else a `SeqLock` would not be needed, simple
  atomics would be sufficient), but may be arbitrarily large without loss of
  generality for the arguments presented here.
+ Atomic operations are written using a notation similar to that used by the
  standard C++ atomics library.  Please refer to the standard library API
  documentation if clarification is needed.

### Writer exclusion and ordering

The `libconcurrent` implementation of `SeqLock` has a built-mechanism for
guaranteeing the writer-exclusion and ordering requirements described
previously.  This section will demonstrate how and why this mechanism works.

In addition, there are two different ways for observing/updating the data
protected by a `SeqLock` instance which satisfy the `SeqLock` requirements.
Both will be eventually presented, but for right now, the specific details of
how these two implementations work will be omitted for simplicity.

#### Operations

The operations conducted during a write transaction are as follows.

```
1) while ((observed_seq = Seq.load(relaxed)) & 0x1) goto 1;
2) if (!Seq.compare_and_exchange(observed_seq, observed_seq + 1, acquire, relaxed))
     goto 1;
3) ReadModifyWritePayload();
4) Seq.fetch_add(1, release);
```

#### Exclusion

Consider an arbitrary write transaction, `W(x)`.  The transaction begins by
loading `Seq` with relaxed semantics repeatedly, as long as the observed value
for `Seq` is odd.  Once an even value is observed (call it `N`), `W(x)` attempts a
compare-and-exchange operation, attempting to change the value of `Seq` from `N`
to `N+1`.  When successful, the CMPX operations imposes acquire semantics on the
load of `Seq` it performed.  When the transaction is complete, it increments
`Seq`, transitioning it from `N+1` to `N+2`.  The value of `N` is even, meaning
that the value of `Seq` during the transaction (`N+1`) must be odd, and only
transitions back to an even value (`N+2`) after the transaction has completed.

So, in order to enter a write transaction, `W(x)` must successfully transition
`Seq` from an even number to an odd number.  Step #1 ensures that a `W(x)` will
never attempt to CMPX `Seq` in a way which would result in a transition from an
odd number to an even one.  The atomic nature of step #2 ensures that only one
thread can succeed, even if multiple threads are attempting this operation
concurrently.  Any threads which fail the CMPX will go back to waiting for `Seq`
to become even again before attempting a new CMPX operation.  So, only one
thread at a time can successfully enter a transaction at a time.  Furthermore,
only this thread can transition `Seq` back to an even value (in step #4),
allowing a new write transaction executing on a different thread to start.

Therefore, all write transactions are exclusive.

#### Ordering.

At the successful start of `W(x)` it observed the value of `Seq` with acquire
semantics (step #2), and at the end of `W(x)`, an even value of `Seq` is written
with release semantics.  Steps #2 and #4 are the only places where `Seq` is
modified, and #2 always transitions `Seq` from even to odd, while #4 always
transitions `Seq` from odd to even.  This means that whatever value is observed
in step #2 by `W(x)`, it was even, and must be a value which was written during
step #4 by a different write transaction `W(x-1)`.  `W(x-1)`' store was
performed with release semantics, and the observation of this value was done
by `W(x)` with a load-acquire.  This store and the load therefore
synchronize-with each other, and we can say that the start of `W(x)`
happens-after the end of `W(x-1)`.  Likewise `W(x+1)` must happen-after `W(x)`,
and generally speaking `x > y => W(x) happens-after W(y)`.

As will be shown shortly, all read transactions start by performing a
load-acquire of `Seq`, which must synchronize-with one of the stores performed in
step #4 of one of the writers.  This synchronization guarantees that all readers
will see the same order of write transactions that the writers themselves
observe, and all readers and writers see the same order of write transactions.

Therefore, all write transactions happen in a well defined order which is the
same for all observers.

#### Payload side effects.

Any loads of payload data in a write transaction during step #3 must
happen-after any synchronization point established by the CMPX-acquire in step
#2, and must happen-before the synchronization point established by the
store-release performed in step #4.

From this, we know that any loads performed (regardless of memory order) by
`W(x)` must see the most recent value value written by a transaction `W(y); y <
x`, and any values stored by `W(x)` will be observed all transactions `W(z); z >
x`, at least until a subsequent transaction overwrites that value.

#### Summary

Essentially, the `libconcurrent` `SeqLock` implementation implements a simple
exclusive spinlock for write transactions using a single bit of `Seq`s state.
This spinlock behavior establishes not only exclusion for write transaction, but
also ordering.

### Read transactions.  Outline and proof requirements.

Any arbitrary read transaction, `R`, executes the following steps, regardless of
the method being used to load and store the payload.

```
1) while ((before = Seq.load(acquire)) & 0x1) goto 1;
2) CopyPayloadToLocalStorage();
3) after = Seq.load(relaxed);
4) success = (before == after)
```

In order to demonstrate that successful read transaction meet the previously
stated requirements, we must show:

1) If all of the payload loads of `R` observe values written by a single write
   transaction `W(x)`, it must be possible for `R` to observe `before == after`.
2) If, during execution of `R`, there exist different two payload loads
   (`p[0]`, `p[1]`) which observe values written by two different write
   transactions (`W(x)` and `W(y)`, `x != y`), then `before != after` must be true.

In other words; If the payload observations of `R` observe values written by the
same `W(x)`, then it must be possible for the transaction to succeed, and if any
observe values written by different transactions, then the transaction *must*
fail.

### Synchronization using Acquire/Release for payload loads/stores.

Now let's examine one of the two ways we can use to guarantee that we meet these
requirements.  Working from the previous outlines, we expand them to be:

```
---------- Writers ----------
1) while ((observed_seq = Seq.load(relaxed)) & 0x1) goto 1;
2) if (!Seq.compare_and_exchange(observed_seq, observed_seq + 1, acquire, relaxed))
     goto 1;
3) p[0].store(new_p[0], release);
4) p[1].store(new_p[1], release);
5) Seq.fetch_add(1, release);

---------- Readers ----------
1) while ((before = Seq.load(acquire)) & 0x1) goto 1;
2) local_p[0] = p[0].load(acquire);
3) local_p[1] = p[1].load(acquire);
4) after = Seq.load(relaxed);
5) success = (before == after)
```

Note that all stores to the payload performed by writers are done with release
semantics, while all of the loads of the payload performed by a reader are done
with acquire semantics.  These are the key requirements needed to make this
system work.

Start by noting that by the time we make it to step `R.2` the load-acquire of
`Seq` performed by step `R.1` must have observed an even value.  Step `W.5` is
the only place where we ever store an even value to `Seq`, and we do so with
release semantics.

So, for a read transaction `R`, we know:

+ There exists a write transaction `W(x)` where step `W(x).5` synchronizes-with
  step `R.1`.
+ The loads performed by steps `R.[23]` must happen-after the load-acquire
  performed by step `R.1` (because of the acquire semantics of `R.1`).
+ The stores performed by steps `W(x).[34]` must happen-before the store-release
  performed by step `W(x).5` (because of the release semantics of `W(x).5`).
+ Therefore, `R`s payload loads must happen-after `W(x)`s payload stores.

So, these each of these loads can observe _any_ corresponding value written by
the set of write transactions `{W(y)} : y >= x`.

#### Proof of requirement #1

Now consider the case where all of the loads observe the values written by
`W(x)`, the write transaction from which the original value of `Seq` was
observed.  We know:

+ The load-acquire during step `R.1` synchronizes-with the store-release during `W(x).5`
+ The load-acquire during step `R.2` synchronizes-with the store-release during `W(x).3`
+ The load-acquire during step `R.3` synchronizes-with the store-release during `W(x).4`
+ The load-relaxed during step `R.4` must happen-after the load-acquire during `R.3`
  (because of the acquire semantics of the `R.3` load)

So, the set of values that the `R.4` load of `Seq` can possibly observe consists
of value stored by `W(x).5` (which is the same value observed by step `R.1`, and
all of the subsequent values written in steps 2 and 5 of all of the subsequent
write transactions `{ W(y); y > x }`.

Therefore if `R` observes only payload values written by `W(x)`, it is possible
that it also observes the value written to `Seq` by `W(x).5` during steps `R.1`
and `R.4` (although it is not _guaranteed_ to do so).  Thus, we have
demonstrated the first of our proof requirements.

#### Proof of requirement #2

Now consider the case where at least one of the `R`s payload loads observe a
value written by a transaction other than `W(x)`, `W(y); y > x`.  It does not
matter which payload member, so without loss of generality, let's say that it was
the load of `p[0]` which observes the value stored by `W(y)`.  We know:

+ The load-acquire during step `R.2` synchronizes-with the store-release during `W(y).3`
+ The load-relaxed during step `R.4` must happen-after the load-acquire during `R.2`
  (because of the acquire semantics of the `R.2` load)
+ `W(y)` must happen-after `W(x)`, because of the exclusive/ordered properties
  we established earlier, and because `y > x`.

So the set of possible values which can be observed by the load.relaxed in step
`R.4` must be one of the values written by steps #2 and #5 during any of the
members of the set `{ W(y); y > x }`.  None of these values are equal to the
value written by `W(x).5`, which was the value observed during `R.1`. Therefore,
if any payload values observed by `R` was a payload value written by a
transaction other than `W(x)`, the sequence numbers observed by `R` must not
match, and the transaction must fail.  Thus, we have demonstrated the second of
our proof requirements.

### Synchronization using Acquire/Release fences and Relaxed payload loads/stores.

Our second option for properly synchronizing uses acquire/release fences,
instead of acquire/release semantics on individual payload loads/stores.  The
payload loads/stores must remain atomic, in order to avoid a race condition
caused by a write transaction writing concurrently with a read transaction
attempting to make an observation, but these atomic operations only need relaxed
semantics, and nothing more.

Here are the outlines of the two operations:

```
---------- Writers ----------
1) while ((observed_seq = Seq.load(relaxed)) & 0x1) goto 1;
2) if (!Seq.compare_and_exchange(observed_seq, observed_seq + 1, acquire, relaxed))
     goto 1;
3) atomic_thread_fence(release);
4) p[0].store(new_p[0], relaxed);
5) p[1].store(new_p[1], relaxed);
6) Seq.fetch_add(1, release);

---------- Readers ----------
1) while ((before = Seq.load(acquire)) & 0x1) goto 1;
2) local_p[0] = p[0].load(relaxed);
3) local_p[1] = p[1].load(relaxed);
4) atomic_thread_fence(acquire);
5) after = Seq.load(relaxed);
6) success = (before == after)
```

The requirements for our proof-of-correctness remain the same, as does our
initial setup.  We know that we have a read transaction `R`, for which the
payload loads must happen-after the payload stores performed by a write
transaction `W(x)`.

#### Definition of fence-fence synchronization.

Before proceeding, let's take a quick look at the [definition of fence-fence
synchronization](https://en.cppreference.com/w/cpp/atomic/atomic_thread_fence)
as summarized by cppreference.com.

```
Fence-fence synchronization

A release fence FA in thread A synchronizes-with an acquire fence FB in thread B, if

+ There exists an atomic object M,
+ There exists an atomic write X (with any memory order) that modifies M in
  thread A FA is sequenced-before X in thread A
+ There exists an atomic read Y (with any memory order) in thread B Y reads the
  value written by X (or the value would be written by release sequence headed
  by X if X were a release operation)
+ Y is sequenced-before FB in thread B

In this case, all non-atomic and relaxed atomic stores that are sequenced-before
FA in thread A will happen-before all non-atomic and relaxed atomic loads from
the same locations made in thread B after FB
```

So, an acquire fence synchronizes with a release fence if and only if there
exists an atomic load performed before (in sequence order) the acquire-fence on
thread B, which observes the value written by a store performed (in sequence
order) after the release-fence on thread B.

#### Proof of requirement #1

Once again, consider the case where all of the loads observe the values written
by `W(x)`, the write transaction from which the original value of `Seq` was
observed.  We know:

+ The load-acquire during step `R.1` synchronizes-with the store-release during
  `W(x).6`
+ When the load-relaxed during step `R.2` observes the value written by the
  store-relaxed during `W(x).4` it means that `R.4`'s fence-acquire must
  synchronizes-with `W(x).3`'s fence-release.
+ When the load-relaxed during step `R.3` observes the value written by the
  store-relaxed during `W(x).5` it means that `R.4`'s fence-acquire must
  synchronizes-with `W(x).3`'s fence-release.
+ The load-relaxed during step `R.5` must happen-after the fence-acquire during
  `R.4` (because of the acquire semantics of `R.3`'s fence).

So, the set of values that the `R.5` load of `Seq` can possibly observe consists
of value stored by `W(x).5` (which is the same value observed by step `R.1`, and
all of the subsequent values written in steps 2 and 6 of all of the subsequent
write transactions `{ W(y); y > x }`.

Therefore if `R` observes only payload values written by `W(x)`, it is possible
that it also observes the value written to `Seq` by `W(x).6` during steps `R.1`
and `R.5` (although it is not _guaranteed_ to do so).  Thus, we have
demonstrated the first of our proof requirements.

#### Proof of requirement #2

Once again, consider the case where at least one of the `R`s payload loads observe a
value written by a transaction other than `W(x)`, `W(y); y > x`.  It does not
matter which payload member, so without loss of generality, let's say that it was
the load of `p[0]` which observes the value stored by `W(y)`.  We know:

+ When the load-relaxed during step `R.2` observes the value written by the
  store-relaxed during `W(y).4` it means that `R.4`'s fence-acquire must
  synchronizes-with `W(y).3`'s fence-release.
+ The load-relaxed during step `R.5` must happen-after the fence-acquire during
  `R.4` (because of the acquire semantics of `R.3`'s fence).
+ `W(y)` must happen-after `W(x)`, because of the exclusive/ordered properties
  we established earlier, and because `y > x`.

So the set of possible values which can be observed by the load.relaxed in step
`R.5` must be one of the values written by steps #2 and #6 during any of the
members of the set `{ W(y); y > x }`.  None of these values are equal to the
value written by `W(x).6`, which was the value observed during `R.1`. Therefore,
if any payload values observed by `R` was a payload value written by a
transaction other than `W(x)`, the sequence numbers observed by `R` must not
match, and the transaction must fail.  Thus, we have demonstrated the second of
our proof requirements.

