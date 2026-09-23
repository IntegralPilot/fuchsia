// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_DEBUG_COMPRESSOR_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_DEBUG_COMPRESSOR_H_

#include <zircon/types.h>

#include <fbl/array.h>
#include <fbl/macros.h>
#include <fbl/ring_buffer.h>
#include <kernel/mutex.h>
#include <kernel/thread.h>
#include <vm/page.h>

class VmCowPages;

// A debug compressor that can be given references to pages in VMOs and will randomly compress a
// subset of them. The compression will be performed in a difference Zircon thread, so the pages
// can be given with arbitrary locks held.
class VmDebugCompressor {
 public:
  VmDebugCompressor() = default;
  ~VmDebugCompressor();

  // Initializes the debug compressor. This method may acquire Mutexes, and so must not be called
  // with any spinlocks held. Other methods should not be called before |Init| is called and returns
  // ZX_OK.
  zx_status_t Init();

  // Adds the specified |page| at |offset| in |object| to the debug compressor as a candidate for
  // compression.  The |page| and |object| must remain valid until |Add| returns. This implies that
  // |object| lock must be held, however this cannot be stated with an TA_REQ statement since
  // VmCowPages is not declared yet.
  void Add(vm_page_t* page, VmCowPages* object, uint64_t offset);

  // Pauses the debug compressor such that all future |Add| calls will be ignored. It is an error to
  // call |Pause| twice without calling |Resume| in between. Pause might acquire arbitrary VMO and
  // other locks and should not be called with other locks held.
  void Pause();

  // Resumes from a |Pause|, causing calls to |Add| to no longer be ignored. It is an error to call
  // |Resume| except after having called |Pause|.
  void Resume();

 private:
  struct Entry {
    fbl::RefPtr<VmCowPages> cow;
    vm_page_t* page = nullptr;
    uint64_t offset = 0;
  };

  // Entry point for the |thread_| that performs the actual compression.
  void CompressThread();

  // Helper that Pops an entry from the list_. Will return an entry with a null cow if the list is
  // empty.
  Entry Pop() TA_EXCL(lock_);

  // Shuts down and cleans up |thread_|.
  void Shutdown() TA_EXCL(lock_);

  // Size of the |list_| that will be allocated.
  static constexpr size_t kArraySize = 128;

  DECLARE_SPINLOCK(VmDebugCompressor) lock_;

  // Reference to the thread that does compression so we can shut it down later.
  Thread* thread_ TA_GUARDED(lock_) = nullptr;

  // The array of entries is used to transfer pages from the synchronous |Add| call to the
  // compression thread. The list is finite and if full pages will be silently dropped, which is
  // equivalent to as if those pages were randomly chosen to not be added.
  ktl::unique_ptr<fbl::RingBuffer<Entry, kArraySize>> list_ TA_GUARDED(lock_);

  enum class State {
    Shutdown,
    Running,
    Paused,
  };
  // State is initially shutdown to require |Init| to be called.
  State state_ TA_GUARDED(lock_) = State::Shutdown;

  // Private rng state to avoid further synchronization overhead with the global rand().
  uintptr_t rng_state_ TA_GUARDED(lock_) = 0;

  // Used to signal the compression thread if the list_ goes from empty->non-empty.
  AutounsignalEvent event_;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_DEBUG_COMPRESSOR_H_
