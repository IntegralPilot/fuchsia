// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_TRACE_BASE_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_TRACE_BASE_H_

#include <lib/fxt/argument.h>
#include <lib/fxt/interned_category.h>
#include <lib/fxt/interned_string.h>
#include <lib/fxt/map_macro.h>
#include <zircon/compiler.h>

#include <optional>
#include <tuple>
#include <type_traits>
#include <utility>

//
// Base utilities and macros for integrating FXT tracing with a host tracing environment (e.g. the
// kernel or userspace).
//

namespace fxt {

// RAII type and utilities for representing a scope to trace with an FXT complete duration event.
//
// The macros FXT_BEGIN_SCOPE and FXT_END_SCOPE construct delegates that implement conditional
// argument capture and integrate with the host tracing environment.
//
// Example raw usage:
//
//  void Foo(int number, const char* string) {
//    fxt::Scope scope = FXT_BEGIN_SCOPE(tracing::kTracingEnabled, runtime_condition,
//                                       tracing::CategoryEnabled, tracing::GetTimestamp,
//                                       tracing::EmitCompleteEvent, "category",
//                                       FXT_INTERN_STRING("label"), ("number", number), ("string",
//                                       string));
//
//    const int result = DoWork(number, string);
//
//    // Optional explicit end with additional arguments.
//    scope = FXT_END_SCOPE(("result", result));
//  }
//
// While fxt::Scope is intended to be user facing (perhaps imported into the tracing library
// namespace), FXT_BEGIN_SCOPE/FXT_END_SCOPE are intended to be wrapped in macros that provide the
// integration parameters for the host tracing environment.
//
// Example integrated usage:
//
//  void Foo(int number, const char* string) {
//    tracing::Scope scope =
//        TRACING_BEGIN_SCOPE("category", "label", ("number", number), ("string", string));
//
//    const int result = DoWork(number, string);
//
//    // Optional explicit end with additional arguments.
//    scope = TRACING_END_SCOPE(("result", result));
//  }
//

// Wrapper types to distinguish between delegates when beginning and ending a Scope. The delegates
// are lambda expressions created by the FXT_BEGIN_SCOPE and FXT_END_SCOPE macros.
template <typename Capture>
struct ScopeBegin {
  Capture capture_delegate;
};
template <typename Capture>
ScopeBegin(Capture&&) -> ScopeBegin<Capture>;

template <typename Capture>
struct ScopeEnd {
  Capture capture_delegate;
};
template <typename Capture>
ScopeEnd(Capture&&) -> ScopeEnd<Capture>;

// RAII type representing a scope to trace with an FXT complete duration event. Stores and executes
// delegates created by FXT_BEGIN_SCOPE and FXT_END_SCOPE to capture arguments and emit trace events
// when tracing is enabled.
template <typename Delegate>
class [[nodiscard]] Scope {
 public:
  // Constructs a Scope with the given capture delegate. The delegate should be created by
  // FXT_BEGIN_SCOPE(...) or a suitable wrapper macro.
  template <typename Capture>
  constexpr Scope(ScopeBegin<Capture>&& begin) : delegate_{begin.capture_delegate()} {}

  // The Scope ends automatically if it is not explicitly ended earlier.
  ~Scope() { End(); }

  // No copy or move.
  Scope(const Scope&) = delete;
  Scope& operator=(const Scope&) = delete;
  Scope(Scope&&) = delete;
  Scope& operator=(Scope&&) = delete;

  // Ends the Scope early with the given capture delegate to include additional arguments. The
  // delegate should be created by FXT_END_SCOPE(...) or a suitable wrapper macro.
  // TODO(https://fxbug.dev/42071611): Get consensus on ergonomic syntax for ending a scope with arguments.
  template <typename Capture>
  constexpr Scope& operator=(ScopeEnd<Capture>&& end) {
    if (delegate_.has_value()) {
      (*delegate_)(end.capture_delegate());
      delegate_.reset();
    }
    return *this;
  }

  // Ends the Scope early without additional arguments.
  constexpr void End() {
    *this = ScopeEnd{[] { return [] { return std::tuple<>{}; }; }};
  }

  // Returns a callable to end this Scope. This is useful to delegate ending the Scope to a callee
  // within same lexical scope. The lifetime of this instance must not end before the completer is
  // invoked.
  auto Completer() {
    return [this] { End(); };
  }

 private:
  Delegate delegate_;
};

template <typename T>
Scope(ScopeBegin<T>&&) -> Scope<std::invoke_result_t<T>>;

}  // namespace fxt

// Creates a delegate to conditionally capture the given arguments at the beginning of a scope to
// trace with an FXT complete event when tracing is enabled, injecting the given host integration
// functions to interface with the tracing implementation.
//
// Arguments:
// - constexpr_enabled: Boolean constexpr expression to enable/disable the trace at compile time.
// - is_category_enabled: Boolean runtime predicate to determine if the given category is enabled in
//   the host tracing system. Must accept a single argument of type const fxt::InternedCategory&.
// - get_timestamp: Function returning the current timestamp in the host tracing system.
// - emit_complete: Function to emit the FXT complete event in the host tracing system. Must accept
//   arguments compatible with the following signature:
//
//   emit_complete(const fxt::InternedCategory& category,
//                 fxt::StringRef label,
//                 uint64_t start_timestamp,
//                 uint64_t end_timestamp,
//                 T pass_through,
//                 std::tuple<fxt::Argument...> args);
//
// - runtime_condition: Boolean runtime expression to check before the category check to determine
//   whether the trace point is enabled.
// - category_literal: String literal tracing category for the FXT complete event. This string
//   literal is internally transformed into `category_literal _category`, which evaluates to an
//   interned category reference of type const fxt::InternedCategory&.
// - label: Label for the FXT complete event. Must be convertible to fxt::StringRef.
// - pass_through: Arbitrary value passed to emit_complete immediately preceding the variable args
//   tuple. This value evaluated at the end of the scope when the trace event is emitted.
// - ...: Variable list of trace arguments to include in the FXT complete event. Must be a list of
//   parenthesized ("name", values...) tuples. Each tuple is internally transformed into an argument
//   expression equivalent to: fxt::Argument{"name"_intern, values...};
//
#define FXT_BEGIN_SCOPE(constexpr_enabled, runtime_enabled, is_category_enabled, get_timestamp, \
                        emit_complete, category_literal, label, pass_through, ...)              \
  fxt::ScopeBegin {                                                                             \
    [&] {                                                                                       \
      if constexpr (constexpr_enabled) {                                                        \
        using fxt::operator""_intern;                                                           \
        using fxt::operator""_category;                                                         \
        auto capture = [&] {                                                                    \
          return [start_timestamp = get_timestamp(),                                            \
                  args = std::make_tuple(FXT_MAP_LIST_ARGS(FXT_MAKE_ARGUMENT, ##__VA_ARGS__))]( \
                     auto capture_end_args) {                                                   \
            emit_complete(FXT_INTERN_CATEGORY(category_literal), fxt::StringRef{label},         \
                          start_timestamp, get_timestamp(), pass_through,                       \
                          std::tuple_cat(args, capture_end_args()));                            \
          };                                                                                    \
        };                                                                                      \
        using Invokable = std::invoke_result_t<decltype(capture)>;                              \
        if ((runtime_enabled) &&                                                                \
            unlikely(is_category_enabled(FXT_INTERN_CATEGORY(category_literal)))) {             \
          return std::optional<Invokable>{capture()};                                           \
        }                                                                                       \
        return std::optional<Invokable>{};                                                      \
      } else {                                                                                  \
        return std::make_optional([](auto) {});                                                 \
      }                                                                                         \
    }                                                                                           \
  }

// Creates a delegate to conditionally capture the given arguments at the end of a scope to trace
// with an FXT complete event when tracing is enabled.
//
// Receives a variable list of trace arguments to include in the FXT complete event. Must be a list
// of parenthesized ("name", values...) tuples. Each tuple is internally transformed into an
// argument expression equivalent to: fxt::Argument{"name"_intern, values...};
//
#define FXT_END_SCOPE(...)                                                                   \
  fxt::ScopeEnd {                                                                            \
    [&] {                                                                                    \
      return [args = std::make_tuple(FXT_MAP_LIST_ARGS(FXT_MAKE_ARGUMENT, ##__VA_ARGS__))] { \
        return args;                                                                         \
      };                                                                                     \
    }                                                                                        \
  }

// Utility to access values with Clang static annotations in a scoped trace event argument list.
//
// Due to the way arguments are captured by lambdas when using FXT_BEGIN_SCOPE and FXT_END_SCOPE, it
// is necessary to hoist static assertions or lock guards into the argument capture expressions
// using FXT_ANNOTATED_VALUE.
//
// Examples:
//
// - Lock is already held and it is safe to read the guarded value:
//
//  fxt::Scope scope =
//      TRACE_BEGIN_SCOPE("category", "label",
//          ("guarded_value", FXT_ANNOTATED_VALUE(AssertHeld(lock), guarded_value)));
//
// - Lock is not held and must be acquired to read the guarded value:
//
//  fxt::Scope scope =
//      TRACE_BEGIN_SCOPE("category", "label",
//          ("guarded_value", FXT_ANNOTATED_VALUE(Guard guard{lock}, guarded_value)));
//
#define FXT_ANNOTATED_VALUE(acquire_or_assert_expression, value_expression) \
  [&] {                                                                     \
    acquire_or_assert_expression;                                           \
    return value_expression;                                                \
  }()

// Builds a trace point to emit an FXT kernel object record with the given required and variable
// arguments.
//
// Arguments:
// - constexpr_enabled: Boolean constexpr expression to enable/disable the trace at compile time.
// - is_category_enabled: Boolean runtime predicate to determine if the given category is enabled in
//   the host tracing system. Must accept a single argument of type const fxt::InternedCategory&.
// - emit_kernel_object: Function to emit the FXT kernel object record in the host tracing system.
//   Must accept arguments compatible with the following signature:
//
//   emit_kernel_object(const fxt::InternedCategory& category,
//                      zx_koid_t koid,
//                      zx_obj_type_t obj_type,
//                      fxt::StringRef name,
//                      std::tuple<fxt::Argument...> args);
//
// - category_literal: String literal tracing category for the FXT kernel object record. This string
//   literal is internally transformed into `category_literal _category`, which evaluates to an
//   interned category reference of type const fxt::InternedCategory&.
// - koid: zx_koid_t value representing the KOID of the kernel object.
// - obj_type: zx_obj_type_t value representing the type of the kernel object.
// - name: Name of the kernel object. Must be convertible to fxt::StringRef.
// - ...: Variable list of trace arguments to include in the FXT kernel object record. Must be a
//   list of parenthesized ("name", values...) tuples. Each tuple is internally transformed into an
//   argument expression equivalent to: fxt::Argument{"name"_intern, values...};
//
#define FXT_KERNEL_OBJECT(constexpr_enabled, is_category_enabled, emit_kernel_object,             \
                          category_literal, koid, obj_type, name, ...)                            \
  do {                                                                                            \
    if constexpr (constexpr_enabled) {                                                            \
      using fxt::operator""_intern;                                                               \
      using fxt::operator""_category;                                                             \
      if (unlikely(is_category_enabled(FXT_INTERN_CATEGORY(category_literal)))) {                 \
        emit_kernel_object(koid, obj_type, fxt::StringRef{name},                                  \
                           std::make_tuple(FXT_MAP_LIST_ARGS(FXT_MAKE_ARGUMENT, ##__VA_ARGS__))); \
      }                                                                                           \
    }                                                                                             \
  } while (false)

// Similar to FXT_KERNEL_OBJECT but does not use a category to control when the FXT kernel object
// record is emitted. Used to emit thread and process names at the start of a tracing session before
// tracing is enabled.
#define FXT_KERNEL_OBJECT_ALWAYS(constexpr_enabled, emit_kernel_object, koid, obj_type, name, ...) \
  do {                                                                                             \
    if constexpr (constexpr_enabled) {                                                             \
      using fxt::operator""_intern;                                                                \
      emit_kernel_object(koid, obj_type, fxt::StringRef{name},                                     \
                         std::make_tuple(FXT_MAP_LIST_ARGS(FXT_MAKE_ARGUMENT, ##__VA_ARGS__)));    \
    }                                                                                              \
  } while (false)

// Builds a trace point to emit a FXT event record with the given required and variable arguments.
//
// Arguments:
// - constexpr_enabled: Boolean constexpr expression to enable/disable the trace at compile time.
// - is_category_enabled: Boolean runtime predicate to determine if the given category is enabled in
//   the host tracing system. Must accept a single argument of type const fxt::InternedCategory&.
// - emit_event: Function to emit the desired FXT event record in the host tracing system.  Must
//   accept arguments compatible with the following signature:
//
//   emit_event(const fxt::InternedCategory& category,
//              const fxt::InternedString& label,
//              T0 pass_through0,
//              T1 pass_through1,
//              T2 pass_through2,
//              std::tuple<fxt::Argument...> args);
//
// - category_literal: String literal tracing category for the FXT event record. This string literal
//   is internally transformed into `category_literal _category`, which evaluates to an interned
//   category reference of type const fxt::InternedCategory&.
// - label: Label for the FXT event record. Must be convertible to fxt::StringRef.
// - pass_through0: First pass through value. May be a sentinel if unused.
// - pass_through1: Second pass through value. May be a sentinel if unused.
// - pass_through2: Third pass through value. May be a sentinel if unused.
// - ...: Variable list of trace arguments to include in the FXT kernel object record. Must be a
//   list of parenthesized ("name", values...) tuples. Each tuple is internally transformed into an
//   argument expression equivalent to: fxt::Argument{"name"_intern, values...};
//
#define FXT_EVENT_COMMON(constexpr_enabled, is_category_enabled, emit_event, category_literal,  \
                         label, pass_through0, pass_through1, pass_through2, ...)               \
  do {                                                                                          \
    if constexpr (constexpr_enabled) {                                                          \
      using fxt::operator""_intern;                                                             \
      using fxt::operator""_category;                                                           \
      if (unlikely(is_category_enabled(FXT_INTERN_CATEGORY(category_literal)))) {               \
        emit_event(FXT_INTERN_CATEGORY(category_literal), fxt::StringRef{label}, pass_through0, \
                   pass_through1, pass_through2,                                                \
                   std::make_tuple(FXT_MAP_LIST_ARGS(FXT_MAKE_ARGUMENT, ##__VA_ARGS__)));       \
      }                                                                                         \
    }                                                                                           \
  } while (false)

// Concatenates the given symbols.
#define FXT_CONCATENATE(a, b) a##b

// Internalizes the given string literal using fxt::operator""_intern.
#define FXT_INTERN_STRING(string_literal) FXT_CONCATENATE(string_literal, _intern)

// Internalizes the given string literal using fxt::operator""_category.
#define FXT_INTERN_CATEGORY(string_literal) FXT_CONCATENATE(string_literal, _category)

// Builds an instance of fxt::Argument from the given string literal and variable arguments,
// deducing the template parameters of fxt::Argument from the variable arguments.
#define FXT_MAKE_ARGUMENT(label_literal, ...) \
  fxt::MakeArgument(FXT_INTERN_STRING(label_literal), ##__VA_ARGS__)

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_TRACE_BASE_H_
