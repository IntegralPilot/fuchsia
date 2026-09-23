// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <lib/tee_internal_api/tee_internal_api.h>
#include <stdio.h>
#include <unistd.h>

// This tests that all of the expected TEE symbols are available within a TA.  Some require
// specialized setup or have side effects that make them difficult to call in isolation so this
// looks up the symbols directly instead of invoking them.

const char* const kExpectedTeeSymbols[] = {
    "TEE_Panic",   "TEE_Malloc",     "TEE_Realloc", "TEE_Free",
    "TEE_MemMove", "TEE_MemCompare", "TEE_MemFill",
};

void load_expected_tee_symbols() {
  for (const char* name : kExpectedTeeSymbols) {
    if (dlsym(RTLD_DEFAULT, name) == nullptr) {
      fprintf(stderr, "could not load symbol %s\n", name);
      _exit(1);
    }
  }
}

TEE_Result TA_CreateEntryPoint() {
  load_expected_tee_symbols();
  return TEE_SUCCESS;
}

void TA_DestroyEntryPoint() {}

TEE_Result TA_OpenSessionEntryPoint(uint32_t paramTypes,
                                    /* inout */ TEE_Param params[4],
                                    /* out */ /* ctx */ void** sessionContext) {
  return TEE_SUCCESS;
}

void TA_CloseSessionEntryPoint(
    /* ctx */ void* sessionContext) {}

TEE_Result TA_InvokeCommandEntryPoint(
    /* ctx */ void* sessionContext, uint32_t commandID, uint32_t paramTypes,
    /* inout */ TEE_Param params[4]) {
  return TEE_SUCCESS;
}
