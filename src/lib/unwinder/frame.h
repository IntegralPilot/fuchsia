// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_FRAME_H_
#define SRC_LIB_UNWINDER_FRAME_H_

#include <cstring>

#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

struct Frame {
  // LINT.IfChange
  enum class Trust {
    kScan,       // From scanning the stack with heuristics, least reliable.
    kSCS,        // From the shadow call stack.
    kSigReturn,  // From a sigreturn trampoline.
    kFP,         // From the frame pointer.
    kPLT,        // From PLT unwinder.
    kArmEhAbi,   // From the ARM Exception Handling ABI .ARM.exidx/.ARM.extab sections.
    kCFI,        // From call frame info / .eh_frame section.
    kContext,    // From the input / context, most reliable.
  };
  // LINT.ThenChange(//src/developer/debug/ipc/records.h)

  // Register status at each return site. Unknown registers may be included.
  Registers regs;

  // Whether the PC in |regs| is a return address or a precise code location.
  //
  // PC is usually a precise location for the first frame and a return address for the rest frames.
  // However, it could also be a precise location when it's not a regular function call, e.g., in
  // signal frames or when unwinding into restricted mode in Starnix.
  bool pc_is_return_address;

  // This flag is set to true if the CFI for this frame was associated with a CIE containing an 'S'
  // character in its augmentation string.
  //
  // The 'S' augmentation is poorly documented and is not a part of the DWARF standard, but is a de
  // facto standard to mark unwind information for signal trampolines. Unfortunately it's also not
  // mentioned in any of the Linux Standard Base (LSB) nor in any of the ARM vendor DWARF extension
  // specifications or other standards referenced by this unwinder. In practice, this is set by hand
  // writing CFI directives (.cfi_signal_frame in particular).
  //
  // The best references that can be found are in source code of other unwinders: libgcc has the
  // canonical implementation of this augmentation [1], and libunwind also implements it [2].
  //
  // .cfi_signal_frame is not always set on functions that are actually signal frames, so this flag
  // can be set by probing the instruction sequence set at |pc| for a particular sigreturn method,
  // which has a predictable and short instruction sequence. This approach is only taken in the case
  // the CFI unwinder failed to find the 'S' augmentation.
  //
  // [1]: https://gcc.gnu.org/git/?p=gcc.git;a=blob;f=libgcc/unwind-dw2.c;h=883b1c7a13dbdf620e64724a2179ffcbfdfa9c69;hb=HEAD#l498
  // [2]: https://github.com/llvm/llvm-project/blob/main/libunwind/src/DwarfParser.hpp#L395
  bool is_signal_frame = false;

  // Trust level of the frame.
  Trust trust;

  // Error when unwinding from this frame, which could be non-fatal and present in any frames.
  Error error = Success();

  // Whether the above error is fatal and aborts the unwinding, causing the stack to be incomplete.
  // This could only be true for the last frame.
  bool fatal_error = false;

  // Disallow default constructors.
  Frame(Registers regs, bool pc_is_return_address, Trust trust)
      : regs(std::move(regs)), pc_is_return_address(pc_is_return_address), trust(trust) {}

  // Useful for debugging.
  std::string Describe() const;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_FRAME_H_
