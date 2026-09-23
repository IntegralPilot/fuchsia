// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/assert.h>

#include <ktl/string_view.h>
#include <phys/elf-image.h>
#include <phys/symbolize.h>

#include "physload-test-main.h"

#include <ktl/enforce.h>

int PhysLoadTestMain(KernelStorage& kernel_storage) {
  constexpr const char* kTestName = "physload-handoff-test";

  gSymbolize->set_name(kTestName);

  ktl::string_view me = gSymbolize->modules().back()->name();
  if (me != kTestName) {
    printf("symbolize->name() \"%s\", last module name \"%.*s\"", gSymbolize->name(),
           static_cast<int>(me.size()), me.data());
    return 1;
  }

  Allocation::GetPool().PrintMemoryRanges(gSymbolize->name());
  return 0;
}
