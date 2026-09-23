// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Undefined Behavior commands.
// These commands can be used to test the undefined behavior sanitizer.
// A kernel compiled with the `kubsan` variant should be able to detect
// each of them.
//
// Most of the functions use inline assembly as an attempt to avoid the compiler
// from optimizing the operations away or throwing warnings.

#include <inttypes.h>
#include <lib/boot-options/boot-options.h>
#include <lib/console.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <ktl/algorithm.h>
#include <ktl/iterator.h>
#include <ktl/span.h>
#include <ktl/string_view.h>

#include <ktl/enforce.h>

#if __has_feature(undefined_behavior_sanitizer)

namespace {

// The compiler cannot assume it knows the return value of Launder.
template <typename T>
T Launder(T x) {
  __asm__("" : "=r"(x) : "0"(x));
  return x;
}

void array_oob() {
  // Out of bounds array indexing, in cases where the array bound can be
  // statically determined
  uint32_t buf[] = {0, 1, 2};
  size_t index = Launder(3);

  printf("array read out of bounds: buf[%zu]\n", index);
  uint32_t val = buf[index];
  printf("result: %u\n", val);
}

void invalid_builtin_clz() {
  int zero = Launder(0);
  printf("__builtin_clz(0)\n");
  int result = __builtin_clz(zero);
  printf("result: %d\n", result);
}

void invalid_builtin_ctz() {
  int zero = Launder(0);
  printf("__builtin_ctz(0)\n");
  int result = __builtin_ctz(zero);
  printf("result: %d\n", result);
}

void overflow_signed_int_add() {
  // Signed integer overflow, where the result of a signed integer computation
  // cannot be represented in its type.
  int32_t x = Launder(INT32_MAX);
  int32_t y = Launder(1);

  printf("integer overflow: %d + %d\n", x, y);
  int32_t res = x + y;
  printf("result: %d\n", res);
}

[[gnu::returns_nonnull]] void* nonnull_return_helper() { return Launder<void*>(nullptr); }

void nonnull_return() {
  printf("function declared [[gnu::returns_nonnull]] returns nullptr\n");
  printf("result: %p\n", nonnull_return_helper());
}

void* _Nonnull nullability_return_helper() { return Launder<void*>(nullptr); }

void nullability_return() {
  printf("function declared `T* _Nonnull` returns nullptr\n");
  printf("result: %p\n", nullability_return_helper());
}

void overflow_signed_int_shift() {
  // Shift operators where the amount shifted is greater or equal to the
  // promoted bit-width of the left hand side or less than zero, or where the
  // left hand side is negative.

  int64_t big_val = Launder(0x1000000);
  size_t shift = Launder(50);

  printf("shift overflowed: %" PRId64 " << %zu\n", big_val, shift);
  int64_t res = big_val << shift;
  printf("result: %" PRId64 "\n", res);
}

void overflow_ptr() {
  // Performing pointer arithmetic which overflows, or where either the old or
  // new pointer value is a null pointer (or in C, when they both are).
  uint8_t local_variable = 0x01;
  uint8_t* ptr = &local_variable;
  size_t overflower = Launder(UINT64_MAX);

  printf("pointer overflow: %p + 0x%zx\n", ptr, overflower);
  uint8_t* newptr = ptr + overflower;
  printf("result: %p\n", newptr);
}

void misaligned_ptr() {
  // Use of a misaligned pointer or creation of a misaligned reference.
  uint64_t aligned = 0;
  uint32_t* addr = reinterpret_cast<uint32_t*>(Launder(reinterpret_cast<uintptr_t>(&aligned)) + 1);

  printf("misaligned pointer access: *%p\n", addr);
  uint32_t val = *addr;
  printf("result: %x\n", val);
}

void unaligned_assumption() {
  // Make a false alignment assumption on a pointer.
  uint64_t aligned = 0;
  uint32_t* addr = reinterpret_cast<uint32_t*>(Launder(reinterpret_cast<uintptr_t>(&aligned)) + 1);

  printf("assuming that %p is aligned to 256 bytes.\n", addr);
  uint32_t* __attribute__((align_value(256))) p = addr;
  printf("p: %x\n", *p);
}

void undefined_bool() {
  // Load of a bool value that is neither true nor false.
  static_assert(sizeof(uint64_t) >= sizeof(bool), "bool is larger than uint64_t");
  uint64_t garbage = Launder(uint64_t{0xdeadbeef});

  printf("loading a bool with value: %" PRIu64 "\n", garbage);

  bool val;
  memcpy(&val, &garbage, sizeof(val));

  bool b = val;
  uint64_t res = 0;
  memcpy(&res, &b, sizeof(b));

  printf("load of bad bool value: %" PRIu64 "\n", res);
}

void unreachable() {
  // Execute unreachable code.
  printf("About to execute unreachable code\n");

  // There is no version of unreachable code that can be recovered from,
  // because the compiler will always treat it as a "noreturn" path and omit
  // the epilogue of the function entirely.
  __builtin_unreachable();
}

void undefined_enum() {
  // Load of a value of an enumerated type which is not in the range of
  // representable values for that enumerated type.
  enum Stuff : uint8_t { Foo, Bar, Baz };

  uint32_t garbage = Launder(0xdeadbeef);
  printf("loading an enum with value: %" PRIu32 "\n", garbage);

  Stuff val;
  memcpy(&val, &garbage, sizeof(val));

  Stuff b = val;
  printf("load of invalid enum value: %d\n", b);
}

struct UndefinedBehaviorCommand {
  ktl::string_view name;
  void (*func)();
  const char* description;
  bool cannot_continue;
};

constexpr UndefinedBehaviorCommand kUbCommands[] = {
    {"all", nullptr, "run each subcommand in turn (requires kernel.ubsan.panic=false)"},
    {"array_oob", &array_oob, "array out of bounds access"},
    {"invalid_builtin_clz", &invalid_builtin_clz, "call __builtin_clz with 0"},
    {"invalid_builtin_ctz", &invalid_builtin_ctz, "call __builtin_ctz with 0"},
    {"misaligned_ptr", &misaligned_ptr, "use a misaligned pointer"},
    {"nonnull_return", &nonnull_return, "return nullptr from returns_nonnull function"},
    {"nullability_return", &nullability_return, "return nullptr from _Nonnull function"},
    {"overflow_ptr", &overflow_ptr, "pointer arithmetic that overflows"},
    {"overflow_signed_int_add", &overflow_signed_int_add, "signed integer addition that overflows"},
    {"overflow_signed_int_shift", &overflow_signed_int_shift,
     "signed integer shift that overflows"},
    {"unaligned_assumption", &unaligned_assumption, "make a wrong alignment assumption"},
    {"undefined_enum", &undefined_enum, "use an undefined value in a enum"},
    {"undefined_bool", &undefined_bool, "use a bool that is not true nor false"},
    {"unreachable", &unreachable, "execute unreachable code.", true},
};

constexpr size_t kMaxCommandNameSize = [] {
  size_t size = 0;
  for (const auto& ub_cmd : kUbCommands) {
    size = ktl::max(size, ub_cmd.name.size());
  }
  return size;
}();

int cmd_usage(const char* cmd_name) {
  printf("usage:\n");
  for (const auto& ub_cmd : kUbCommands) {
    printf("%s %-*s : %s\n", cmd_name, static_cast<int>(kMaxCommandNameSize), ub_cmd.name.data(),
           ub_cmd.description);
  }
  return ZX_ERR_INTERNAL;
}

int cmd_ub(int argc, const cmd_args* argv, uint32_t flags) {
  const char* name = argv[0].str;
  if (argc != 2) {
    printf("Exactly one argument required.\n");
    return cmd_usage(name);
  }

  ktl::string_view subcommand = argv[1].str;
  for (const auto& ub_cmd : kUbCommands) {
    if (ub_cmd.name == subcommand) {
      if (ub_cmd.func) {
        ub_cmd.func();
      } else {
        for (const auto& next_ub_cmd : ktl::span(kUbCommands).subspan(1)) {
          if (BootOptions::Get()->ubsan_action == CheckFailAction::kOops &&
              next_ub_cmd.cannot_continue) {
            printf("*** Skipping `ub %s`, which cannot avoid panic ***\n", next_ub_cmd.name.data());
          } else {
            printf("*** ub %s\n", next_ub_cmd.name.data());
            next_ub_cmd.func();
          }
        }
      }
      return 0;
    }
  }

  return cmd_usage(name);
}

STATIC_COMMAND_START
STATIC_COMMAND("ub", "trigger undefined behavior", &cmd_ub)
STATIC_COMMAND_END(ub)

}  // namespace

#endif  // __has_feature(undefined_behavior_sanitizer)
