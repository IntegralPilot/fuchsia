// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page/size.h>

#include <hwreg/asm.h>

int main(int argc, char** argv) {
  return hwreg::AsmHeader()
      .Macro("PAGE_SHIFT", kPageShift)
      .Macro("PAGE_SIZE", kPageSize)
      .Macro("PAGE_MASK", kPageMask)
      .Main(argc, argv);
}
