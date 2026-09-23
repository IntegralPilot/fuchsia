// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_LIBC_INCLUDE_STDIO_H_
#define ZIRCON_KERNEL_LIB_LIBC_INCLUDE_STDIO_H_

#include <stdarg.h>
#include <stddef.h>
#include <zircon/compiler.h>

// All anybody really wants from stdio is printf.

#ifdef __cplusplus
#include <ktl/concepts.h>
#include <ktl/string_view.h>
#include <ktl/type_traits.h>

class FILE {
 public:
  // This is basically equivalent to having a virtual Write function with
  // subclasses providing their own data members in lieu of ptr.  But it's
  // simpler and avoids a vtable that might need address fixup at load time
  // (and the double indirection for a single-entry vtable--at the cost of
  // double indirection for the ptr data in a callback that uses it).

  using Callback = int(void*, const char*, size_t);

  FILE() = default;

  constexpr FILE(Callback* write, void* ptr) : write_(write), ptr_(ptr) {}

  explicit FILE(int (*write)(ktl::string_view str))
      : FILE(
            [](void* ptr, const char* str, size_t len) {
              return reinterpret_cast<int (*)(ktl::string_view)>(ptr)({str, len});
            },
            reinterpret_cast<void*>(write)) {}

  template <class T>
    requires requires(T* writer, ktl::string_view str) {
      { writer->Write(str) } -> ktl::convertible_to<int>;
    }
  explicit FILE(T* writer)
      : FILE([](void* ptr, const char* str,
                size_t len) { return static_cast<T*>(ptr)->Write({str, len}); },
             writer) {}

  // This is what fprintf calls to do output.
  int Write(ktl::string_view s) { return write_(ptr_, s.data(), s.size()); }

  constexpr explicit operator bool() const { return write_; }

  constexpr bool operator==(const FILE& other) const {
    return write_ == other.write_ && ptr_ == other.ptr_;
  }

  constexpr bool operator!=(const FILE& other) const { return !(*this == other); }

 private:
  Callback* write_ = nullptr;
  void* ptr_ = nullptr;
};
// FILE has a standard layout that stays compatible with C and thus #[repr(C)].
static_assert(ktl::is_standard_layout_v<FILE>);

// This is not defined by libc itself.  The kernel defines it to point at
// the default console output mechanism.
extern FILE gStdout;
#define stdout (&gStdout)

// Shorthands for printing ktl::string_view via printf family functions.
// Note FMT_ARG_SV evaluates its argument twice!
#define FMT_SV ".*s"
#define FMT_ARG_SV(sv) static_cast<int>((sv).size()), (sv).data()

#else  // !__cplusplus

// C users just need the function declarations.
typedef struct _FILE_is_opaque FILE;

#endif  // __cplusplus

__BEGIN_CDECLS

int fputc(int c, FILE* f);
int putc(int c, FILE* f);
int putchar(int c);

int fputs(const char* s, FILE* f);
int puts(const char* s);
size_t fwrite(const void* buf, size_t size, size_t n, FILE * f);

int printf(const char*, ...) __PRINTFLIKE(1, 2);
int fprintf(FILE*, const char*, ...) __PRINTFLIKE(2, 3);
int snprintf(char* buf, size_t len, const char*, ...) __PRINTFLIKE(3, 4);

int vprintf(const char*, va_list);
int vfprintf(FILE*, const char*, va_list);
int vsnprintf(char* buf, size_t len, const char*, va_list);

__END_CDECLS

#if DISABLE_DEBUG_OUTPUT
// The declarations stand so these can be used without parens to get
// the real functions (e.g. &printf or (printf)(...)).
#define printf(...)
#define vprintf(fmt, args)
#endif

#endif  // ZIRCON_KERNEL_LIB_LIBC_INCLUDE_STDIO_H_
