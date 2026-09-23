// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdio.h>
#include <threads.h>

#include <array>
#include <atomic>

#include "librace.h"
#include "model-assert.h"

//
// Requirements:
//
// Design a SeqLock which is intended to allow concurrent reads, and write
// transactions from different threads running concurrently.  The sequence
// number itself serves both as the transaction failure detector for readers, as
// well as the spinlock-style exclusive lock for writers.
//
// The SeqLock implementation described in this example must tolerate the
// following thread behaviors, and obey the following rules.
//
// ++ Readers are allowed to execute concurrently with writers
// ++ Writer may attempt to execute concurrently (from multiple threads), but
//    the SeqLock implementation must not allow them to modify the payload at
//    the same time.
// ++ Writers may read the payload contents during a transaction, in addition to
//    writing them.
// ++ Relaxed payload loads/stores are to be used in this example.  An attempt
//    is made to use atomic-to-fence synchronization to guarantee proper memory
//    ordering semantics.  Note: THIS DOES NOT WORK.
//
// Background:
//
// Start by considering the outline of the read/write transactions for a SeqLock
// using fence-to-fence synchronization.
//
// :: Read Transaction ::
// 1) while ((before = Seq) & 0x1) {};    // Acquire semantics
// 2) ReadPayload()                       // Relaxed semantics for all members
// 3) Fence(Acquire)
// 4) after = Seq;                        // Relaxed semantics
// 5) if (before != after) goto 1;
//
// :: Write Transaction ::
// 1) while ((before = Seq) & 0x1) {};    // Relaxed semantics
// 2) if (!CMPX(Seq, before + 1)) goto 1; // Acquire semantics
// 3) Fence(Release)
// 4) WritePayload()                      // Relaxed semantics for all members
// 5) Seq += 1;                           // Release semantics
//
// The Hypothesis:
//
// Can we get rid of the release fence used by the writer?  What if we changed
// the CMPX to use acq_rel semantics instead of using a fence?  The Write
// transaction would now look like this.
//
// :: Write Transaction ::
// 1) while ((before = Seq) & 0x1) {};    // Relaxed semantics
// 2) if (!CMPX(Seq, before + 1)) goto 1; // Acq_Rel semantics
// 3) WritePayload()                      // Relaxed semantics for all members
// 4) Seq += 1;                           // Release semantics
//
// So, it feels like we have merged the release from the release fence in the
// previous version step #3, with the CMPX in step #2.  The acquire fence should
// be able to synchronize-with this new CMPX, just like it did with the fence
// previously, right?
//
// Turns out, the answer is "No, it does not."
//
// The Failure:
//
// The CDS-checker model (below) finds the problem with this idea.  We model a
// system with one reader (R1), and two writers (W1 and W2), all executing
// concurrently.  After a successful read, the reader must have observed one of
// the following:
//
// 1) The initial state of the payload.
// 2) The state of the payload as written by W1.
// 3) The state of the payload as written by W2.
//
// Any mixing of the payload would be a failure, and indeed, this does fail.
// Here is one example from the checker.  The addresses of the accesses have
// been "symbolized" for ease of reading, and unrelated accesses (those used
// when launching threads) have been removed.  Finally, the full sequence is
// presented in execution order, and then again grouped by thread.
//
// 1) The initial state of the payload is P = (0, 1)
// 2) W1 writes P = (2, 3)
// 3) W2 writes P = (4, 5)
//
// ------------------------------------------------------------------------------------
// #    t    Action type     MO       Location         Value               Rf  CV
// ------------------------------------------------------------------------------------
// 2    1    atomic write    seq_cst             Seq   0                       ( 0,  2)
// 3    1    atomic write    seq_cst            p[0]   0                       ( 0,  3)
// 4    1    atomic write    seq_cst            p[1]   0x1                     ( 0,  4)
// 7    2    atomic read     acquire             Seq   0                   2   ( 0,  5,  7)
// 10   2    atomic read     relaxed            p[0]   0                   3   ( 0,  5, 10)
// 11   3    atomic read     relaxed             Seq   0                   2   ( 0,  8,  0, 11)
// 14   2    atomic read     relaxed            p[1]   0x1                 4   ( 0,  5, 14)
// 15   3    atomic rmw      acq_rel             Seq   0                   2   ( 0,  8,  0, 15)
// 16   4    atomic read     relaxed             Seq   0                   2   ( 0, 12,  0,  0, 16)
// 17   2    fence           acquire             0x7   0xdeadbeef              ( 0,  5, 17)
// 18   3    atomic rmw      relaxed            p[0]   0                   3   ( 0,  8,  0, 18)
// 19   4    atomic read     acq_rel             Seq   0x1                 15  ( 0, 12,  0, 15, 19)
// 20   2    atomic read     relaxed             Seq   0x1                 15  ( 0,  5, 20)
// 21   3    atomic rmw      relaxed            p[1]   0x1                 4   ( 0,  8,  0, 21)
// 22   4    atomic read     relaxed             Seq   0x1                 15  ( 0, 12,  0, 15, 22)
// 23   2    atomic read     acquire             Seq   0x1                 15  ( 0,  8, 23, 15)
// 24   3    atomic rmw      release             Seq   0x1                 15  ( 0,  8,  0, 24)
// 28   4    atomic read     relaxed             Seq   0x2                 24  ( 0, 12,  0, 15, 28)
// 29   2    atomic read     acquire             Seq   0x2                 24  ( 0,  8, 29, 24)
// 30   4    atomic rmw      acq_rel             Seq   0x2                 24  ( 0, 12,  0, 24, 30)
// 31   2    atomic read     relaxed            p[0]   0x2                 18  ( 0,  8, 31, 24)
// 32   4    atomic rmw      relaxed            p[0]   0x2                 18  ( 0, 12,  0, 24, 32)
// 33   2    atomic read     relaxed            p[1]   0x5                 34  ( 0,  8, 33, 24)
// 34   4    atomic rmw      relaxed            p[1]   0x3                 21  ( 0, 12,  0, 24, 34)
// 35   2    fence           acquire             0x7   0xdeadbeef              ( 0,  8, 35, 24)
// 36   4    atomic rmw      release             Seq   0x3                 30  ( 0, 12,  0, 24, 36)
// 37   2    atomic read     relaxed             Seq   0x2                 24  ( 0,  8, 37, 24)
//
// -------------------------- The sequence seen by W1 --------------------------
//
// W1 sees no contention at all.  It grabs the "lock" and writes (2, 3) to the
// payload, then exits.
//
// 11   3    atomic read     relaxed             Seq   0                   2   ( 0,  8,  0, 11)
// 15   3    atomic rmw      acq_rel             Seq   0                   2   ( 0,  8,  0, 15)
// 18   3    atomic rmw      relaxed            p[0]   0                   3   ( 0,  8,  0, 18)
// 21   3    atomic rmw      relaxed            p[1]   0x1                 4   ( 0,  8,  0, 21)
// 24   3    atomic rmw      release             Seq   0x1                 15  ( 0,  8,  0, 24)
//
// -------------------------- The sequence seen by W2 --------------------------
//
// W2 starts by reading Seq and seeing an even number.  However, we see it
// immediately read Seq again (this time observing an odd number), implying that
// the read during the strong CMPX saw a different value from what was expected,
// and therefore failed.  W2 now spins on Seq, waiting for it to become even again.
//
// 16   4    atomic read     relaxed             Seq   0                   2   ( 0, 12,  0,  0, 16)
// 19   4    atomic read     acq_rel             Seq   0x1                 15  ( 0, 12,  0, 15, 19)
// 22   4    atomic read     relaxed             Seq   0x1                 15  ( 0, 12,  0, 15, 22)
//
// We see the sequence number go back to even, and this time we successfully
// increment it, write out payload, and drop the lock.
//
// 28   4    atomic read     relaxed             Seq   0x2                 24  ( 0, 12,  0, 15, 28)
// 30   4    atomic rmw      acq_rel             Seq   0x2                 24  ( 0, 12,  0, 24, 30)
// 32   4    atomic rmw      relaxed            p[0]   0x2                 18  ( 0, 12,  0, 24, 32)
// 34   4    atomic rmw      relaxed            p[1]   0x3                 21  ( 0, 12,  0, 24, 34)
// 36   4    atomic rmw      release             Seq   0x3                 30  ( 0, 12,  0, 24, 36)
//
// -------------------------- The sequence seen by R1 --------------------------
//
// R1 starts a read sequence, and manages to observe the initial state of (0,1),
// but when it reads the sequence number the second time, it sees that the
// counter has been bumped, and it needs to try again.
//
// 7    2    atomic read     acquire             Seq   0                   2   ( 0,  5,  7)
// 10   2    atomic read     relaxed            p[0]   0                   3   ( 0,  5, 10)
// 14   2    atomic read     relaxed            p[1]   0x1                 4   ( 0,  5, 14)
// 17   2    fence           acquire             0x7   0xdeadbeef              ( 0,  5, 17)
// 20   2    atomic read     relaxed             Seq   0x1                 15  ( 0,  5, 20)
// 23   2    atomic read     acquire             Seq   0x1                 15  ( 0,  8, 23, 15)
//
// So, R1 goes back to waiting for Seq to become even again.  Once it does, R1
// successfully observes (2,5), a mix of states written by W1 and W2.  This is an
// incoherent observation, so we are supposed to see a different final sequence
// number during the final read.
//
// 29   2    atomic read     acquire             Seq   0x2                 24  ( 0,  8, 29, 24)
// 31   2    atomic read     relaxed            p[0]   0x2                 18  ( 0,  8, 31, 24)
// 33   2    atomic read     relaxed            p[1]   0x5                 34  ( 0,  8, 33, 24)
// 35   2    fence           acquire             0x7   0xdeadbeef              ( 0,  8, 35, 24)
// 37   2    atomic read     relaxed             Seq   0x2                 24  ( 0,  8, 37, 24)
//
// But, we don't.  Instead, we see 0x2, the value which was written to Seq by W1
// when W1's write transaction finished.  What happened?
//
// The answer has to do the requirements for achieving a synchronizes-with
// relationship using Atomic-Fence synchronization.  These requirements are
// summarized by cppreference.com, copied here for convenience.
//
// ```
// ====== Atomic-fence synchronization ======
// https://en.cppreference.com/w/cpp/atomic/atomic_thread_fence
//
// An atomic release operation X in thread A synchronizes-with an acquire fence
// F in thread B, if
//
// + there exists an atomic read Y (with any memory order)
// + Y reads the value written by X (or by the release sequence headed by X)
// + Y is sequenced-before F in thread B
//
// In this case, all non-atomic and relaxed atomic stores that are
// sequenced-before X in thread A will happen-before all non-atomic and relaxed
// atomic loads from the same locations made in thread B after F.
// ```
//
// What we want here is for the acquire fence in the reader to synchronize-with
// the CMPXs performed by writers at the start or end of their transactions, and
// to do so in a way such that if we see one of the payload values written by a
// writer (Wx), that we must see one of the sequence numbers written by Wx as
// well.
//
// With fence to fence synchronization, this is guaranteed provided that the
// reader loads a value (before the acquire fence, with any memory order)
// stored by a writer (after the release fence, with any memory order).  So,
// when the reader sees a payload value written by W2, it must also see a
// sequence number written by W2.
//
// For atomic-to-fence synchronization to work, a reader must load a value
// (again, before the acquire fence, with any memory order) which was stored by
// a writer after the release, but *with release semantics* (at least).
//
// So, this is the key difference.  The payload stores done by writers are being
// done relaxed.  R can only synchronize-with W2 if it manages to see one of the
// values written by W2 *with release semantics*.  The only value written by W2
// with release semantics are the stores to Seq it performs.  In the specific
// scenario diagrammed here, this does not happen.   R's loads of Seq only
// observe the final value written by W1, and never any of the value written by
// W2.
//
// The W2 payload writes, however, did happen-after the W1 payload writes in
// this scenario (established by the acq/rel when W2 had to wait for W1 to
// finish).  So, it *is* possible for R to see a W2 payload writ, but this does
// not guarantee that it will see (at least) a W2 Seq write.
//
constexpr uint32_t kPayloadWords = 2;
constexpr uint32_t kTotalWriters = 2;

struct State {
  std::atomic<uint64_t> seq;
  std::array<std::atomic<uint32_t>, kPayloadWords> payload;
};

static void Read(State& s) {
  uint64_t before, after;
  std::array<uint32_t, kPayloadWords> vals;

  do {
    // spin until we see an even sequence number using acq loads.
    while ((before = s.seq.load(std::memory_order_acquire)) & 1u) {
      thrd_yield();
    }

    // Load all payloads with relaxed semantics.
    for (uint32_t i = 0; i < vals.size(); ++i) {
      vals[i] = s.payload[i].load(std::memory_order_relaxed);
    }

    // Now, drop an acquire-fence into the sequence, then load the sequence
    // number again, this time with relaxed semantics.  The fence guarantees
    // that the load of the sequence number cannot move before the fence (and any
    // of the payload loads)
    std::atomic_thread_fence(std::memory_order_acquire);
    after = s.seq.load(std::memory_order_relaxed);
  } while (before != after);

  MODEL_ASSERT((vals[0] % kPayloadWords) == 0);
  for (uint32_t i = 1; i < vals.size(); ++i) {
    MODEL_ASSERT((vals[i - 1] + 1) == vals[i]);
  }
}

static void Write(State& s) {
  // Spin until we can increment the sequence number from even to odd, using only relaxed semantics.
  uint64_t seq;
  do {
    while ((seq = s.seq.load(std::memory_order_relaxed)) & 1u) {
      thrd_yield();
    }
  } while (!s.seq.compare_exchange_strong(seq, seq + 1, std::memory_order_acq_rel,
                                          std::memory_order_relaxed));

  // Store the payloads, each with relaxed semantics.
  for (size_t i = 0; i < s.payload.size(); ++i) {
    s.payload[i].fetch_add(kPayloadWords, std::memory_order_relaxed);
  }

  // Increment the sequence number again, this time with release semantics.
  s.seq.fetch_add(1u, std::memory_order_release);
}

extern "C" int user_main(int argc, char** argv) {
  State s;

  printf("Seq  = %p\n", &s.seq);
  s.seq.store(0u);
  for (uint32_t i = 0; i < s.payload.size(); ++i) {
    printf("p[%i] = %p\n", i, &s.payload[i]);
    s.payload[i].store(i);
  }

  thrd_t reader;
  std::array<thrd_t, kTotalWriters> writers;

  thrd_create(
      &reader, [](void* ctx) { Read(*reinterpret_cast<State*>(ctx)); }, &s);

  for (auto& writer : writers) {
    thrd_create(
        &writer, [](void* ctx) { Write(*reinterpret_cast<State*>(ctx)); }, &s);
  }

  thrd_join(reader);
  for (auto& writer : writers) {
    thrd_join(writer);
  }

  return 0;
}
