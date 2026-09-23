#ifndef ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_LIBC_H_
#define ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_LIBC_H_

#include <limits.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#ifdef __cplusplus
#include <atomic>
using std::atomic_int;
#else
#include <stdatomic.h>
#endif

__BEGIN_CDECLS

struct __locale_map;

struct __locale_struct {
  const struct __locale_map* volatile cat[6];
};

struct tls_module {
  struct tls_module* next;
  void* image;
  size_t len, size, align, offset;
};

struct tls_layout {
  size_t size, align;
  struct tls_module* tls_head;
  size_t tls_cnt;
};

struct __libc {
  atomic_int thread_count;
  struct __locale_struct global_locale;
};

#ifdef __PIC__
#define ATTR_LIBC_VISIBILITY __attribute__((visibility("hidden")))
#else
#define ATTR_LIBC_VISIBILITY
#endif

// Put this on things that are touched only during dynamic linker startup.
#define ATTR_RELRO __SECTION(".data.rel.ro")

extern struct __libc __libc ATTR_LIBC_VISIBILITY;
#define libc __libc

extern size_t __hwcap ATTR_LIBC_VISIBILITY;

// TODO(49419) These exist for legacy err.h support, which we will be removing.
extern char* __progname ATTR_LIBC_VISIBILITY;
extern char* __progname_full ATTR_LIBC_VISIBILITY;

void __libc_init_gwp_asan(void) ATTR_LIBC_VISIBILITY;
void __libc_start_init(void) ATTR_LIBC_VISIBILITY;

void __funcs_on_exit(void) ATTR_LIBC_VISIBILITY;
void __funcs_on_quick_exit(void) ATTR_LIBC_VISIBILITY;
void __libc_exit_fini(void) ATTR_LIBC_VISIBILITY;

void __dl_thread_cleanup(void) ATTR_LIBC_VISIBILITY;

void __tls_run_dtors(void) ATTR_LIBC_VISIBILITY;

struct tls_layout _dl_tls_layout(void) ATTR_LIBC_VISIBILITY;
size_t _dl_stack_size(void) ATTR_LIBC_VISIBILITY;

// Registers the handles that zx_take_startup_handle() will return.
//
// This function takes ownership of the data, but not the memory: it assumes
// that the arrays are valid as long as the process is alive.
//
// |handles| and |handle_info| are parallel arrays and must have |nhandles|
//     entries.
// |handles| contains the actual handle values, or ZX_HANDLE_INVALID if a
//     handle has already been claimed.
// |handle_info| contains the PA_HND value associated with the
//     corresponding element of |handles|, or zero if the handle has already
//     been claimed.
void __libc_startup_handles_init(uint32_t nhandles, zx_handle_t handles[],
                                 uint32_t handle_info[]) ATTR_LIBC_VISIBILITY;

// Hook for extension libraries to init. Extensions must zero out
// handle[i] and handle_info[i] for any handles they claim.
void __libc_extensions_init(uint32_t handle_count, zx_handle_t handle[], uint32_t handle_info[],
                            uint32_t name_count, char** names) __attribute__((weak));

// Hook for extension libraries to clean up. This is run after exit
// and quick_exit handlers.
void __libc_extensions_fini(void) __attribute__((weak));

int __lockfile(FILE*) ATTR_LIBC_VISIBILITY;
void __unlockfile(FILE*) ATTR_LIBC_VISIBILITY;

// Hook for extension libraries to return the maximum number of files that
// a process can have open at any time. Used to answer sysconf(_SC_OPEN_MAX).
// Returns -1 if the value is unknown.
int _fd_open_max(void);

// Hook for extension libraries to provide a context associated to a given fd.
// Operation on the context must be available until it has been release, even if
// the fd is then closed.
void* _fd_get_context(int fd);
void _fd_release_context(void* context);

extern char** __environ;

#undef weak_alias
#define weak_alias(old, new) extern __typeof(old) new __attribute__((weak, alias(#old)))

#undef strong_alias
#define strong_alias(old, new) extern __typeof(old) new __attribute__((alias(#old)))

#ifdef __clang__
#define NO_ASAN __attribute__((no_sanitize("address", "hwaddress")))
#else
#define NO_ASAN
#endif

// Indicate the given function should not use LLVM's stack hardening features,
// but instead put all local variables on the standard stack. Additionally, the
// function should not be sanitized with HWASan. This particular combination is
// significant because libc currently does checks via calls into the hwasan
// runtime which is instrumented with these stack features. Making a runtime
// call in a libc function invoked after the shadow call stack is deallocated
// can result in a fault, so those functions which would be marked with
// NO_SAFESTACK should also take care not to make a libcall into the hwasan
// runtime.
#ifdef __clang__
#define LIBC_NO_SAFESTACK \
  __attribute__((no_sanitize("safe-stack", "shadow-call-stack", "hwaddress")))
#else
#define LIBC_NO_SAFESTACK
#endif

#define STRICT_BYTE_ACCESS \
  (!__has_feature(address_sanitizer) && !__has_feature(hwaddress_sanitizer))

__END_CDECLS

#endif  // ZIRCON_THIRD_PARTY_ULIB_MUSL_SRC_INTERNAL_LIBC_H_
