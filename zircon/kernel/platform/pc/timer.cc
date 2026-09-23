// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2009 Corey Tabaka
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/affine/transform.h>
#include <lib/arch/intrin.h>
#include <lib/boot-options/boot-options.h>
#include <lib/boot-options/types.h>
#include <lib/counters.h>
#include <lib/fixed_point.h>
#include <platform.h>
#include <pow2.h>
#include <sys/types.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <arch/x86.h>
#include <arch/x86/apic.h>
#include <arch/x86/feature.h>
#include <arch/x86/pv.h>
#include <arch/x86/timer_freq.h>
#include <dev/interrupt.h>
#include <fbl/algorithm.h>
#include <kernel/spinlock.h>
#include <kernel/thread.h>
#include <ktl/bit.h>
#include <ktl/iterator.h>
#include <ktl/limits.h>
#include <lk/init.h>
#include <phys/handoff.h>
#include <platform/pc.h>
#include <platform/pc/hpet.h>
#include <platform/pc/timer.h>
#include <platform/timer.h>

#include <ktl/enforce.h>

KCOUNTER(platform_timer_set_counter, "platform.timer.set")
KCOUNTER(platform_timer_cancel_counter, "platform.timer.cancel")

// Current timer scheme:
// The HPET is used to calibrate the local APIC timers and the TSC.  If the
// HPET is not present, we will fallback to calibrating using the PIT.
//
// For wall-time, we use the following mechanisms, in order of highest
// preference to least:
// 1) TSC: If the CPU advertises an invariant TSC, then we will use the TSC for
// tracking wall time in a tickless manner.
// 2) HPET: If there is an HPET present, we will use its count to track wall
// time in a tickless manner.
// 3) PIT: We will use periodic interrupts to update wall time.
//
// The local APICs are responsible for handling timer callbacks
// sent from the scheduler.

enum clock_source {
  // Used before wall_clock is selected. current_mono_ticks() returns 0.
  CLOCK_UNSELECTED = 0,

  CLOCK_TSC,
  CLOCK_PIT,
  CLOCK_HPET,

  CLOCK_COUNT
};

#if defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
#endif
const char* clock_name[] = {
    [CLOCK_UNSELECTED] = "UNSELECTED",
    [CLOCK_TSC] = "TSC",
    [CLOCK_PIT] = "PIT",
    [CLOCK_HPET] = "HPET",
};
#if defined(__clang__)
#pragma GCC diagnostic pop
#endif
static_assert(ktl::size(clock_name) == CLOCK_COUNT, "");

// PIT time accounting info
static struct fp_32_64 us_per_pit;
static volatile uint64_t pit_ticks;
static uint16_t pit_divisor;

// Whether or not we have an Invariant TSC (controls whether we use the PIT or
// not after initialization).  The Invariant TSC is rate-invariant under P-, C-,
// and T-state transitions.
static bool invariant_tsc;
// Whether or not we have a Constant TSC (controls whether we bother calibrating
// the TSC).  Constant TSC predates the Invariant TSC.  The Constant TSC is
// rate-invariant under P-state transitions.
static bool constant_tsc;

// The ratio between the chosen reference timer's ticks and the APIC's ticks.
// This is set after clock selection is complete in pc_init_timer.
static affine::Ratio reference_timer_ticks_to_apic_ticks;

static enum clock_source wall_clock = CLOCK_UNSELECTED;
static enum clock_source calibration_clock;

// APIC timer calibration values
static bool use_tsc_deadline;
static uint32_t apic_ticks_per_ms = 0;
static struct fp_32_64 apic_ticks_per_ns;
static uint8_t apic_divisor = 0;

// TSC timer calibration values
static uint64_t tsc_ticks_per_ms;
static struct fp_32_64 ns_per_tsc;
static affine::Ratio rdtsc_ticks_to_clock_monotonic;

// HPET calibration values
static struct fp_32_64 ns_per_hpet;
static affine::Ratio hpet_ticks_to_clock_monotonic;

// An affine transformation from times sampled from the EarlyTicks timeline to
// the chosen ticks timeline.  By default, this transformation is set up as:
//
//   f(t) = (((t - 0) * 0) / 1) + 0;
//
// meaning that it will map all early ticks value `t` to 0, and the inverse
// transformation will be undefined.  This is consistent with with simply
// reporting 0 for normalized EarlyTicks values if we cannot (or do not know how
// to) convert from one timeline to the other.
static affine::Transform early_ticks_to_ticks{0, 0, {0, 1}};

#define INTERNAL_FREQ 1193182U
#define INTERNAL_FREQ_3X 3579546U

#define INTERNAL_FREQ_TICKS_PER_MS (INTERNAL_FREQ / 1000)

/* Maximum amount of time that can be program on the timer to schedule the next
 *  interrupt, in miliseconds */
#define MAX_TIMER_INTERVAL ZX_MSEC(55)

#define LOCAL_TRACE 0

static inline zx_ticks_t current_ticks_rdtsc(void) { return _rdtsc(); }
static inline zx_ticks_t current_ticks_rdtscp(void) {
  unsigned int unused;
  return __rdtscp(&unused);
}

static zx_ticks_t current_ticks_hpet(void) { return hpet_get_value(); }
static zx_ticks_t current_ticks_pit(void) { return pit_ticks; }

template <GetTicksSyncFlag Flags>
inline zx_ticks_t platform_current_raw_ticks_synchronized() {
  // Directly call the ticks functions to avoid the cost of a virtual (indirect) call.
  if (wall_clock == CLOCK_TSC) {
    // See "Intel® 64 and IA-32 Architectures Software Developer’s Manual Vol.
    // 2B Section 4.3", specifically the entries for RDTSC and RDTSCP for a
    // description of the serialization properties of the instructions which
    // access the TSC.
    //
    // If all stores must be completed and "globally visible" before the TSC is
    // sampled, docs say to put an MFENCE in front of the TSC access.
    if constexpr ((Flags & GetTicksSyncFlag::kAfterPreviousStores) != GetTicksSyncFlag::kNone) {
      arch::DeviceMemoryBarrier();
    }

    // If all loads must be complete and "globally visible" (meaning that the
    // value to load has been determined) before the TSC is sampled, docs say to
    // either execute `LFENCE ; RDTSC` or just `RDTSCP`.
    const zx_ticks_t ret = []() {
      if constexpr ((Flags & GetTicksSyncFlag::kAfterPreviousLoads) != GetTicksSyncFlag::kNone) {
        return current_ticks_rdtscp();
      } else {
        return current_ticks_rdtsc();
      }
    }();

    // Finally, if we need the TSC sampling to have finished before any
    // subsequent loads/stores start, docs say that we should put an LFENCE
    // immediately after the RDTSC/RDTSCP.
    if constexpr ((Flags & (GetTicksSyncFlag::kBeforeSubsequentLoads |
                            GetTicksSyncFlag::kBeforeSubsequentStores)) !=
                  GetTicksSyncFlag::kNone) {
      __asm__ __volatile__("lfence" ::: "memory");
    }

    return ret;
  } else {
    switch (wall_clock) {
      case CLOCK_UNSELECTED:
        return 0;
      case CLOCK_PIT:
        // In theory, we should not need anything special to synchronize the
        // PIT. Right now, the PIT is just a global counter incremented by an
        // IRQ handler when the interrupt timer fires once per msec, and Intel's
        // memory model is strongly ordered, implying that no special
        // synchronization should be required.
        return current_ticks_pit();
      case CLOCK_HPET:
        // TODO(johngro): Research and apply any barriers required to
        // synchronize observations of the HPET with the instruction pipeline.
        // Right now, we almost never use the HPET as our reference, which
        // somewhat lowers the priority of this issue.
        return current_ticks_hpet();
      default:
        PANIC_UNIMPLEMENTED;
    }
  }
}

// Explicit instantiation of all of the forms of synchronized tick access.
#define EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(flags) \
  template zx_ticks_t                                         \
  platform_current_raw_ticks_synchronized<static_cast<GetTicksSyncFlag>(flags)>()
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(0);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(1);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(2);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(3);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(4);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(5);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(6);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(7);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(8);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(9);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(10);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(11);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(12);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(13);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(14);
EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED(15);
#undef EXPAND_PLATFORM_CURRENT_RAW_TICKS_SYNCHRONIZED

zx_duration_t convert_raw_tsc_duration_to_nanoseconds(int64_t duration) {
  return rdtsc_ticks_to_clock_monotonic.Scale(duration);
}

zx_instant_mono_t convert_raw_tsc_timestamp_to_clock_monotonic(int64_t ts) {
  if (wall_clock == CLOCK_TSC) {
    // If TSC is being used as our clock monotonic reference, then conversion is
    // simple.  We just need to convert from the raw TSC timestamps to a ticks
    // timestamp by adding the offset, then scale by the ticks -> mono ratio.
    // As the offset is only updated early during boot when we're running on a
    // single core with interrupts disabled, we don't need to worry about thread
    // synchronization so memory_order_relaxed is sufficient.
    int64_t abs_ticks = ts + timer_get_mono_ticks_offset();
    return rdtsc_ticks_to_clock_monotonic.Scale(abs_ticks);
  } else {
    // If we are using something other than TSC as our monotonic reference, then
    // things are slightly more tricky.  We need to figure out how far in the
    // future this TSC timestamp is (in nanoseconds), and then add that delta to
    // the current time to establish the new deadline.
    //
    // Bracket our observation of current time with two observations of ticks,
    // and use the average of those two values to create the ticks half of the
    // correspondence pair.
    uint64_t before_tsc = current_ticks_rdtsc();
    zx_instant_mono_t now_mono = current_mono_time();
    uint64_t after_tsc = current_ticks_rdtsc();
    uint64_t now_tsc = (before_tsc >> 1) + (after_tsc >> 1) + (before_tsc & after_tsc & 1);
    int64_t time_till_tsc_timestamp = zx_time_sub_time(ts, now_tsc);
    return now_mono = rdtsc_ticks_to_clock_monotonic.Scale(time_till_tsc_timestamp);
  }
}

/* i8253/i8254 programmable interval timer registers */
static constexpr uint16_t I8253_CONTROL_REG = 0x43;
static constexpr uint16_t I8253_DATA_REG = 0x40;

// The PIT timer will keep track of wall time if we aren't using the TSC
static void pit_timer_tick(void* cookie) { pit_ticks = pit_ticks + 1; }

// The APIC timers will call this when they fire
void platform_handle_apic_timer_tick(void) { timer_tick(); }

static void set_pit_frequency(uint32_t frequency) {
  uint32_t count, remainder;

  /* figure out the correct pit_divisor for the desired frequency */
  if (frequency <= 18) {
    count = 0xffff;
  } else if (frequency >= INTERNAL_FREQ) {
    count = 1;
  } else {
    count = INTERNAL_FREQ_3X / frequency;
    remainder = INTERNAL_FREQ_3X % frequency;

    if (remainder >= INTERNAL_FREQ_3X / 2) {
      count += 1;
    }

    count /= 3;
    remainder = count % 3;

    if (remainder >= 1) {
      count += 1;
    }
  }

  pit_divisor = count & 0xffff;

  /*
   * funky math that i don't feel like explaining. essentially 32.32 fixed
   * point representation of the configured timer delta.
   */
  fp_32_64_div_32_32(&us_per_pit, 1000 * 1000 * 3 * count, INTERNAL_FREQ_3X);

  // dprintf(DEBUG, "set_pit_frequency: pit_divisor=%04x\n", pit_divisor);

  /*
   * setup the Programmable Interval Timer
   * timer 0, mode 2, binary counter, LSB followed by MSB
   */
  outp(I8253_CONTROL_REG, 0x34);
  outp(I8253_DATA_REG, static_cast<uint8_t>(pit_divisor));       // LSB
  outp(I8253_DATA_REG, static_cast<uint8_t>(pit_divisor >> 8));  // MSB
}

static inline void pit_calibration_cycle_preamble(uint16_t ms) {
  // Make the PIT run for
  const uint16_t init_pic_count = static_cast<uint16_t>(INTERNAL_FREQ_TICKS_PER_MS * ms);
  // Program PIT in the interrupt on terminal count configuration,
  // this makes it count down and set the output high when it hits 0.
  outp(I8253_CONTROL_REG, 0x30);
  outp(I8253_DATA_REG, static_cast<uint8_t>(init_pic_count));  // LSB
}

static inline void pit_calibration_cycle(uint16_t ms) {
  // Make the PIT run for ms millis, see comments in the preamble
  const uint16_t init_pic_count = static_cast<uint16_t>(INTERNAL_FREQ_TICKS_PER_MS * ms);
  outp(I8253_DATA_REG, static_cast<uint8_t>(init_pic_count >> 8));  // MSB

  uint8_t status = 0;
  do {
    // Send a read-back command that latches the status of ch0
    outp(I8253_CONTROL_REG, 0xe2);
    status = inp(I8253_DATA_REG);
    // Wait for bit 7 (output) to go high and for bit 6 (null count) to go low
  } while ((status & 0xc0) != 0x80);
}

static inline void pit_calibration_cycle_cleanup(void) {
  // Stop the PIT by starting a mode change but not writing a counter
  outp(I8253_CONTROL_REG, 0x38);
}

static inline void hpet_calibration_cycle_preamble(void) { hpet_enable(); }

static inline void hpet_calibration_cycle(uint16_t ms) { hpet_wait_ms(ms); }

static inline void hpet_calibration_cycle_cleanup(void) { hpet_disable(); }

static void calibrate_apic_timer(void) {
  ASSERT(arch_ints_disabled());

  const uint64_t apic_freq = x86_lookup_core_crystal_freq();
  if (apic_freq != 0) {
    ASSERT(apic_freq / 1000 <= UINT32_MAX);
    apic_ticks_per_ms = static_cast<uint32_t>(apic_freq / 1000);
    apic_divisor = 1;
    fp_32_64_div_32_32(&apic_ticks_per_ns, apic_ticks_per_ms, 1000 * 1000);
    printf("APIC frequency: %" PRIu32 " ticks/ms\n", apic_ticks_per_ms);
    return;
  }

  printf("Could not find APIC frequency: Calibrating APIC with %s\n",
         clock_name[calibration_clock]);

  apic_divisor = 1;
outer:
  while (apic_divisor != 0) {
    uint32_t best_time[2] = {UINT32_MAX, UINT32_MAX};
    const uint16_t duration_ms[2] = {2, 4};
    for (int trial = 0; trial < 2; ++trial) {
      for (int tries = 0; tries < 3; ++tries) {
        switch (calibration_clock) {
          case CLOCK_HPET:
            hpet_calibration_cycle_preamble();
            break;
          case CLOCK_PIT:
            pit_calibration_cycle_preamble(duration_ms[trial]);
            break;
          default:
            PANIC_UNIMPLEMENTED;
        }

        // Setup APIC timer to count down with interrupt masked
        zx_status_t status = apic_timer_set_oneshot(UINT32_MAX, apic_divisor, true);
        ASSERT(status == ZX_OK);

        switch (calibration_clock) {
          case CLOCK_HPET:
            hpet_calibration_cycle(duration_ms[trial]);
            break;
          case CLOCK_PIT:
            pit_calibration_cycle(duration_ms[trial]);
            break;
          default:
            PANIC_UNIMPLEMENTED;
        }

        uint32_t apic_ticks = UINT32_MAX - apic_timer_current_count();
        if (apic_ticks < best_time[trial]) {
          best_time[trial] = apic_ticks;
        }
        LTRACEF("Calibration trial %d found %u ticks/ms\n", tries, apic_ticks);

        switch (calibration_clock) {
          case CLOCK_HPET:
            hpet_calibration_cycle_cleanup();
            break;
          case CLOCK_PIT:
            pit_calibration_cycle_cleanup();
            break;
          default:
            PANIC_UNIMPLEMENTED;
        }
      }

      // If the APIC ran out of time every time, try again with a higher
      // divisor
      if (best_time[trial] == UINT32_MAX) {
        apic_divisor = static_cast<uint8_t>(apic_divisor * 2);
        goto outer;
      }
    }
    apic_ticks_per_ms = (best_time[1] - best_time[0]) / (duration_ms[1] - duration_ms[0]);
    fp_32_64_div_32_32(&apic_ticks_per_ns, apic_ticks_per_ms, 1000 * 1000);
    break;
  }
  ASSERT(apic_divisor != 0);

  printf("APIC timer calibrated: %" PRIu32 " ticks/ms, divisor %d\n", apic_ticks_per_ms,
         apic_divisor);
}

static uint64_t calibrate_tsc_count(uint16_t duration_ms) {
  zx_ticks_t best_time = ktl::numeric_limits<zx_ticks_t>::max();

  for (int tries = 0; tries < 3; ++tries) {
    switch (calibration_clock) {
      case CLOCK_HPET:
        hpet_calibration_cycle_preamble();
        break;
      case CLOCK_PIT:
        pit_calibration_cycle_preamble(duration_ms);
        break;
      default:
        PANIC_UNIMPLEMENTED;
    }

    arch::SerializeInstructions();
    uint64_t start = _rdtsc();
    arch::SerializeInstructions();

    switch (calibration_clock) {
      case CLOCK_HPET:
        hpet_calibration_cycle(duration_ms);
        break;
      case CLOCK_PIT:
        pit_calibration_cycle(duration_ms);
        break;
      default:
        PANIC_UNIMPLEMENTED;
    }

    arch::SerializeInstructions();
    zx_ticks_t end = _rdtsc();
    arch::SerializeInstructions();

    zx_ticks_t tsc_ticks = end - start;
    if (tsc_ticks < best_time) {
      best_time = tsc_ticks;
    }
    LTRACEF("Calibration trial %d found %" PRId64 " ticks/ms\n", tries, tsc_ticks);
    switch (calibration_clock) {
      case CLOCK_HPET:
        hpet_calibration_cycle_cleanup();
        break;
      case CLOCK_PIT:
        pit_calibration_cycle_cleanup();
        break;
      default:
        PANIC_UNIMPLEMENTED;
    }
  }

  return best_time;
}

static void calibrate_tsc(bool has_pv_clock) {
  ASSERT(arch_ints_disabled());

  const uint64_t tsc_freq = has_pv_clock ? pv_clock_get_tsc_freq() : x86_lookup_tsc_freq();
  if (tsc_freq != 0) {
    uint64_t N = 1'000'000'000;
    uint64_t D = tsc_freq;
    affine::Ratio::Reduce(&N, &D);

    // ASSERT that we can represent this as a 32 bit ratio.  If we cannot,
    // it means that tsc_freq is a number so large, and with so few prime
    // factors of 2 and 5, that it cannot be reduced to fit into a 32 bit
    // integer.  This is pretty unreasonable, for now, just assert that it
    // will not happen.
    ZX_ASSERT_MSG(
        (N <= ktl::numeric_limits<uint32_t>::max()) && (D <= ktl::numeric_limits<uint32_t>::max()),
        "Clock monotonic ticks : RDTSC ticks ratio (%lu : %lu) "
        "too large to store in a 32 bit ratio!!",
        N, D);
    rdtsc_ticks_to_clock_monotonic = {static_cast<uint32_t>(N), static_cast<uint32_t>(D)};

    tsc_ticks_per_ms = tsc_freq / 1000;
    printf("TSC frequency: %" PRIu64 " ticks/ms\n", tsc_ticks_per_ms);
  } else {
    printf("Could not find TSC frequency: Calibrating TSC with %s\n",
           clock_name[calibration_clock]);

    uint32_t duration_ms[2] = {2, 4};
    uint64_t best_time[2] = {calibrate_tsc_count(static_cast<uint16_t>(duration_ms[0])),
                             calibrate_tsc_count(static_cast<uint16_t>(duration_ms[1]))};

    while (best_time[0] >= best_time[1] && 2 * duration_ms[1] < MAX_TIMER_INTERVAL) {
      duration_ms[0] = duration_ms[1];
      duration_ms[1] *= 2;
      best_time[0] = best_time[1];
      best_time[1] = calibrate_tsc_count(static_cast<uint16_t>(duration_ms[1]));
    }

    ASSERT(best_time[0] < best_time[1]);

    uint64_t tsc_ticks_per_sec =
        ((best_time[1] - best_time[0]) * 1000) / (duration_ms[1] - duration_ms[0]);

    ZX_ASSERT_MSG(tsc_ticks_per_sec <= ktl::numeric_limits<uint32_t>::max(),
                  "Estimated TSC (%lu) is to high!\n", tsc_ticks_per_sec);

    tsc_ticks_per_ms = tsc_ticks_per_sec / 1000;
    rdtsc_ticks_to_clock_monotonic = {1'000'000'000, static_cast<uint32_t>(tsc_ticks_per_sec)};

    printf("TSC calibrated: %" PRIu64 " ticks/ms\n", tsc_ticks_per_ms);
  }

  ASSERT(tsc_ticks_per_ms <= UINT32_MAX);
  fp_32_64_div_32_32(&ns_per_tsc, 1000 * 1000, static_cast<uint32_t>(tsc_ticks_per_ms));

  LTRACEF("ns_per_tsc: %08x.%08x%08x\n", ns_per_tsc.l0, ns_per_tsc.l32, ns_per_tsc.l64);
}

static void pc_init_timer(uint level) {
  const struct x86_model_info* cpu_model = x86_get_model();
  // Declares the desired PIT frequency to be 1000, which gives us ~1ms granularity.
  // This may not be used if we chose to use a different platform reference timer.
  constexpr uint32_t desired_pit_frequency = 1000;

  constant_tsc = false;
  if (x86_vendor == X86_VENDOR_INTEL) {
    /* This condition taken from Intel 3B 17.15 (Time-Stamp Counter).  This
     * is the negation of the non-Constant TSC section, since the Constant
     * TSC section is incomplete (the behavior is architectural going
     * forward, and modern CPUs are not on the list). */
    constant_tsc = !((cpu_model->family == 0x6 && cpu_model->model == 0x9) ||
                     (cpu_model->family == 0x6 && cpu_model->model == 0xd) ||
                     (cpu_model->family == 0xf && cpu_model->model < 0x3));
  }
  invariant_tsc = x86_feature_test(X86_FEATURE_INVAR_TSC);

  bool has_pv_clock = x86_hypervisor_has_pv_clock();
  if (has_pv_clock) {
    zx_status_t status = pv_clock_init();
    if (status == ZX_OK) {
      invariant_tsc = pv_clock_is_stable();
    } else {
      has_pv_clock = false;
    }
  }

  bool has_hpet = hpet_is_present();
  if (has_hpet) {
    calibration_clock = CLOCK_HPET;
    const uint64_t hpet_ms_rate = hpet_ticks_per_ms();
    ASSERT(hpet_ms_rate <= UINT32_MAX);
    printf("HPET frequency: %" PRIu64 " ticks/ms\n", hpet_ms_rate);
    fp_32_64_div_32_32(&ns_per_hpet, 1000 * 1000, static_cast<uint32_t>(hpet_ms_rate));
  } else {
    calibration_clock = CLOCK_PIT;
  }

  bool force_wallclock = BootOptions::Get()->x86_wallclock != WallclockType::kAutoDetect;
  bool use_invariant_tsc =
      invariant_tsc &&
      (!force_wallclock || BootOptions::Get()->x86_wallclock == WallclockType::kTsc);

  use_tsc_deadline = use_invariant_tsc && x86_feature_test(X86_FEATURE_TSC_DEADLINE);
  if (use_tsc_deadline) {
    apic_timer_tsc_deadline_init();
  } else {
    calibrate_apic_timer();
  }

  if (use_invariant_tsc) {
    calibrate_tsc(has_pv_clock);

    // Program PIT in the software strobe configuration, but do not load
    // the count.  This will pause the PIT.
    outp(I8253_CONTROL_REG, 0x38);

    // Set up our wall clock to rdtsc, and stash the initial
    // transformation from ticks to clock monotonic.
    //
    // We cannot (or at least, really should not) reset the TSC to zero, so
    // instead we use the time of clock selection ("now" according to the TSC)
    // to define the zero point on our ticks timeline moving forward.
    timer_set_ticks_to_time_ratio(rdtsc_ticks_to_clock_monotonic);
    timer_set_initial_ticks(static_cast<uint64_t>(current_ticks_rdtsc()));

    // A note about this casting operation.  There is a technical risk of UB
    // here, in the case that -mono_ticks_offset is a value too large to
    // fit into a signed 64 bit integer.  UBSAN builds _might_ technically
    // assert if the value -mono_ticks_offset is >= 2^63 during this
    // cast.
    //
    // This _should_ never happen, however.  This offset is the two's compliment
    // of what the TSC read when we decided that the ticks timeline should be
    // zero.  For -mono_ticks_offset to be >= 2^63, the TSC counter
    // value itself would have needed to be >= 2^63 in the line above where it
    // was sampled.  Assuming that the TSC started to count from 0 at cold power
    // on time, and assuming that the TSC was running extremely quickly (say,
    // 5GHz), the system would have needed to be powered on for at least ~58.45
    // years before we hit this mark (and this assumes that the TSC is not reset
    // during a warm reboot, or that no warm reboots take place over almost 60
    // years of uptime).  So, for now, we perform the cast and take
    // the risk, assuming that nothing bad will happen.
    early_ticks_to_ticks =
        affine::Transform{static_cast<int64_t>(-timer_get_mono_ticks_offset()), 0, {1, 1}};
    wall_clock = CLOCK_TSC;
  } else {
    if (constant_tsc || invariant_tsc) {
      // Calibrate the TSC even though it's not as good as we want, so we
      // can still let folks still use it for cheap timing.
      calibrate_tsc(has_pv_clock);
    }

    if (has_hpet &&
        (!force_wallclock || BootOptions::Get()->x86_wallclock == WallclockType::kHpet)) {
      // Set up our wall clock to the HPET, and stash the initial
      // transformation from ticks to clock monotonic.
      timer_set_ticks_to_time_ratio(hpet_ticks_to_clock_monotonic);
      timer_set_initial_ticks(0);

      // Explicitly set the value of the HPET to zero, then make sure it is
      // started.  Take a correspondence pair between HPET and TSC by observing
      // TSC after we start the HPET so we can define the transformation between
      // TSC (the EarlyTicks reference) and HPET.
      //
      // Note: we do not bother to bracket the observation of HPET with a TSC
      // observation before and after.  We are at a point in the boot where we
      // are running on a single core, and should not be taking exceptions or
      // interrupts yet.  TL;DR, this observation should be "good enough"
      // without any need for averaging.
      hpet_set_value(0);
      hpet_enable();
      const zx_ticks_t tsc_reference = current_ticks_rdtsc();

      // Now set up our transformation from EarlyTicks (using TSC as a
      // reference) and HPET (the reference for the zx_ticks_get timeline).
      affine::Ratio rdtsc_ticks_to_hpet_ticks =
          affine::Ratio::Product(rdtsc_ticks_to_clock_monotonic,
                                 hpet_ticks_to_clock_monotonic.Inverse(), affine::Ratio::Exact::No);
      early_ticks_to_ticks = affine::Transform{tsc_reference, 0, rdtsc_ticks_to_hpet_ticks};

      // HPET is now our chosen "ticks" reference.
      wall_clock = CLOCK_HPET;
    } else {
      if (force_wallclock && BootOptions::Get()->x86_wallclock != WallclockType::kPit) {
        panic("Could not satisfy kernel.wallclock choice\n");
      }

      // Set up our wall clock to pit, and stash the initial
      // transformation from ticks to clock monotonic.
      timer_set_ticks_to_time_ratio({1'000'000, 1});

      set_pit_frequency(desired_pit_frequency);

      uint32_t irq = apic_io_isa_to_global(ISA_IRQ_PIT);
      zx_status_t status =
          register_permanent_int_handler(irq, interrupt_handler_t{nullptr, &pit_timer_tick});
      DEBUG_ASSERT(status == ZX_OK);
      unmask_interrupt(irq);

      // See the HPET code above.  Observe the value of TSC as we figure out the
      // PIT offset so that we can define a function which maps EarlyTicks to
      // ticks.
      timer_set_initial_ticks(static_cast<uint64_t>(current_ticks_pit()));
      const zx_ticks_t tsc_reference = current_ticks_rdtsc();

      affine::Ratio rdtsc_ticks_to_pit_ticks = affine::Ratio::Product(
          rdtsc_ticks_to_clock_monotonic, affine::Ratio{1, 1'000'000}, affine::Ratio::Exact::No);

      // Note, see the comment above in the TSC section for why it is considered
      // to be reasonably safe to perform the static cast from unsigned to
      // signed here.
      early_ticks_to_ticks =
          affine::Transform{tsc_reference, static_cast<int64_t>(-timer_get_mono_ticks_offset()),
                            rdtsc_ticks_to_pit_ticks};

      // PIT is now our chosen "ticks" reference.
      wall_clock = CLOCK_PIT;
    }
  }

  // Now that we've decided on which wall_clock to use as our timer reference, set up the ratio
  // that converts from reference timer ticks to APIC ticks.
  switch (wall_clock) {
    case CLOCK_UNSELECTED:
      panic("Wall clock was unselected by the time pc_init_timer completed\n");
      break;
    case CLOCK_TSC:
      ASSERT(tsc_ticks_per_ms <= UINT32_MAX);
      reference_timer_ticks_to_apic_ticks = {apic_ticks_per_ms,
                                             static_cast<uint32_t>(tsc_ticks_per_ms)};
      break;
    case CLOCK_HPET: {
      const uint64_t hpet_ticks_ms = hpet_ticks_per_ms();
      ASSERT(hpet_ticks_ms <= UINT32_MAX);
      reference_timer_ticks_to_apic_ticks = {apic_ticks_per_ms,
                                             static_cast<uint32_t>(hpet_ticks_ms)};
      break;
    }
    case CLOCK_PIT: {
      // Here's how we computed the ms_per_pit ratio:
      //
      // count = INTERNAL_FREQ_3X/desired_pit_frequency
      // ms/pit = (3000 * count) / INTERNAL_FREQ_3X
      //        = (3000 * (INTERNAL_FREQ_3X/desired_pit_frequency))/ INTERNAL_FREQ_3X
      //        = (3000 * INTERNAL_FREQ_3X) / (desired_pit_frequency * INTERNAL_FREQ_3X)
      //        = 3000/desired_pit_frequency
      const affine::Ratio ms_per_pit = {3000, desired_pit_frequency};
      const affine::Ratio apic_per_ms = {apic_ticks_per_ms, 1};
      reference_timer_ticks_to_apic_ticks = affine::Ratio::Product(apic_per_ms, ms_per_pit);
      break;
    }
    default:
      PANIC_UNIMPLEMENTED;
  }

  printf("timer features: constant_tsc %d invariant_tsc %d tsc_deadline %d\n", constant_tsc,
         invariant_tsc, use_tsc_deadline);
  printf("Using %s as wallclock\n", clock_name[wall_clock]);
}
LK_INIT_HOOK(timer, &pc_init_timer, LK_INIT_LEVEL_VM + 3)

// Converts the given duration's units from the platform's selected tick source to APIC ticks.
uint64_t apic_ticks_from_platform_ticks(zx_duration_t interval) {
  DEBUG_ASSERT(wall_clock != CLOCK_UNSELECTED);
  DEBUG_ASSERT(wall_clock != CLOCK_COUNT);
  return reference_timer_ticks_to_apic_ticks.Scale<affine::Ratio::Round::Up>(interval);
}

zx_status_t platform_set_oneshot_timer(zx_ticks_t deadline) {
  DEBUG_ASSERT(arch_ints_disabled());
  // We use 1 tick as the minimum deadline here because we want a deadline that immediately fires
  // the timer, but we can't use 0 because setting a TSC deadline to zero disables the APIC timer.
  deadline = ktl::max<zx_ticks_t>(deadline, 1);

  if (use_tsc_deadline) {
    LTRACEF("Scheduling oneshot timer: %" PRIi64 " deadline\n", deadline);
    apic_timer_set_tsc_deadline(deadline);
    kcounter_add(platform_timer_set_counter, 1);
    return ZX_OK;
  }

  const zx_ticks_t now = platform_current_raw_ticks();
  if (now >= deadline) {
    // Deadline has already passed. We still need to schedule a timer so that
    // the interrupt fires.
    LTRACEF("Scheduling oneshot timer for min duration\n");
    kcounter_add(platform_timer_set_counter, 1);
    return apic_timer_set_oneshot(1, 1, false /* unmasked */);
  }

  const zx_duration_t interval = zx_ticks_sub_ticks(deadline, now);
  DEBUG_ASSERT(interval > 0);

  // Convert the interval, which is in platform reference timer ticks, to APIC timer ticks.
  const uint64_t apic_ticks_needed = apic_ticks_from_platform_ticks(interval);
  DEBUG_ASSERT(apic_ticks_needed > 0);

  // Find the shift needed for this timeout, since count is 32-bit.
  const auto highest_set_bit = static_cast<uint32_t>(log2_ulong_floor(apic_ticks_needed));
  uint8_t extra_shift = (highest_set_bit <= 31) ? 0 : static_cast<uint8_t>(highest_set_bit - 31);
  if (extra_shift > 8) {
    extra_shift = 8;
  }

  uint32_t divisor = apic_divisor << extra_shift;
  uint32_t count;
  // If the divisor is too large, we're at our maximum timeout.  Saturate the
  // timer.  It'll fire earlier than requested, but the scheduler will notice
  // and ask us to set the timer up again.
  if (divisor <= 128) {
    count = (uint32_t)(apic_ticks_needed >> extra_shift);
    DEBUG_ASSERT((apic_ticks_needed >> extra_shift) <= UINT32_MAX);
  } else {
    divisor = 128;
    count = UINT32_MAX;
  }

  // Make sure we're not underflowing
  if (count == 0) {
    DEBUG_ASSERT(divisor == 1);
    count = 1;
  }

  LTRACEF("Scheduling oneshot timer: %u count, %u div\n", count, divisor);
  kcounter_add(platform_timer_set_counter, 1);
  return apic_timer_set_oneshot(count, static_cast<uint8_t>(divisor), false /* unmasked */);
}

void platform_stop_timer(void) {
  /* Enable interrupt mode that will stop the decreasing counter of the PIT */
  // outp(I8253_CONTROL_REG, 0x30);
  if (use_tsc_deadline) {
    // In TSC deadline mode, a deadline of 0 disarms the LAPIC timer
    apic_timer_set_tsc_deadline(0);
  } else {
    apic_timer_stop();
  }
  kcounter_add(platform_timer_cancel_counter, 1);
}

void platform_shutdown_timer(void) {
  DEBUG_ASSERT(arch_ints_disabled());

  if (x86_hypervisor_has_pv_clock() && arch_curr_cpu_num() == 0) {
    pv_clock_shutdown();
  }
}

zx_status_t platform_suspend_timer_curr_cpu() { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t platform_resume_timer_curr_cpu() { return ZX_ERR_NOT_SUPPORTED; }

zx_instant_mono_ticks_t platform_convert_early_ticks(arch::EarlyTicks sample) {
  return early_ticks_to_ticks.Apply(sample.tsc);
}

// Currently, usermode can access our source of ticks only if we have chosen TSC
// to be our tick counter.  Otherwise, they will need to go through a syscall.
//
// In theory, we can fix this, but it would require having the vDSO map some
// read-only memory in the user mode process (either the HPET registers, or the
// variable which represents the PIT timer).  Currently, doing this is not
// something we support, and the vast majority of x64 systems that we run on
// have an invariant TSC which is accessible from usermode.  For now, we just
// take the syscall hit instead of attempting to get more fancy.
bool platform_usermode_can_access_tick_registers(void) { return (wall_clock == CLOCK_TSC); }

extern "C" void cpp_hpet_set_ticks_to_clock_monotonic(uint32_t N, uint32_t D);
extern "C" void cpp_hpet_set_ticks_to_clock_monotonic(uint32_t N, uint32_t D) {
  hpet_ticks_to_clock_monotonic = affine::Ratio(N, D);
}
