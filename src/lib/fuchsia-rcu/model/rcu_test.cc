// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <threads.h>

#include <condition_variable>
#include <mutex>

#include "librace.h"
#include "model-assert.h"

using Func = void (*)(void*);

// RCU Implementation mimicking fuchsia-rcu
struct Callback {
  Func func;
  void* arg;
  void* next;
};

atomic_int generation;
atomic_int read_counters[2];
atomic_uintptr_t callback_chain;

// The real implementation uses a futex for the advancer and a mutex for waiting_callbacks.
// This model uses std::mutex and std::condition_variable to simulate that behavior.
struct State {
  std::mutex waiting_callbacks_mtx, advancer_mtx;
  std::condition_variable advancer_cnd;
}* state;

Callback* pending_callbacks = nullptr;

void my_rcu_read_lock(int* index) {
  int gen = atomic_load_explicit(&generation, memory_order_relaxed);
  *index = gen & 1;
  atomic_fetch_add_explicit(&read_counters[*index], 1, memory_order_seq_cst);
}

void my_rcu_read_unlock(int index) {
  if (atomic_fetch_sub_explicit(&read_counters[index], 1, memory_order_seq_cst) == 1) {
    state->advancer_mtx.lock();
    state->advancer_mtx.unlock();
    state->advancer_cnd.notify_all();
  }
}

void rcu_call(Func func, void* arg) {
  // We need to synchronize with the rcu_read_lock.
  atomic_thread_fence(memory_order_release);
  atomic_fetch_add_explicit(&read_counters[0], 0, memory_order_relaxed);
  atomic_fetch_add_explicit(&read_counters[1], 0, memory_order_relaxed);

  Callback* cb = (Callback*)malloc(sizeof(Callback));
  cb->func = func;
  cb->arg = arg;
  for (;;) {
    uintptr_t old_head = atomic_load_explicit(&callback_chain, memory_order_relaxed);
    store_64(&cb->next, old_head);
    if (atomic_compare_exchange_strong_explicit(&callback_chain, &old_head, (uintptr_t)cb,
                                                memory_order_release, memory_order_relaxed)) {
      break;
    }
  }
}

void rcu_grace_period() {
  state->waiting_callbacks_mtx.lock();

  Callback* ready = pending_callbacks;

  pending_callbacks = (Callback*)atomic_exchange_explicit(&callback_chain, 0, memory_order_acquire);

  int gen = atomic_fetch_add_explicit(&generation, 1, memory_order_relaxed);

  state->advancer_mtx.lock();
  while (atomic_load_explicit(&read_counters[gen & 1], memory_order_acquire) > 0) {
    state->advancer_cnd.wait(state->advancer_mtx);
  }
  state->advancer_mtx.unlock();

  state->waiting_callbacks_mtx.unlock();

  while (ready != nullptr) {
    Callback* next = (Callback*)load_64(&ready->next);
    ready->func(ready->arg);
    free(ready);
    ready = next;
  }
}

void my_rcu_synchronize() {
  rcu_grace_period();
  rcu_grace_period();
}

// DirEntry test structures
struct DirEntry {
  uint8_t alive;
};

atomic_uintptr_t global_parent;

void drop_dir_entry(void* arg) {
  DirEntry* e = (DirEntry*)arg;
  store_8(&e->alive, 0);
}

void thread_reader(void* arg) {
  int index;
  my_rcu_read_lock(&index);
  DirEntry* p = (DirEntry*)atomic_load_explicit(&global_parent, memory_order_acquire);
  if (p) {
    MODEL_ASSERT(load_8(&p->alive) == 1);
  }
  my_rcu_read_unlock(index);
}

void thread_writer(void* arg) {
  DirEntry* new_p = (DirEntry*)malloc(sizeof(DirEntry));
  new_p->alive = 1;

  uintptr_t old_p_val =
      atomic_exchange_explicit(&global_parent, (uintptr_t)new_p, memory_order_acq_rel);
  DirEntry* old_p = (DirEntry*)old_p_val;
  if (old_p) {
    rcu_call(drop_dir_entry, old_p);
  }
}

int user_main(int argc, char** argv) {
  state = new State;

  atomic_init(&generation, 0);
  atomic_init(&read_counters[0], 0);
  atomic_init(&read_counters[1], 0);
  atomic_init(&callback_chain, 0);

  DirEntry* initial_p = (DirEntry*)malloc(sizeof(DirEntry));
  initial_p->alive = 1;
  atomic_init(&global_parent, (uintptr_t)initial_p);

  thrd_t t1, t2;
  thrd_create(&t1, thread_reader, nullptr);
  thrd_create(&t2, thread_writer, nullptr);

  my_rcu_synchronize();

  thrd_join(t1);
  thrd_join(t2);

  my_rcu_synchronize();

  MODEL_ASSERT(initial_p->alive == 0);

  return 0;
}
