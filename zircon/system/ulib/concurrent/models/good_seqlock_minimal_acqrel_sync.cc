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
// Design A SeqLock which is intended to allow concurrent reads, and
// non-concurrent writes.  In other words, write operations never overlap each
// other, and all write operations have a well-defined order.
//
// A system in which all write operations came from a single thread would
// satisfy these requirements, as would one where some external mechanism
// guarantees that the writes are ordered and non-overlapping.  The sequence
// number in this system is used only as a transaction failure detector for
// readers, not to guarantee the writer requirements.
//
// The SeqLock implementation described in this example must tolerate the
// following thread behaviors, and obey the following rules.
//
// ++ Readers are allowed to execute concurrently with writers
// ++ Writer may not execute concurrently, and all write transactions must have
//    a well defined order relative to each other.
// ++ Writers may read the payload contents during a transaction, in addition to
//    writing them.
// ++ Payload AcqRel semantics are used in this example, instead of fences.
//

constexpr uint32_t kPayloadWords = 3;
constexpr uint32_t kTotalWrites = 2;

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
    }

    // Load all payloads with acquire.
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
  // Increment the sequence number with relaxed semantics
  s.seq.fetch_add(1u, std::memory_order_relaxed);

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

  thrd_t reader, writer;

  thrd_create(
      &reader,
      [](void* ctx) {
        State& s = *reinterpret_cast<State*>(ctx);
        Read(s);
      },
      &s);

  thrd_create(
      &writer,
      [](void* ctx) {
        State& s = *reinterpret_cast<State*>(ctx);
        for (uint32_t i = 1; i <= kTotalWrites; ++i) {
          Write(s, kPayloadWords * i);
        }
      },
      &s);

  thrd_join(reader);
  thrd_join(writer);

  return 0;
}
