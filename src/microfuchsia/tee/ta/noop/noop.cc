// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/tee_internal_api/tee_internal_api.h>

TEE_Result TA_CreateEntryPoint() { return TEE_SUCCESS; }

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
