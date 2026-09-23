// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_STDIO_PRINTF_CORE_WRAPPER_H_
#define LIB_C_STDIO_PRINTF_CORE_WRAPPER_H_

#include <cstdarg>
#include <span>
#include <string_view>
#include <type_traits>

namespace LIBC_NAMESPACE::printf_core {

// This provides a handy generic wrapper for using the printf core with an
// arbitrary int(std::string_view) callable object.

enum class PrintfNewline : bool { kNo, kYes };

int PrintfImpl(int (*write)(std::string_view str, void* hook), void* hook, std::span<char> buffer,
               PrintfNewline newline, const char* format, va_list args);

template <size_t BufferSize, PrintfNewline AddNewline = PrintfNewline::kNo, typename T>
inline int Printf(T&& write, const char* format, va_list args) {
  static_assert(std::is_invocable_r_v<int, T, std::string_view>);

  struct Wrapper {
    using Pointable = std::remove_reference_t<T>;
    using Pointee = std::remove_pointer_t<Pointable>;

    void* Erase() {
      if constexpr (std::is_pointer_v<T>) {
        // It's already a pointer, and now we have the pointer to that pointer.
        if constexpr (std::is_function_v<Pointee>) {
          // Can't cast a function pointer to an object pointer.
          // So use the pointer this wrapper containing the function pointer.
          return static_cast<void*>(this);
        } else if constexpr (std::is_const_v<Pointee>) {
          return const_cast<void*>(static_cast<const void*>(value));
        } else {
          return static_cast<void*>(value);
        }
      } else if constexpr (std::is_const_v<Pointable>) {
        return const_cast<void*>(static_cast<const void*>(&value));
      } else if constexpr (std::is_function_v<Pointable>) {
        // When T was a reference to function, then the wrapper really just
        // holds the function pointer but with reference type.
        return static_cast<void*>(this);
      } else {
        return static_cast<void*>(&value);
      }
    }

    static decltype(auto) Unerase(void* ptr) {
      if constexpr (std::is_pointer_v<T>) {
        // It's already a pointer.
        if constexpr (std::is_function_v<Pointee>) {
          return static_cast<Wrapper*>(ptr)->value;
        } else {
          return static_cast<T>(ptr);
        }
      } else if constexpr (std::is_function_v<Pointable>) {
        return static_cast<Wrapper*>(ptr)->value;
      } else {
        return *static_cast<Pointable*>(ptr);
      }
    }

    static int Call(std::string_view wrapper_str, void* hook) {
      std::string_view str{wrapper_str.data(), wrapper_str.size()};
      return Unerase(hook)(str);
    }

    T value;
  } wrapper{std::forward<T>(write)};

  char buffer[BufferSize];
  return PrintfImpl(Wrapper::Call, wrapper.Erase(), std::span{buffer}, AddNewline, format, args);
}

template <size_t BufferSize, PrintfNewline AddNewline = PrintfNewline::kNo, typename T>
[[gnu::format(printf, 2, 3)]] inline int Printf(T&& write, const char* format, ...) {
  va_list args;
  va_start(args, format);
  int result = Printf<BufferSize, AddNewline>(std::forward<T>(write), format, args);
  va_end(args);
  return result;
}

// This returns a move-only lambda the argument is moved into.
// Calling it with ... calls Printf(write, ...).
template <size_t BufferSize, PrintfNewline AddNewline = PrintfNewline::kNo, typename T>
constexpr auto MakePrintf(T write) {
  return [write = std::move(write)](const char* format, auto... args) mutable {
    return Printf<BufferSize, AddNewline>(write, format, args...);
  };
}

}  // namespace LIBC_NAMESPACE::printf_core

#endif  // LIB_C_STDIO_PRINTF_CORE_WRAPPER_H_
