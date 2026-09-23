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
// Design A SeqLock which is intended to allow concurrent reads, and write
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
// ++ Relaxed payload loads/stores are used in this example.  Fence-to-fence
//    synchronization is used to guarantee proper memory ordering semantics.
//
// The outline of read and write transactions look like the following.
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
  // Spin until we observe an even sequence number using relaxed semantics.
  // Then attempt to increment that even number using acquire semantics.
  uint64_t seq;
  do {
    while ((seq = s.seq.load(std::memory_order_relaxed)) & 1u) {
      thrd_yield();
    }
  } while (!s.seq.compare_exchange_strong(seq, seq + 1, std::memory_order_acquire,
                                          std::memory_order_relaxed));

  // Now, drop a release-fence into the sequence.  This will prevent the relaxed
  // store of the sequence number from moving past the fence.  Therefore, the
  // relaxed store of the sequence number cannot happen-after the stores of the
  // payload.
  std::atomic_thread_fence(std::memory_order_release);

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
