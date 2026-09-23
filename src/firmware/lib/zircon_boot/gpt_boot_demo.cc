// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file sets up an executable to run the example in
// src/firmware/lib/zircon_boot/gpt_boot_reference.c. See the source file for detail explanation.

#include <lib/storage/gpt_utils.h>
#include <lib/storage/storage.h>
#include <lib/zircon_boot/zircon_boot.h>

#include <atomic>
#include <chrono>
#include <thread>

#include "test/test_data/test_images.h"

#define ASSERT(cond)                                                     \
  do {                                                                   \
    if (!(cond)) {                                                       \
      printf("%s: %d. ASSERT(%s) failed.\n", __FILE__, __LINE__, #cond); \
      exit(1);                                                           \
    }                                                                    \
  } while (0)

extern FuchsiaFirmwareStorage gpt_boot_storage;
extern "C" ZirconBootResult GptBootMain(void);

void InitializeRamDisk() {
  // See src/firmware/lib/zircon_boot/test/test_data/generate_test_data.py for how test images used
  // in this demo are generated.
  ASSERT(
      FuchsiaFirmwareStorageWriteConst(&gpt_boot_storage, 0, sizeof(kTestGptDisk), kTestGptDisk));
  GptData gpt_data;
  ASSERT(FuchsiaFirmwareStorageSyncGpt(&gpt_boot_storage, &gpt_data));
  gpt_boot_storage.total_blocks = sizeof(kTestGptDisk) / gpt_boot_storage.block_size;
  ASSERT(FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, &gpt_data, "zircon_a", 0,
                                             sizeof(kTestZirconAImage), kTestZirconAImage));
  ASSERT(FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, &gpt_data, "zircon_b", 0,
                                             sizeof(kTestZirconBImage), kTestZirconBImage));
  ASSERT(FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, &gpt_data, "zircon_r", 0,
                                             sizeof(kTestZirconRImage), kTestZirconRImage));
  ASSERT(FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, &gpt_data, "vbmeta_a", 0,
                                             sizeof(kTestVbmetaAImage), kTestVbmetaAImage));
  ASSERT(FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, &gpt_data, "vbmeta_b", 0,
                                             sizeof(kTestVbmetaBImage), kTestVbmetaBImage));
  ASSERT(FuchsiaFirmwareStorageGptWriteConst(&gpt_boot_storage, &gpt_data, "vbmeta_r", 0,
                                             sizeof(kTestVbmetaRImage), kTestVbmetaRImage));
  FuchsiaFirmwareStorageFreeGptData(&gpt_boot_storage, &gpt_data);
}

std::atomic<char> key;

void StartKeyListener() {
  std::thread listener([&]() {
    while (true) {
      char ch = (char)getchar();
      if (ch == '\n') {
        continue;
      }
      key = ch;
    }
  });
  listener.detach();
}

extern "C" ZirconBootFlags WaitForUserForceRecoveryInput(uint32_t delay_seconds) {
  key = 0;
  auto start = std::chrono::high_resolution_clock::now();
  while (std::chrono::duration_cast<std::chrono::seconds>(
             std::chrono::high_resolution_clock::now() - start)
             .count() < delay_seconds) {
    if (key == 'f') {
      printf("Got force recovery request\n");
      return kZirconBootFlagsForceRecovery;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }

  return kZirconBootFlagsNone;
}

int main(int argc, char* argv[]) {
  printf("Initializing disk for demo...\n");
  InitializeRamDisk();

  StartKeyListener();
  printf("Start demo\n\n");
  ZirconBootResult res;
  do {
    res = GptBootMain();
  } while (res == kBootResultRebootReturn);

  return 0;
}
