// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_BACKTRACE_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_BACKTRACE_H_

#include <lib/arch/internal/arch-backtrace.h>
#include <zircon/compiler.h>

#include <concepts>
#include <cstdint>
#include <iterator>
#include <ranges>
#include <span>
#include <type_traits>

namespace arch {

// The FramePointerBacktrace and ShadowCallStackBacktrace classes (see below)
// provide container-like APIs for safely traversing a backtrace from either
// source using forward iteration.
//
// This stores a backtrace from either of those in a fixed-sized buffer, and
// returns the number of frames stored there.  The optional third argument can
// be `__builtin_return_address(0)` in the function where the backtrace was
// collected.  This will be included as the innermost frame if the backtrace
// doesn't already record it there, such as when the collecting function has
// no frame pointer itself.
template <class Backtrace>
inline size_t StoreBacktrace(Backtrace&& bt, std::span<uintptr_t> pcs, void* raptr = nullptr) {
  if (pcs.empty()) [[unlikely]] {
    return 0;
  }

  // Prepend the immediate return address if it's not the innermost caller
  // already, in case the capturing function doesn't have a frame pointer or
  // shadow-call-stack spill itself.  (The latter should technically be
  // impossible, since the arch::GetShadowCallStackPointer() call always
  // requires a spill of the return-address register.)
  uintptr_t ra = reinterpret_cast<uintptr_t>(raptr);
  if (raptr && bt.empty()) {
    pcs.front() = ra;
    return 1;
  }

  size_t i = 0;
  for (uintptr_t pc : bt) {
    if (raptr && i == 0 && pc != ra) {
      pcs[i++] = ra;
    }
    if (i == pcs.size()) {
      break;
    }
    pcs[i++] = pc;
  }
  return i;
}

// Each frame records its caller's FP and PC (return address).  A call pushes
// the PC and the prologue then pushes the caller's FP (x86), or the prologue
// pushes the return-address register and PC together (other CPUs). Since the
// stack grows down, the PC is always just after the FP in memory.  It then
// sets the FP to point at (or above) the FP, PC pair just pushed.  On x86 it's
// unavoidable that the FP is two words below the CFA (SP at call site), since
// the call itself puts the PC there; the FP points directly to the FP, PC pair
// describing the caller.  On ARM, the compiler will often place the FP, PC
// pair at the bottom of the new frame instead of the top; the FP points
// directly to the FP, PC pair describing the caller, but there's no guarantee
// where the FP is in relation to the CFA.  On RISC-V, the FP is set to the CFA
// (SP at call site / entry); so it points *just past* the FP, PC pair
// describing the caller, but it's also guaranteed to be the CFA.
struct CallFrame {
  // Frames are ordered by FP address.  Since stacks grow down, the least
  // address means the innermost callee.
  constexpr auto operator<=>(const CallFrame&) const = default;

  // On most machines, this is the CFA, which is the SP at the call site and
  // thus the address that's the upper bound on this call frame.  On ARM it's
  // more likely to be near the bottom of the frame, but will still at least be
  // an upper bound on the address range of the next frame in.
  uintptr_t frame_address() const {
    return fp ? reinterpret_cast<uintptr_t>(fp + arch::internal::kArchFpOffset + 1) : 0;
  }

  const CallFrame* fp = nullptr;
  uintptr_t pc = 0;
};

// The template parameter to FramePointerBacktrace is the type of a callable
// object to determine whether it's safe to dereference a known-aligned pointer
// that's expected to be on the stack.  The type must be default-constructible
// and copyable.  But a non-default object can be passed to BackTrace().
template <typename T>
concept IsOnStackApi = std::default_initializable<T> && std::copyable<T> &&
                       requires(T is_on_stack, const CallFrame* fp) {
                         { is_on_stack(fp) } -> std::convertible_to<bool>;
                       };

// A FramePointerBacktrace is a forward iterator object that also acts as its
// own forward-range object.  So in a range-based for loop it yields a list of
// uintptr_t PC values.  When the optional second template parameter is true,
// it instead yields a list of CallFrame values.
template <IsOnStackApi IsOnStack, bool WithFp = false>
class FramePointerBacktrace {
 public:
  using iterator = FramePointerBacktrace;
  using const_iterator = iterator;
  using value_type = std::conditional_t<WithFp, CallFrame, uintptr_t>;
  using difference_type = ptrdiff_t;
  using reference = const value_type&;
  using pointer = const value_type*;
  using iterator_category = std::forward_iterator_tag;

  FramePointerBacktrace() = default;
  FramePointerBacktrace(const FramePointerBacktrace&) = default;
  FramePointerBacktrace& operator=(const FramePointerBacktrace&) = default;

  // The caller evaluates the default argument to supply its own backtrace:
  // `for (uintptr_t pc : FramePointerBacktrace::BackTrace()) { ... }` or
  // `vector<uintptr_t>(FramePointerBacktrace::BackTrace(),
  // FramePointerBacktrace::end())`.  That way the immediate caller itself is
  // not included in the backtrace.
  static FramePointerBacktrace BackTrace(
      const CallFrame* fp = static_cast<const CallFrame*>(__builtin_frame_address(0)),
      IsOnStack is_on_stack = {}) {
    FramePointerBacktrace bt;
    bt.is_on_stack_ = is_on_stack;
    // frame_ is a copy of the CallFrame describing the caller's caller.  If
    // this object instead held only a CallFrame* pointer, that would be a
    // pointer into the caller's frame and so would not be valid after the
    // caller returns.  But since the backtrace intentionally omits the
    // immediate caller, it's reasonable to expect that some callers might be
    // returning the backtrace object.  The increment operator just does a
    // copy with arch adjustment and safety checks (alignment and on-stack).
    // If the safety checks fail, the object winds up in empty / end() state.
    bt.frame_.fp = fp;
    ++bt;
    return bt;
  }

  static FramePointerBacktrace BackTrace(uintptr_t pc) {
    return BackTrace(reinterpret_cast<const CallFrame*>(pc));
  }

  // Container interface.

  bool empty() const { return *this == end(); }

  iterator begin() const { return *this; }

  static iterator end() { return {}; }

  // Iterator interface.

  bool operator==(const FramePointerBacktrace& other) const { return frame_ == other.frame_; }

  FramePointerBacktrace& operator++() {  // prefix
    static_assert(std::ranges::forward_range<FramePointerBacktrace>);
    if (frame_.fp && IsAligned(frame_.fp)) {
      const CallFrame* fp = frame_.fp + arch::internal::kArchFpOffset;
      if (is_on_stack_(fp)) [[likely]] {
        frame_ = *fp;
        return *this;
      }
    }
    frame_ = {};
    return *this;
  }

  FramePointerBacktrace operator++(int) {  // postfix
    auto old = *this;
    ++*this;
    return old;
  }

  value_type operator*() const {
    if constexpr (WithFp) {
      return frame_;
    } else {
      return frame_.pc;
    }
  }

 private:
  static bool IsAligned(const CallFrame* fp) {
    return reinterpret_cast<uintptr_t>(fp) % alignof(CallFrame) == 0;
  }

  CallFrame frame_;
  __NO_UNIQUE_ADDRESS IsOnStack is_on_stack_{};
};

// Note there is no BackTrace() method here as in FramePointerBacktrace.
// arch::GetShadowCallStackPointer() can be used to get the upper bound of the
// currently-used shadow-call-stack, but finding the lower bound depends on the
// runtime environment's exact details.  So both fetching the base address and
// checking whether the current shadow-call-stack pointer falls within the
// allocated bounds must be dealt with before passing the constructor a valid
// span of the in-use part of the shadow-call-stack.
class ShadowCallStackBacktrace {
 public:
  ShadowCallStackBacktrace() = default;

  ShadowCallStackBacktrace(const ShadowCallStackBacktrace&) = default;

  explicit ShadowCallStackBacktrace(std::span<const uintptr_t> stack) : stack_(stack) {}

  ShadowCallStackBacktrace(std::span<const uintptr_t> stack, uintptr_t sp) : stack_(stack) {
    ShrinkToSp(sp);
  }

  ShadowCallStackBacktrace& operator=(const ShadowCallStackBacktrace&) = default;

  // The shadow call stack grows up, so iterating over frames from innermost to
  // outermost has to go last to first.

  bool empty() const { return stack_.empty(); }

  auto begin() const { return stack_.rbegin(); }

  auto end() const { return stack_.rend(); }

  bool IsValidSp(uintptr_t sp) const {
    return IsValidSp(sp, reinterpret_cast<uintptr_t>(stack_.data()), stack_.size_bytes());
  }

  static bool IsValidSp(uintptr_t sp, uintptr_t base, size_t size) {
    return sp >= base && sp - base <= size && sp % sizeof(uintptr_t) == 0;
  }

  void ShrinkToSp(uintptr_t sp) {
    if (IsValidSp(sp)) {
      stack_ = stack_.subspan(0, reinterpret_cast<const uintptr_t*>(sp) - stack_.data());
    } else {
      stack_ = {};
    }
    // An outermost return address of zero can be used as a marker not to look
    // further back.
    while (!stack_.empty() && stack_.front() == 0) {
      stack_ = stack_.subspan(1);
    }
  }

 private:
  std::span<const uintptr_t> stack_;
};
static_assert(std::ranges::random_access_range<ShadowCallStackBacktrace>);

#if __has_feature(shadow_call_stack)
// This has to be defined separately in assembly (not inline asm), even though
// it just fetches a register.
extern "C" uintptr_t GetShadowCallStackPointer();
#else
constexpr uintptr_t GetShadowCallStackPointer() { return 0; }
#endif

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_BACKTRACE_H_
