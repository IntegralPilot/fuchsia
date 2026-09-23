// -*- C++ -*-
#ifndef ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_THREADS_IMPL_H_
#define ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_THREADS_IMPL_H_

#include <assert.h>
#include <errno.h>
#include <lib/zircon-internal/unique-backtrace.h>
#include <limits.h>
#include <locale.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/uio.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/threads.h>
#include <zircon/tls.h>
#include <zircon/types.h>

#include "libc.h"
#include "pthread_arch.h"
#include "threads/zxr-tls.h"

#ifdef __cplusplus
#include <lib/zx/thread.h>

#include <atomic>
#include <optional>
#endif

__BEGIN_CDECLS

#define pthread __pthread

// This is what the thread pointer points to directly.  On TLS_ABOVE_TP
// machines, the size of this is part of the ABI known to the compiler
// and linker.
typedef struct {
  // The position of this pointer is part of the ABI on x86.
  // It has the same value as the thread pointer itself.
  uintptr_t tp;
  void** dtv;
} tcbhead_t;

// The locations of these fields is part of the ABI known to the compiler.
typedef struct {
  uintptr_t stack_guard;
  uintptr_t unsafe_sp;
} tp_abi_t;

struct tls_dtor;

// Note this is distinct from `__has_feature(shadow_call_stack)`!  That
// indicates that the library code is currently being compiled to use the
// shadow call stack.  This indicates that the library should support the
// shadow call stack ABI so that other code might use it.  This is an
// aspect of the Fuchsia ABI for the machine.  That is an implementation
// detail of a particular build of the C library code.
#ifdef __x86_64__
#define HAVE_SHADOW_CALL_STACK 0
#define HAVE_UNSAFE_STACK 1
#else
#define HAVE_SHADOW_CALL_STACK 1
#define HAVE_UNSAFE_STACK 0
#endif

struct pthread {
#ifdef __cplusplus
  // A thread starts JOINABLE, or DETACHED via pthread_attr_setdetachstate.
  //  * If someone calls ThreadJoin on it, it transitions to JOINED.
  //  * If someone calls ThreadDetach on it, it transitions to DETACHED.
  //  * When it begins exiting, the EXITING state is entered.
  //  * When it is no longer using its memory and handle resources, it
  //   transitions to DONE.  If the thread was DETACHED prior to EXITING, this
  //   transition MAY not happen.
  // No other transitions occur.
  enum class Lifecycle : int {
    JOINABLE,
    DETACHED,
    JOINED,
    EXITING,
    DONE,
    FREED,
  };

  zx_futex_t* LifecycleFutex() { return reinterpret_cast<zx_futex_t*>(&lifecycle_); }

  // Claim the thread as JOINED or DETACHED.  Returns std::nullopt on success,
  // which only happens if the previous state was JOINABLE.  On failure, it
  // returns the actual previous state.
  std::optional<Lifecycle> JoinOrDetachLifecycle(Lifecycle new_lifecycle) {
    if (Lifecycle old_lifecycle = Lifecycle::JOINABLE; !lifecycle_.compare_exchange_strong(
            old_lifecycle, new_lifecycle, std::memory_order_acq_rel, std::memory_order_acquire))
        [[unlikely]] {
      return old_lifecycle;
    }
    return std::nullopt;
  }

  // Extract the thread handle.  Synchronizes with readers by setting the state
  // to FREED and checks the given expected state for consistency.
  zx::thread TakeHandle(Lifecycle expected_lifecycle) {
    zx::thread taken{std::exchange(handle_, ZX_HANDLE_INVALID)};
    if (!lifecycle_.compare_exchange_strong(expected_lifecycle, Lifecycle::FREED,
                                            std::memory_order_acq_rel, std::memory_order_acquire)) {
      CRASH_WITH_UNIQUE_BACKTRACE();
    }
    return taken;
  }
#endif

#ifndef TLS_ABOVE_TP
  // These must be the very first members.
  tcbhead_t head;
  tp_abi_t abi;
#endif

  zx_handle_t handle_;
#ifdef __cplusplus
  std::atomic<Lifecycle> lifecycle_;
#else
  _Atomic(int) lifecycle_;
#endif

  struct pthread* next;
  struct pthread** prevp;

  // The _unowned_ process and VMAR handles for creating new threads.
  thrd_zx_create_handles_t create_handles;

  // These `storage_*` fields all "belong" to the ThreadStorage class.
  // Only it accesses them.
  size_t storage_stack_size, storage_guard_size, storage_thread_block_size;
  uintptr_t storage_thread_block_address, storage_machine_stack_address;
#if HAVE_UNSAFE_STACK
  uintptr_t storage_unsafe_stack_address;
#endif
#if HAVE_SHADOW_CALL_STACK
  uintptr_t storage_shadow_call_stack_address;
#endif
  // The _unowned_ handles copied from the creator's create_handles.  These
  // VMAR handles are used to unmap their respective storage blocks.  (The
  // process handle stored here is not used, but the struct is convenient and
  // the same space would be lost to alignment padding anyway.)
  thrd_zx_create_handles_t storage_handles;

  struct tls_dtor* tls_dtors;
  void* tsd[PTHREAD_KEYS_MAX];
  int tsd_used;
  int errno_value;

  uintptr_t scudo_tsd;
  uint64_t gwp_asan_tsd;

  void* sanitizer_hook;

  intptr_t join_value;

  locale_t locale;
  char* dlerror_buf;
  int dlerror_flag;

#ifdef TLS_ABOVE_TP
  // This is a misnomer in this case, since it's entirely an implementation
  // detail where the dtv pointer lives and if there is no ABI_TCBHEAD_SIZE
  // then there's no particularly natural place to put it.  Likewise, there's
  // no possibility of a presumption that *tp==tp as in TLS-below-TP machines
  // so there's no need to have that field either. But to simplify things for
  // the existing code using the `head` field, it's placed here with the same
  // name (and the unused `head.tp` slot).
#ifndef ABI_TCBHEAD_SIZE
  tcbhead_t head;
#endif  // ABI_TCBHEAD_SIZE

  // These must be the very last members.
  tp_abi_t abi;
#ifdef ABI_TCBHEAD_SIZE
  tcbhead_t head;
#endif  // ABI_TCBHEAD_SIZE
#endif  // TLS_ABOVE_TP
};

#ifdef TLS_ABOVE_TP
#ifdef ABI_TCBHEAD_SIZE
#define PTHREAD_TP_OFFSET offsetof(struct pthread, head)
#else
#define PTHREAD_TP_OFFSET sizeof(struct pthread)
#endif
#else
#define PTHREAD_TP_OFFSET 0
#endif

#define TP_OFFSETOF(field) ((ptrdiff_t)offsetof(struct pthread, field) - PTHREAD_TP_OFFSET)

#if !defined(TLS_ABOVE_TP) || defined(ABI_TCBHEAD_SIZE)
static_assert(TP_OFFSETOF(head) == 0, "ABI tcbhead_t misplaced in struct pthread");
#endif

#ifdef ABI_TCBHEAD_SIZE
static_assert(ABI_TCBHEAD_SIZE >= sizeof(tcbhead_t), "ABI_TCBHEAD_SIZE doesn't make sense");
static_assert((sizeof(struct pthread) - offsetof(struct pthread, head)) == ABI_TCBHEAD_SIZE,
              "ABI tcbhead_t misplaced in struct pthread");
#endif

#if defined(__x86_64__) || defined(__aarch64__)
// The tlsdesc.s assembly code assumes this, though it's not part of the ABI.
static_assert(TP_OFFSETOF(head.dtv) == 8, "dtv misplaced in struct pthread");
#elif defined(__riscv)
static_assert(TP_OFFSETOF(head.dtv) == -24, "dtv misplaced in struct pthread");
#endif

static_assert(TP_OFFSETOF(abi.stack_guard) == ZX_TLS_STACK_GUARD_OFFSET,
              "stack_guard not at ABI-mandated offset from thread pointer");
static_assert(TP_OFFSETOF(abi.unsafe_sp) == ZX_TLS_UNSAFE_SP_OFFSET,
              "unsafe_sp not at ABI-mandated offset from thread pointer");

LIBC_NO_SAFESTACK static inline void* pthread_to_tp(struct pthread* thread) {
  return (void*)((char*)thread + PTHREAD_TP_OFFSET);
}

static inline struct pthread* tp_to_pthread(void* tp) {
  return (struct pthread*)((char*)tp - PTHREAD_TP_OFFSET);
}

#define SIGALL_SET ((sigset_t*)(const unsigned long long[2]){-1, -1})

#define PTHREAD_MUTEX_TYPE_MASK (PTHREAD_MUTEX_RECURSIVE | PTHREAD_MUTEX_ERRORCHECK)
#define PTHREAD_MUTEX_TYPE_SHIFT (0u)

#define PTHREAD_MUTEX_ROBUST_MASK (PTHREAD_MUTEX_ROBUST)
#define PTHREAD_MUTEX_ROBUST_SHIFT (2u)

#define PTHREAD_MUTEX_PROTOCOL_MASK (PTHREAD_PRIO_INHERIT | PTHREAD_PRIO_PROTECT)
#define PTHREAD_MUTEX_PROTOCOL_SHIFT (3u)

#define PTHREAD_MUTEX_MAKE_ATTR(_type, _proto)                                 \
  (unsigned)(((_type & PTHREAD_MUTEX_TYPE_MASK) << PTHREAD_MUTEX_TYPE_SHIFT) | \
             ((_proto & PTHREAD_MUTEX_PROTOCOL_MASK) << PTHREAD_MUTEX_PROTOCOL_SHIFT))

static_assert(((PTHREAD_MUTEX_TYPE_MASK << PTHREAD_MUTEX_TYPE_SHIFT) &
               (PTHREAD_MUTEX_ROBUST_MASK << PTHREAD_MUTEX_ROBUST_SHIFT)) == 0,
              "pthread_mutex type attr overlaps with robust attr!");
static_assert(((PTHREAD_MUTEX_TYPE_MASK << PTHREAD_MUTEX_TYPE_SHIFT) &
               (PTHREAD_MUTEX_PROTOCOL_MASK << PTHREAD_MUTEX_PROTOCOL_SHIFT)) == 0,
              "pthread_mutex type attr overlaps with protocol attr!");
static_assert(((PTHREAD_MUTEX_ROBUST_MASK << PTHREAD_MUTEX_ROBUST_SHIFT) &
               (PTHREAD_MUTEX_PROTOCOL_MASK << PTHREAD_MUTEX_PROTOCOL_SHIFT)) == 0,
              "pthread_mutex robust attr overlaps with protocol attr!");

static inline int pthread_mutex_get_type(pthread_mutex_t* m) {
  return (m->_m_attr >> PTHREAD_MUTEX_TYPE_SHIFT) & PTHREAD_MUTEX_TYPE_MASK;
}

static inline int pthread_mutex_get_robust(pthread_mutex_t* m) {
  return (m->_m_attr >> PTHREAD_MUTEX_ROBUST_SHIFT) & PTHREAD_MUTEX_ROBUST_MASK;
}

static inline int pthread_mutex_get_protocol(pthread_mutex_t* m) {
  return (m->_m_attr >> PTHREAD_MUTEX_PROTOCOL_SHIFT) & PTHREAD_MUTEX_PROTOCOL_MASK;
}

static inline bool pthread_mutex_prio_inherit(pthread_mutex_t* m) {
  return (m->_m_attr & (PTHREAD_PRIO_INHERIT << PTHREAD_MUTEX_PROTOCOL_MASK)) != 0;
}

// Contested state tracking bits.  Note; all users are required to use the
// static inline functions for manipulating and checking state.  This
// centralizes the operations and makes it easier to adapt code if/when the
// reserve handle bit(s) change.
//
// Note; currently valid handles are always expected to have the contested bit
// *set*.  A uncontested-and-owned mutex state is turned into a
// contested-and-owned mutex state by clearing the contested bit, not setting
// it.

#define _PTHREAD_MUTEX_CONTESTED_BIT ((int)0x00000001)
#define _PTHREAD_MUTEX_CONTESTED_MASK ((int)(~_PTHREAD_MUTEX_CONTESTED_BIT))

static inline int pthread_mutex_tid_to_uncontested_state(pid_t h) {
  // We rely on the fact that the reserved must-be-one bits are always set.
  // For now, let's incur the cost of this sanity check, but consider relaxing
  // it so that it is only performed in debug builds.
  if ((h & ZX_HANDLE_FIXED_BITS_MASK) != ZX_HANDLE_FIXED_BITS_MASK) {
    CRASH_WITH_UNIQUE_BACKTRACE();
  }
  return ((int)h);
}

static inline int pthread_mutex_tid_to_contested_state(pid_t h) {
  return ((int)(h & _PTHREAD_MUTEX_CONTESTED_MASK));
}

static inline int pthread_mutex_uncontested_to_contested_state(int state) {
  return (state & _PTHREAD_MUTEX_CONTESTED_MASK);
}

static inline pid_t pthread_mutex_state_to_tid(int state) {
  return state ? ((pid_t)(state | _PTHREAD_MUTEX_CONTESTED_BIT)) : 0;
}

static inline bool pthread_mutex_is_state_contested(int state) {
  return ((state & _PTHREAD_MUTEX_CONTESTED_BIT) == 0);
}

#undef _PTHREAD_MUTEX_CONTESTED_BIT
#undef _PTHREAD_MUTEX_CONTESTED_MASK

// Bits used by pthreads R/W locks for tracking locked vs. unlocked state, as
// well as reader count.
//
// Notes about pthreads R/W lock state...
// 1) (state == 0)               => "unlocked"
// 2) (state in [1, 0x7ffffffe]) => locked-for-read.
// 3) (state == 0x7fffffff)      => locked-for-write.
// 4) #2 and #3 above may also have the CONTESTED bit set to indicate that there
//    are waiters.
#define PTHREAD_MUTEX_RWLOCK_CONTESTED_BIT ((int)0x80000000)
#define PTHREAD_MUTEX_RWLOCK_COUNT_MASK ((int)(~PTHREAD_MUTEX_RWLOCK_CONTESTED_BIT))
#define PTHREAD_MUTEX_RWLOCK_UNLOCKED ((int)0)
#define PTHREAD_MUTEX_RWLOCK_LOCKED_FOR_WR (PTHREAD_MUTEX_RWLOCK_COUNT_MASK)
#define PTHREAD_MUTEX_RWLOCK_MAX_RD_COUNT ((int)(PTHREAD_MUTEX_RWLOCK_COUNT_MASK - 1))

extern void* __pthread_tsd_main[];
extern volatile size_t __pthread_tsd_size;

void* __tls_get_new(size_t offset, size_t modid) ATTR_LIBC_VISIBILITY;

static inline struct pthread* __pthread_self(void) {
  return tp_to_pthread(__builtin_thread_pointer());
}

static inline thrd_t __thrd_current(void) { return (thrd_t)__pthread_self(); }

static inline pid_t __thread_get_tid(void) { return (int)__pthread_self()->handle_; }

// This function maps a zx_handle_t for the thread into an int, similar to
// __thread_get_tid(). This version is used by FILE::lock to indicate that this
// thread owns the lock. In that lock structure, values < 0 (in particular -1)
// are used to signal that the FILE structure does not require locking (this is
// used for unshared structures, or rentrant calls where the FILE is already
// locked).
//
// Because zx_handle_t uses the top bits of its uint32_t, simply returning the
// uint32_t as an int32_t would erronously cause the tid to be < 0, causing the
// FILE structure to go unguarded. See https://fxbug.dev/42109323 for more detail.
//
// However, zx_handle_t reserves the bits in ZX_HANDLE_FIXED_BITS_MASK, and
// they're always set to 1. These bits happen to be the lowest two bits, and
// because we're only using this as an opaque identifier (and no longer treating
// it as a handle value), we can simply shift the valid bits of the handle down
// to avoid the sign bit being set.
//
// This function is (semi-)exposed for testing, but should only be used by
// __thread_get_tid_for_filelock().
#define _MUSL_MIN_FIXED_SHIFT 2
#define _MUSL_MIN_FIXED_MASK ((1u << _MUSL_MIN_FIXED_SHIFT) - 1)
static inline pid_t __thread_handle_to_filelock_tid(zx_handle_t handle) {
  static_assert((_MUSL_MIN_FIXED_MASK & ZX_HANDLE_FIXED_BITS_MASK) == _MUSL_MIN_FIXED_MASK,
                "Lowest 2 bits of handles are not in ZX_HANDLE_FIXED_BITS_MASK");
  return (pid_t)(handle >> 2);
}
#undef _MUSL_MIN_FIXED_SHIFT
#undef _MUSL_MIN_FIXED_MASK

static inline pid_t __thread_get_tid_for_filelock(void) {
  return __thread_handle_to_filelock_tid((int)__pthread_self()->handle_);
}

// Signal n (or all, for -1) threads on a pthread_cond_t or cnd_t.
void __private_cond_signal(void* condvar, int n) ATTR_LIBC_VISIBILITY;

// This is guaranteed to only return 0, EINVAL, or ETIMEDOUT.
int __timedwait_assign_owner(atomic_int*, int, clockid_t, const struct timespec*,
                             zx_handle_t) ATTR_LIBC_VISIBILITY;
static inline int __timedwait(atomic_int* futex, int val, clockid_t clk,
                              const struct timespec* at) {
  return __timedwait_assign_owner(futex, val, clk, at, ZX_HANDLE_INVALID);
}

// Loading a library can introduce more thread_local variables. Thread
// allocation bases bookkeeping decisions based on the current state
// of thread_locals in the program, so thread creation needs to be
// inhibited by a concurrent dlopen. This lock implements that
// exclusion.
void __thread_allocation_inhibit(void) ATTR_LIBC_VISIBILITY;
void __thread_allocation_release(void) ATTR_LIBC_VISIBILITY;

void __thread_tsd_run_dtors(void) ATTR_LIBC_VISIBILITY;

int __clock_gettime(clockid_t, struct timespec*) ATTR_LIBC_VISIBILITY;

__END_CDECLS

#endif  // ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_THREADS_IMPL_H_
