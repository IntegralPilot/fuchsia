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
// Design a SeqLock which is intended to allow concurrent reads, and exclusive
// writes.  The sequence number itself serves both as the transaction failure
// detector for readers, as well as the spinlock-style exclusive lock for
// writers.
//
// The SeqLock implementation described in this example must tolerate the
// following thread behaviors, and obey the following rules.
//
// ++ Readers are allowed to execute concurrently with writers
// ++ Writer may attempt to execute concurrently (from multiple
//    threads), but the SeqLock implementation must not allow them to
//    modify the payload at the same time.
// ++ Writers *may not* read the payload during the transaction.  IOW,
//    they may only write new values to the payload, which do not
//    depend on the old values.
// ++ Payload AcqRel semantics are to be used in this example, instead
//    of fences.
//
// The Hypothesis:
//
// A minimal implementation of a SeqLock where all writes came from a
// single thread would look something like this:
//
// :: Read Transaction ::
// 1) while ((before = Seq) & 0x1) {};  // Acquire semantics
// 2) Read Payload;                     // Acquire semantics for all members
// 3) after = Seq;                      // Relaxed semantics
// 4) if (before != after) repeat;
//
// :: Write Transaction ::
// 1) Seq += 1;      // Relaxed semantics
// 2) Write Payload; // Release semantics for all members
// 3) Seq += 1;      // Release semantics
//
// If we wish to extend this to allow writes to come in from multiple
// threads, we need to make sure that two writers are not allowed to
// modify the payload at the same time.  So, we use the "odd sequence
// number implies write-in-flight" property of our sequence number to
// cause writers to wait in line.  The new sequence is now:
//
// :: Write Transaction ::
// 1) while ((observed = Seq) & 0x1) {};         // Relaxed semantics
// 2) if (!CMPX(observed, observed +1)) goto 1;  // Relaxed semantics
// 3) Write Payload;                             // Release semantics for all members
// 4) Seq += 1;                                  // Release semantics
//
// Note that there are no acquire semantics on either step #1 or step #2.  There
// are no loads of any of the payload values in step #3, so there is nothing in
// step #3 which would be need to be prevented from moving up before steps #1,2.
// Therefore, a load-acquire operation at that point in the sequence would serve
// no purpose, right?
//
// Turns out, the answer is "No, this is not fine".
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
// 2    1    atomic write    seq_cst  Seq              0                       ( 0,  2)
// 3    1    atomic write    seq_cst  p[0]             0                       ( 0,  3)
// 4    1    atomic write    seq_cst  p[1]             0x1                     ( 0,  4)
// 7    2    atomic read     acquire  Seq              0                   2   ( 0,  5,  7)
// 10   2    atomic read     acquire  p[0]             0                   3   ( 0,  5, 10)
// 11   3    atomic read     relaxed  Seq              0                   2   ( 0,  8,  0, 11)
// 14   2    atomic read     acquire  p[1]             0x1                 4   ( 0,  5, 14)
// 15   3    atomic rmw      relaxed  Seq              0                   2   ( 0,  8,  0, 15)
// 16   4    atomic read     relaxed  Seq              0                   2   ( 0, 12,  0,  0, 16)
// 17   2    atomic read     relaxed  Seq              0x1                 15  ( 0,  5, 17)
// 18   3    atomic write    release  p[0]             0x2                     ( 0,  8,  0, 18)
// 19   4    atomic read     relaxed  Seq              0x1                 15  ( 0, 12,  0,  0, 19)
// 20   2    atomic read     acquire  Seq              0x1                 15  ( 0,  5, 20)
// 21   3    atomic write    release  p[1]             0x3                     ( 0,  8,  0, 21)
// 22   4    atomic read     relaxed  Seq              0x1                 15  ( 0, 12,  0,  0, 22)
// 24   3    atomic rmw      release  Seq              0x1                 15  ( 0,  8,  0, 24)
// 26   2    atomic read     acquire  Seq              0x2                 24  ( 0,  8, 26, 24)
// 28   4    atomic read     relaxed  Seq              0x2                 24  ( 0, 12,  0,  0, 28)
// 29   2    atomic read     acquire  p[0]             0x2                 18  ( 0,  8, 29, 24)
// 30   4    atomic rmw      relaxed  Seq              0x2                 24  ( 0, 12,  0,  0, 30)
// 31   2    atomic read     acquire  p[1]             0x3                 21  ( 0,  8, 31, 24)
// 32   4    atomic write    release  p[0]             0x4                     ( 0, 12,  0,  0, 32)
// 33   2    atomic read     relaxed  Seq              0x3                 30  ( 0,  8, 33, 24)
// 34   4    atomic write    release  p[1]             0x5                     ( 0, 12,  0,  0, 34)
// 35   2    atomic read     acquire  Seq              0x3                 30  ( 0,  8, 35, 24)
// 36   4    atomic rmw      release  Seq              0x3                 30  ( 0, 12,  0,  0, 36)
// 39   2    atomic read     acquire  Seq              0x4                 36  ( 0, 12, 39, 24, 36)
// 40   2    atomic read     acquire  p[0]             0x2                 18  ( 0, 12, 40, 24, 36)
// 41   2    atomic read     acquire  p[1]             0x5                 34  ( 0, 12, 41, 24, 36)
// 42   2    atomic read     relaxed  Seq              0x4                 36  ( 0, 12, 42, 24, 36)
//
// -------------------------- The sequence seen by W1 --------------------------
//
// W1 sees no contention at all.  It grabs the "lock" and writes (2, 3) to the
// payload, then exits.
//
// 11   3    atomic read     relaxed  Seq              0                   2   ( 0,  8,  0, 11)
// 15   3    atomic rmw      relaxed  Seq              0                   2   ( 0,  8,  0, 15)
// 18   3    atomic write    release  p[0]             0x2                     ( 0,  8,  0, 18)
// 21   3    atomic write    release  p[1]             0x3                     ( 0,  8,  0, 21)
// 24   3    atomic rmw      release  Seq              0x1                 15  ( 0,  8,  0, 24)
//
// -------------------------- The sequence seen by W2 --------------------------
//
// W2 starts by reading Seq and seeing an even number.  However, we see it immediately
// read Seq again (this time observing an odd number), implying that the relaxed
// read during the strong CMPX saw a different value from what was expected, and
// therefore failed.  W2 now spins on Seq, waiting for it to become even again.
//
// 16   4    atomic read     relaxed  Seq              0                   2   ( 0, 12,  0,  0, 16)
// 19   4    atomic read     relaxed  Seq              0x1                 15  ( 0, 12,  0,  0, 19)
// 22   4    atomic read     relaxed  Seq              0x1                 15  ( 0, 12,  0,  0, 22)
//
// We see the sequence number go back to even, and this time we successfully
// increment it, write out payload, and drop the lock.
//
// 28   4    atomic read     relaxed  Seq              0x2                 24  ( 0, 12,  0,  0, 28)
// 30   4    atomic rmw      relaxed  Seq              0x2                 24  ( 0, 12,  0,  0, 30)
// 32   4    atomic write    release  p[0]             0x4                     ( 0, 12,  0,  0, 32)
// 34   4    atomic write    release  p[1]             0x5                     ( 0, 12,  0,  0, 34)
// 36   4    atomic rmw      release  Seq              0x3                 30  ( 0, 12,  0,  0, 36)
//
// -------------------------- The sequence seen by R1 --------------------------
//
// R1 starts a read sequence, and manages to observe the initial state of (0,1),
// but when it reads the sequence number the second time, it sees that the
// counter has been bumped, and it needs to try again.
//
// 7    2    atomic read     acquire  Seq              0                   2   ( 0,  5,  7)
// 10   2    atomic read     acquire  p[0]             0                   3   ( 0,  5, 10)
// 14   2    atomic read     acquire  p[1]             0x1                 4   ( 0,  5, 14)
// 17   2    atomic read     relaxed  Seq              0x1                 15  ( 0,  5, 17)
//
// So, R1 goes back to waiting for Seq to become even again.  Once it does, R1
// successfully observes (2,3), the state written by W1.  Once again, however,
// it sees the sequence number advance, and so it has to retry.
//
// 20   2    atomic read     acquire  Seq              0x1                 15  ( 0,  5, 20)
// 26   2    atomic read     acquire  Seq              0x2                 24  ( 0,  8, 26, 24)
// 29   2    atomic read     acquire  p[0]             0x2                 18  ( 0,  8, 29, 24)
// 31   2    atomic read     acquire  p[1]             0x3                 21  ( 0,  8, 31, 24)
// 33   2    atomic read     relaxed  Seq              0x3                 30  ( 0,  8, 33, 24)
//
// Back to waiting for an even sequence number.  When we finally see one, we see
// the final sequence number written by W2 (0x4) and proceed to read our
// payload.  When we get to the end, we see the sequence number is still 0x4, so
// we declare victory.
//
// 35   2    atomic read     acquire  Seq              0x3                 30  ( 0,  8, 35, 24)
// 39   2    atomic read     acquire  Seq              0x4                 36  ( 0, 12, 39, 24, 36)
// 40   2    atomic read     acquire  p[0]             0x2                 18  ( 0, 12, 40, 24, 36)
// 41   2    atomic read     acquire  p[1]             0x5                 34  ( 0, 12, 41, 24, 36)
// 42   2    atomic read     relaxed  Seq              0x4                 36  ( 0, 12, 42, 24, 36)
//
// Unfortunately, the payload we read is (2, 5).  We saw p[0] as written by W1,
// and p[1] as written by W2.  This "should" be impossible given the following
// argument:
//
// 1) The read of Seq at #39 synchronized-with the write of Seq at #36.
// 2) The subsequent loads #40-42 must therefore happen-after the write at #36.
// 3) The W2 payload stores #32,34 cannot move past the store at #36.
// 4) Therefore the payload loads #40,41 must have happened-after the stores
//    #32,34.
//
// So how did load #40 manage to see store #18?  The clock vector tells the
// tale.  Load #39 did synchronize-with store #36 in W2, so all of the
// subsequent loads did happen after W2 completed.  But, it also synchronized
// with store #18 from W1, and the loads also happened after W1 completed.
//
// The key point:
//
// The issue here is that W1 and W2 have no ordering relationship.  While R1
// happened-after both W1 and W2, W1 did not happen-after W2, or vice-versa;
// they have no defined order.  Therefore, loads done in R1 can see the state
// left by W1 after store #24 OR they can see the state left by W2 after store
// #36.  In this case, that is what happens.  p[0] comes from the W1 state, and
// p[1] comes from the W2 state.
//
// The fix:
//
// The fix here is to add acquire semantics to the initial load of Seq done
// during the write sequence.  This forces the start of every write transaction
// to synchronize with the end of the previous write transaction.  Now, all of
// the write transactions have a defined order, so when the read sequences
// initial Seq load synchronizes-with at the end of a write transaction, it can
// only see states after that write transaction, never any earlier states.
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
    // spin until we see an even sequence number using load-acquires.
    while ((before = s.seq.load(std::memory_order_acquire)) & 1u) {
      thrd_yield();
    }

    // Load all payloads with acquire semantics.
    for (uint32_t i = 0; i < vals.size(); ++i) {
      vals[i] = s.payload[i].load(std::memory_order_acquire);
    }

    // Load the sequence number again, this time relaxed
    after = s.seq.load(std::memory_order_relaxed);
  } while (before != after);

  MODEL_ASSERT((vals[0] % kPayloadWords) == 0);
  for (uint32_t i = 1; i < vals.size(); ++i) {
    MODEL_ASSERT((vals[i - 1] + 1) == vals[i]);
  }
}

static void Write(State& s, uint32_t start_val) {
  // Spin until we can increment the sequence number from even to odd, using
  // only relaxed semantics.
  uint64_t seq;
  do {
    while ((seq = s.seq.load(std::memory_order_relaxed)) & 1u) {
      thrd_yield();
    }
  } while (!s.seq.compare_exchange_strong(seq, seq + 1, std::memory_order_relaxed));

  // Store the payloads, each with release semantics.
  for (size_t i = 0; i < s.payload.size(); ++i) {
    s.payload[i].store(start_val + i, std::memory_order_release);
  }

  // Increment the sequence number again, this time with release.
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
  struct WriterState {
    State* state;
    thrd_t thread;
    uint32_t id;
  };

  std::array<WriterState, kTotalWriters> writers;
  for (uint32_t i = 0; i < writers.size(); ++i) {
    writers[i].state = &s;
    writers[i].id = i + 1;
  }

  thrd_create(
      &reader,
      [](void* ctx) {
        State& s = *reinterpret_cast<State*>(ctx);
        Read(s);
      },
      &s);

  for (auto& writer : writers) {
    thrd_create(
        &writer.thread,
        [](void* ctx) {
          WriterState& ws = *reinterpret_cast<WriterState*>(ctx);
          Write(*ws.state, kPayloadWords * ws.id);
        },
        &writer);
  }

  thrd_join(reader);
  for (auto& writer : writers) {
    thrd_join(writer.thread);
  }

  return 0;
}
