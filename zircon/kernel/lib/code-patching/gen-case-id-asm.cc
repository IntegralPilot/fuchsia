// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdint.h>

#include <initializer_list>
#include <string>
#include <utility>

#include <hwreg/asm.h>

namespace {

namespace Arm {
#include <arch/arm64/code-patches/include/arch/code-patches/case-id.h>
}  // namespace Arm

namespace Riscv {
#include <arch/riscv64/code-patches/include/arch/code-patches/case-id.h>
}  // namespace Riscv

namespace X86 {
#include <arch/x86/code-patches/include/arch/code-patches/case-id.h>
}  // namespace X86

template <typename Id, typename WithNames>
void EmitPatchNames(WithNames&& with_names, std::string_view arch, hwreg::AsmHeader& header) {
  header.Line({"#ifdef ", arch});

  using Patch = std::pair<Id, std::string_view>;
  with_names([&header](std::initializer_list<Patch> names) {
    for (auto [id, name] : names) {
      header.Macro(std::string("CASE_ID_") + std::string(name), static_cast<uint32_t>(id));
    }
  });

  header.Line({"#endif  // ", arch});
}

}  // namespace

int main(int argc, char** argv) {
  auto header = hwreg::AsmHeader();
  EmitPatchNames<Arm::CodePatchId>(Arm::WithCodePatchNames, "__aarch64__", header);
  EmitPatchNames<Riscv::CodePatchId>(Riscv::WithCodePatchNames, "__riscv", header);
  EmitPatchNames<X86::CodePatchId>(X86::WithCodePatchNames, "__x86_64__", header);
  return header.Main(argc, argv);
}
