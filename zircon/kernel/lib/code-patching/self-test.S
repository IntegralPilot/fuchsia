// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <lib/code-patching/asm.h>
#include <arch/code-patches/case-id-asm.h>

.function CodePatchingNopTest, global
  .code_patching.start CASE_ID_SELF_TEST
  assert.fail
  .code_patching.end
  ret
.end_function
