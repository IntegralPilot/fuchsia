// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/counters.h>
#include <lib/crypto/global_prng.h>

#include <fbl/ref_counted_upgradeable.h>
#include <kernel/auto_preempt_disabler.h>
#include <vm/compression.h>
#include <vm/debug_compressor.h>
#include <vm/vm_cow_pages.h>

namespace {
KCOUNTER(pq_compress_debug_random_compression, "pq.compress.debug_random_compression")
}

VmDebugCompressor::~VmDebugCompressor() { Shutdown(); }

void VmDebugCompressor::Shutdown() {
  Guard<SpinLock, IrqSave> guard{&lock_};
  if (thread_) {
    ASSERT(state_ != State::Shutdown);
    state_ = State::Shutdown;
    event_.Signal();
    Thread* t = thread_;
    thread_ = nullptr;
    guard.Release();
    int retcode;
    zx_status_t status = t->Join(&retcode, ZX_TIME_INFINITE);
    ASSERT(status == ZX_OK);
  }
}

void VmDebugCompressor::Add(vm_page_t* page, VmCowPages* object, uint64_t offset) {
  bool signal = false;
  {
    Guard<SpinLock, IrqSave> guard{&lock_};
    if (state_ != State::Running || list_->full()) {
      return;
    }
    // Allow 10% of pages to get added.
    if (rand_r(&rng_state_) >= (RAND_MAX / 10)) {
      return;
    }
    // If currently empty then the compression thread will need a kick.
    signal = list_->empty();
    struct Entry entry{fbl::MakeRefPtrUpgradeFromRaw(object, guard), page, offset};
    // Callers of |Add| are required to ensure |object| lives till the end of the call, so the
    // upgrade should never fail.
    ASSERT(entry.cow);
    list_->push(ktl::move(entry));
  }
  if (signal) {
    event_.Signal();
  }
}

void VmDebugCompressor::CompressThread() {
  VmCompression* compression = Pmm::Node().GetPageCompression();
  // It is an error to attempt to be using the debug compressor if compression isn't available.
  ASSERT(compression);
  do {
    // Check for Shutdown prior to waiting to ensure that if a shutdown was triggered while we were
    // processing entries we do not miss it.
    {
      Guard<SpinLock, IrqSave> guard{&lock_};
      if (state_ == State::Shutdown) {
        return;
      }
    }
    zx_status_t status = event_.Wait();
    ASSERT(status == ZX_OK);

    // Acquire the compression instance, cannot keep this acquired between runs to avoid starving
    // any legitimate compression efforts.
    VmCompression::CompressorGuard instance = compression->AcquireCompressor();

    // Work through all items in the list and attempt to compress them.
    for (Entry entry = Pop(); entry.cow; entry = Pop()) {
      status = instance.get().Arm();
      if (status != ZX_OK) {
        // Arm() may fail with ZX_ERR_NO_MEMORY if it cannot allocate temporary resources.
        // Since this is for random debug compression, we can safely skip this page and continue. We
        // still want to process the rest of the list to drop RefPtrs to the VmCowPages which might
        // be holding them alive.
        continue;
      }
      VmCowReclaimResult reclaimed = entry.cow->ReclaimPage(
          entry.page, entry.offset, VmCowPages::EvictionAction::IgnoreHint, &instance.get());
      if (reclaimed.is_ok()) {
        uint64_t count = reclaimed.value().num_pages;
        if (count > 0) {
          pq_compress_debug_random_compression.Add(count);
        }
      }
    }
  } while (true);
}

void VmDebugCompressor::Pause() {
  lockdep::AssertNoLocksHeld();
  {
    Guard<SpinLock, IrqSave> guard{&lock_};
    if (state_ == State::Shutdown) {
      return;
    }
    ASSERT(state_ != State::Paused);
    state_ = State::Paused;
  }
  // Drain list so that we aren't holding VMOs alive during an arbitrarily long pause.
  for (Entry entry = Pop(); entry.cow; entry = Pop()) {
  }
}

void VmDebugCompressor::Resume() {
  Guard<SpinLock, IrqSave> guard{&lock_};
  if (state_ == State::Shutdown) {
    return;
  }
  ASSERT(state_ != State::Running);
  // The list should be empty as nothing should have been added while paused.
  ASSERT(list_->empty());
  state_ = State::Running;
}

VmDebugCompressor::Entry VmDebugCompressor::Pop() {
  Guard<SpinLock, IrqSave> guard{&lock_};
  if (list_->empty()) {
    return Entry{};
  }
  Entry e = ktl::move(list_->front());
  list_->pop();
  return e;
}

zx_status_t VmDebugCompressor::Init() {
  // Allocate objects outside the lock.
  // The compression thread is made as HIGH_PRIORITY since it will be holding refptrs to VMOs, and
  // could be inadvertently keeping those VMOs alive if the list is not processed fast enough.
  Thread* t = Thread::Create(
      "page-queue-debug-compress-thread",
      [](void* arg) -> int {
        static_cast<VmDebugCompressor*>(arg)->CompressThread();
        return 0;
      },
      this, HIGH_PRIORITY);
  if (!t) {
    return ZX_ERR_NO_MEMORY;
  }

  fbl::AllocChecker ac;
  ktl::unique_ptr<fbl::RingBuffer<Entry, kArraySize>> list(
      new (&ac) fbl::RingBuffer<Entry, kArraySize>());
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  AnnotatedAutoPreemptDisabler apd;
  Guard<SpinLock, IrqSave> guard{&lock_};
  ASSERT(!thread_);
  list_ = ktl::move(list);
  thread_ = t;
  thread_->Resume();
  state_ = State::Running;
  crypto::global_prng::GetInstance()->Draw(&rng_state_, sizeof(rng_state_));
  return ZX_OK;
}
