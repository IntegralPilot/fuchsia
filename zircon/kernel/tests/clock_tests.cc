// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2012 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/arch/intrin.h>
#include <platform.h>
#include <stdio.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <kernel/event.h>
#include <kernel/mp.h>
#include <kernel/mutex.h>
#include <kernel/thread.h>

#include "tests.h"

int clock_tests(int, const cmd_args*, uint32_t) {
  uint64_t c;
  zx_instant_mono_t t2;

  Thread::Current::SleepRelative(ZX_MSEC(100));
  c = arch::Cycles();
  current_mono_time();
  c = arch::Cycles() - c;
  printf("%" PRIu64 " cycles per current_mono_time()\n", c);

  printf("making sure time never goes backwards\n");
  {
    printf("testing current_mono_time()\n");
    zx_instant_mono_t start = current_mono_time();
    zx_instant_mono_t last = start;
    for (;;) {
      t2 = current_mono_time();
      // printf("%llu %llu\n", last, t2);
      if (t2 < last) {
        printf("WARNING: time ran backwards: %" PRIi64 " < %" PRIi64 "\n", t2, last);
        last = t2;
        continue;
      }
      last = t2;
      if (last - start > ZX_SEC(5))
        break;
    }
  }

  printf("counting to 5, in one second intervals\n");
  for (int i = 0; i < 5; i++) {
    Thread::Current::SleepRelative(ZX_SEC(1));
    printf("%d\n", i + 1);
  }

  cpu_mask_t old_affinity = Thread::Current::Get()->GetCpuAffinity();

  for (cpu_num_t cpu = 0; cpu < SMP_MAX_CPUS; cpu++) {
    if (!mp_is_cpu_online(cpu))
      continue;

    printf("measuring cpu clock against current_mono_time() on cpu %u\n", cpu);

    Thread::Current::Get()->SetCpuAffinity(cpu_num_to_mask(cpu));

    for (int i = 0; i < 3; i++) {
      uint64_t cycles = arch::Cycles();
      zx_instant_mono_t start = current_mono_time();
      while ((current_mono_time() - start) < ZX_SEC(1))
        ;
      cycles = arch::Cycles() - cycles;
      printf("cpu %u: %" PRIu64 " cycles per second\n", cpu, cycles);
    }
  }

  Thread::Current::Get()->SetCpuAffinity(old_affinity);

  return 0;
}
