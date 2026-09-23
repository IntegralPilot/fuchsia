// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <model-assert.h>
#include <stdatomic.h>
#include <stdio.h>
#include <threads.h>

#include <mutex>

#include "librace.h"

// NOTE: This model must be run with the -Y flag.

// To simulate the MMU code side we have a pretend parent page table entry "current_page" that will
// refer to another page table (one of pages[]). The goal of the model is to show that when the
// reader looks at pages[] with what it saw as the current_page, that it never sees the 'secret',
// i.e. it does not have a use after free.
atomic_int pages[2];
atomic_int current_page;

// 'Secret' value stored in a page when the 'mmu' should not have a reference.
static constexpr int kSecret = 42;

// Due to Issues with state space explosion this variable controls whether the checker terminates
// by upgrading some of the memory orders to seq_cst. Compared to more weaker memory orders, seq_cst
// causes less states since there are less load/store delays and reorders to be considered. With
// multiple threads having related weak loads/stores the combinations increase exponentially.
// Settings this to false does not cause the checker to find any errors, but it also fails to
// terminate.
static constexpr bool kForceSeqCst = true;

// All of the state is bit packed into one atomic. Only 5 bits of this are actually used, with the
// layout: 0bGBBAA. These bits are
//  G - Generation. 0, or 1 to select between the A and B generation
//  A - Number of readers of the A generation.
//  B - Number of readers of the B generation.
// A and B are capped at 2 bits, since we already cannot really check two parallel readers, so these
// aren't going to overflow.
static constexpr size_t kGenShift = 4;
static constexpr size_t kCountMask = 0b11;
static constexpr size_t kGenCountMult = 2;
atomic_int state_variable;

static int read_lock() {
  int initial_gen = atomic_load_explicit(&state_variable, kForceSeqCst ? memory_order_seq_cst
                                                                       : memory_order_relaxed) >>
                    kGenShift;
  int current_gen =
      atomic_fetch_add_explicit(&state_variable, 1 << (initial_gen * kGenCountMult),
                                kForceSeqCst ? memory_order_seq_cst : memory_order_acquire) >>
      kGenShift;
  if (initial_gen == current_gen) {
    return current_gen;
  }
  current_gen =
      atomic_fetch_add_explicit(&state_variable, 1 << (current_gen * kGenCountMult),
                                kForceSeqCst ? memory_order_seq_cst : memory_order_acquire) >>
      kGenShift;
  atomic_fetch_sub_explicit(&state_variable, 1 << ((1 - current_gen) * kGenCountMult),
                            kForceSeqCst ? memory_order_seq_cst : memory_order_relaxed);
  return current_gen;
}

static void read_unlock(int gen) {
  atomic_fetch_sub_explicit(&state_variable, 1 << (gen * kGenCountMult),
                            kForceSeqCst ? memory_order_seq_cst : memory_order_release);
}

static void synchronize() {
  int old_state = atomic_fetch_xor_explicit(&state_variable, 1 << kGenShift, memory_order_acq_rel);
  int old_gen = old_state >> kGenShift;
  if (((old_state >> (old_gen * kGenCountMult)) & kCountMask) == 0) {
    return;
  }
  while (
      ((atomic_load_explicit(&state_variable, memory_order_acquire) >> (old_gen * kGenCountMult)) &
       kCountMask) > 0) {
    thrd_yield();
  }
}

static void reader(void*) {
  int gen = read_lock();
  // The acquire here matches the release order in the writer thread.
  int p = atomic_load_explicit(&current_page, memory_order_acquire);
  // Should only ever see zero, and not the secret.
  MODEL_ASSERT(atomic_load_explicit(&pages[p], memory_order_relaxed) == 0);
  read_unlock(gen);
}

// Note: The writer, because we use a single one that iterates multiple times, could be made more
// checker optimal to reduce state spaces. In my testing minimizing the states here did not actually
// result in a model that could be checked without still forcing seq_cst (since the majority of the
// state space explosion stems from the two readers interacting with each other) and so there's no
// reason not to leave this fully explicit.
static void writer() {
  for (int i = 0; i < 2; i++) {
    // To avoid unnecessary interleavings, and because we trust the implementation of mutexes,
    // instead of actually having two writer threads with a lock, we assume a single writer that
    // iterates 1 or more times. Because there otherwise would be a lock we simulate the memory
    // order of the lock, which is an acquire/release, for the sake of accuracy, although since
    // there is only a single writing thread these have no actual impact.
    atomic_thread_fence(memory_order_acquire);

    int page = atomic_load_explicit(&current_page, memory_order_relaxed);

    // Before changing the page first remove any secret from it (i.e. initialize it).
    atomic_store_explicit(&pages[1 - page], 0, memory_order_relaxed);
    // The modification to the page above *must* become visible before the page can be found, hence
    // must use a release order here. This is paired with an acquire in the reader.
    atomic_store_explicit(&current_page, 1 - page, memory_order_release);

    synchronize();

    // This page should not be visible to any readers, can store our secret in it.
    atomic_store_explicit(&pages[page], kSecret, memory_order_relaxed);

    atomic_thread_fence(memory_order_release);
  }
}

int user_main(int argc, char** argv) {
  thrd_t r1, r2;

  atomic_init(&state_variable, 0);
  atomic_init(&pages[0], 0);
  atomic_init(&pages[1], kSecret);
  atomic_init(&current_page, 0);

  thrd_create(&r1, (thrd_start_t)&reader, NULL);
  thrd_create(&r2, (thrd_start_t)&reader, NULL);

  writer();

  thrd_join(r1);
  thrd_join(r2);
  return 0;
}
