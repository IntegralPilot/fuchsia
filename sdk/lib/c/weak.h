// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_WEAK_H_
#define LIB_C_WEAK_H_

// This provides some conveniences for code using weak undefined symbols.

#include <zircon/compiler.h>

#include <concepts>
#include <type_traits>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// Implementation helper for Weak::Call so it gets a precise signature for
// function types instead of doing template-forwarding.  That way the arguments
// get coerced (or diagnosed) at the real call site, not inside Weak::Call.
template <typename T, T* Symbol>
struct WeakCall {
  // The generic template will only be used for non-functions.  If they're not
  // actually callable, then it will be an error to instantiate a call.
  template <typename... Args>
  constexpr void operator()(Args&&... args) const {
    if (Symbol != nullptr) {
      Symbol(std::forward<Args>(args)...);
    }
  }
};

// This partial specialization kicks in for function types to make the call
// operator (and thus Weak::Call) have non-templated argument types.
template <typename R, typename... Args, R (*Symbol)(Args...)>
struct WeakCall<R(Args...), Symbol> {
  using Type = R(Args...);

  constexpr void operator()(Args... args) const
    requires(std::is_void_v<R>)
  {
    if (Symbol != nullptr) {
      Symbol(std::forward<Args>(args)...);
    }
  }

  template <Type* Other>
  struct FallbackTo {
    constexpr R operator()(Args... args) const {
      return (Symbol ? Symbol : Other)(std::forward<Args>(args)...);
    }
  };
};

// This facilitates use of a hook interface that libc is a client of.  Each
// public symbol (function or variable) is declared as extern in some public or
// quasi-public header shared with code that defines the symbol.  Those public
// declarations don't use [[gnu::weak]] because definers of the symbols should
// not define them as weak.  Instead, libc-private code that is going to use
// such a symbol does:
// ```
// extern [[gnu::weak]] decltype(__hook_name) __hook_name;
// ```
// Then it can use `Weak<&__hook_name>::...` to make use of the symbol, which
// might or might not actually be defined at runtime.  The `&` can be elided
// for a function symbol, since they're implicitly converted to function
// pointer the template parameter must be a pointer (to a named entity).
template <auto* Symbol>
struct Weak {
  using Type = std::remove_pointer_t<decltype(Symbol)>;

  // Weak<Symbol>::Call(...) just calls Symbol(...) or nothing.
  static constexpr WeakCall<Type, Symbol> Call{};

  // Weak<Symbol>::Fallback<Other>(...) calls Symbol(...) or Other(...).
  template <Type* Other>
    requires(std::is_function_v<Type>)
  static constexpr WeakCall<Type, Symbol>::template FallbackTo<Other> Fallback{};

  // Weak<Symbol>::Or{value}(...) calls Symbol(...) or returns value.  If it's
  // a variable symbol rather than a function symbol, it just returns (copies)
  // *Symbol itself or returns value.
  template <typename T>
  class Or {
   public:
    template <typename... Args>
    constexpr explicit Or(Args&&... args) : value_(std::forward<Args>(args)...) {}

    template <typename... Args>
      requires std::invocable<Type*, Args...> &&
               std::convertible_to<T, std::invoke_result_t<Type*, Args...>>
    std::invoke_result_t<Type*, Args...> operator()(Args&&... args) && {
      if (Symbol) {
        return Symbol(std::forward<Args>(args)...);
      }
      return std::move(value_);
    }

    template <typename U = T>
      requires(std::is_same_v<Type, U> && !std::is_function_v<U>)
    explicit(false) operator U() && {
      if (Symbol != nullptr) {
        return *Symbol;
      }
      return std::move(value_);
    }

   private:
    T value_;
  };

  template <typename T>
  Or(T) -> Or<T>;
};

// This meets the C++ BasicLockable named requirement with a pair of void()
// functions that constitute one lock, and can both be weak undefined symbols.
// Despite the name, this is also fine to use with symbols that haven't been
// declared weak.  The Weak::Call machinery should compile away to the trivial
// tail calls since the comparison will be considered statically tautological.
//
// WeakLock instantiations are empty objects, so there only needs to be one.
// Each one can be declared and used somewhere as:
// ```
// constexpr WeakLock<...> kSomeLock;
// T gSomeLockedThing __TA_GUARDED(kSomeLock);
// ...
//   std::lock_guard lock(kSomeLock);
//   gSomeLockedThing.DoThingWhileLocked();
// ```
// This can be used directly in ``.
template <void (*Lock)(), void (*Unlock)()>
struct __TA_CAPABILITY("Lockable") WeakLock {
  void lock() const __TA_ACQUIRE() { Weak<Lock>::Call(); }
  void unlock() const __TA_RELEASE() { Weak<Unlock>::Call(); }
};

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_WEAK_H_
