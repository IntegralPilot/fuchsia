// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/tee_internal_api/tee_internal_api.h>

TEE_Result TA_CreateEntryPoint() { TEE_Panic(0xdeadbeef); }

void TA_DestroyEntryPoint() { TEE_Panic(0xdeadbeef); }

TEE_Result TA_OpenSessionEntryPoint(uint32_t paramTypes,
                                    /* inout */ TEE_Param params[4],
                                    /* out */ /* ctx */ void** sessionContext) {
  TEE_Panic(0xdeadbeef);
}

void TA_CloseSessionEntryPoint(
    /* ctx */ void* sessionContext) {
  TEE_Panic(0xdeadbeef);
}

TEE_Result TA_InvokeCommandEntryPoint(
    /* ctx */ void* sessionContext, uint32_t commandID, uint32_t paramTypes,
    /* inout */ TEE_Param params[4]) {
  TEE_Panic(0xdeadbeef);
}
