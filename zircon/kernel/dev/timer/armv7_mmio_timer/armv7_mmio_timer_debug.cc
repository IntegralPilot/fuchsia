// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// #include <lib/root_resource_filter.h>

#include <inttypes.h>
#include <lib/console.h>

#include <arch/arm64/periphmap.h>
#include <dev/timer/armv7_mmio_timer.h>
#include <dev/timer/armv7_mmio_timer_registers.h>
#include <ktl/algorithm.h>
#include <ktl/array.h>
#include <ktl/bit.h>
#include <ktl/limits.h>
#include <ktl/string_view.h>
#include <ktl/unique_ptr.h>
#include <ktl/utility.h>
#include <lk/init.h>

namespace {

using namespace armv7_mmio_timer_registers;

template <typename T>
void DumpReg(const char* tag, const T& r) {
  using VT = T::ValueType;
  constexpr int kHexWidth = ktl::numeric_limits<VT>::digits / 4;

  if constexpr (T::PrinterEnabled::value) {
    static constexpr char kRawFieldName[] = "raw";
    size_t max_name_width = sizeof(kRawFieldName) - 1;
    uint32_t field_count = 0;

    r.ForEachField([&max_name_width, &field_count](const char* name, VT, uint32_t, uint32_t) {
      ++field_count;
      if (name != nullptr) {
        max_name_width = ktl::max(max_name_width, strlen(name));
      }
    });

    if (field_count > 0) {
      auto field_printer = [max_name_width](const char* name, VT val, uint32_t hi, uint32_t lo) {
        if (name) {
          const int width = static_cast<int>(max_name_width);
          printf("[%*" PRIu32 ":%*" PRIu32 "] : %*s : 0x%0*" PRIx64 "\n", width, hi, width, lo,
                 width, name, kHexWidth, static_cast<uint64_t>(val));
        }
      };

      field_printer(kRawFieldName, r.reg_value(), ktl::numeric_limits<VT>::digits - 1, 0);
      r.ForEachField(field_printer);
      putchar('\n');
      return;
    }
  }

  printf("%s : 0x%*" PRIx64 " (%" PRIu64 ")\n", tag, kHexWidth,
         static_cast<uint64_t>(r.reg_value()), static_cast<uint64_t>(r.reg_value()));
}

enum class SkipZeros { No, Yes };
template <typename RegType, SkipZeros kSkipZeros = SkipZeros::No>
void DumpRegArray(const char* name, hwreg::RegisterMmio& base) {
  for (uint32_t i = 0; i < RegType::kRegCount; ++i) {
    char tag[32];
    snprintf(tag, sizeof(tag), "%s[%u]", name, i);

    const auto r = RegType::Get(i).ReadFrom(&base);
    if ((kSkipZeros == SkipZeros::No) || (r.reg_value() != 0)) {
      DumpReg(tag, RegType::Get(i).ReadFrom(&base));
    }
  }
}

}  // namespace

int Armv7MmioTimer::Dump(uint8_t timer_mask) {
  if (!mmio_ctl_.base()) {
    printf("No ARMv7 MMIO Timer hardware was detected during initialization\n");
    return -1;
  }

  auto cnttidr = CNTCTLBase::CNTTIDR::Get().ReadFrom(&mmio_ctl_);
  if (timer_mask == 0xff) {
    printf("ARMv7 MMIO Timer CNTCTL\n");

    // FRQ and NSAR are only accessible from a secure context.  We _could_ print
    // them here, but they are all just going to read as zero.
    // DumpReg("CNTFRQ", CNTCTLBase::CNTFRQ::Get().ReadFrom(&mmio_ctl_));
    // DumpReg("CNTNSAR", CNTCTLBase::CNTNSAR::Get().ReadFrom(&mmio_ctl_));
    DumpReg("CNTTIDR", CNTCTLBase::CNTTIDR::Get().ReadFrom(&mmio_ctl_));
    DumpRegArray<CNTCTLBase::CNTACR>("CNTACR", mmio_ctl_);
    DumpRegArray<CNTCTLBase::CNTVOFF>("CNTVOFF", mmio_ctl_);
    DumpRegArray<CNTCTLBase::CounterID, SkipZeros::Yes>("CounterID", mmio_ctl_);
    printf("\n");
  }

  for (uint32_t i = 0; i < timers_.size(); ++i) {
    if (!(timer_mask & (1u << i))) {
      continue;
    }

    const bool has_impl = (cnttidr.reg_value() >> (i * 4)) & 0x1;
    const bool has_virt = (cnttidr.reg_value() >> (i * 4)) & 0x2;
    const bool has_el0 = (cnttidr.reg_value() >> (i * 4)) & 0x4;

    if (!has_impl) {
      continue;
    }

    if (timers_[i] == nullptr) {
      printf("Failed to discover timer frame %u\n", i);
      continue;
    }

    Armv7MmioTimer& timer = *timers_[i];
    printf("ARMv7 MMIO Timer Frame %u CNT\n", i);
    if (timer.pct_timer().supported()) {
      printf("Phys IRQ : %u\n", timer.pct_timer().irq());
    }
    if (timer.vct_timer().supported()) {
      printf("Virt IRQ : %u\n", timer.vct_timer().irq());
    }

    auto el1acr = CNTCTLBase::CNTACR::Get(i).ReadFrom(&mmio_ctl_);
    auto el0acr = CNTBase::CNTEL0ACR::Get().ReadFrom(&timer.mmio_);

    if (el1acr.RFRQ()) {
      DumpReg(" CNTFRQ", CNTBase::CNTFRQ::Get().ReadFrom(&timer.mmio_));
    }
    if (el1acr.RPCT()) {
      DumpReg(" CNTPCT", CNTBase::CNTPCT::Get().ReadFrom(&timer.mmio_));
    }
    if (has_virt && el1acr.RVCT()) {
      DumpReg(" CNTVCT", CNTBase::CNTVCT::Get().ReadFrom(&timer.mmio_));
    }
    if (has_virt && el1acr.RVOFF()) {
      DumpReg("CNTVOFF", CNTBase::CNTVOFF::Get().ReadFrom(&timer.mmio_));
    }
    DumpReg("CNTEL0ACR", el0acr);
    if (el1acr.RWPT()) {
      DumpReg("CNTP_CVAL", CNTBase::CNTP_CVAL::Get().ReadFrom(&timer.mmio_));
      DumpReg("CNTP_TVAL", CNTBase::CNTP_TVAL::Get().ReadFrom(&timer.mmio_));
      DumpReg("CNTP_CTL", CNTBase::CNTP_CTL::Get().ReadFrom(&timer.mmio_));
    }
    if (el1acr.RWVT() && has_virt) {
      DumpReg("CNTV_CVAL", CNTBase::CNTV_CVAL::Get().ReadFrom(&timer.mmio_));
      DumpReg("CNTV_TVAL", CNTBase::CNTV_TVAL::Get().ReadFrom(&timer.mmio_));
      DumpReg("CNTV_CTL", CNTBase::CNTV_CTL::Get().ReadFrom(&timer.mmio_));
    }
    DumpRegArray<CNTBase::CounterID, SkipZeros::Yes>("CounterID", timer.mmio_);
    printf("\n");

    if (has_el0) {
      if (!timer.el0_mmio_.base()) {
        printf("Failed to discover timer EL0 frame %u\n", i);
      } else {
        printf("ARMv7 MMIO Timer Frame %u EL0 CNT\n", i);
        if (el0acr.EL0PCTEN() | el0acr.EL0VCTEN()) {
          DumpReg("CNTFRQ", CNTEL0Base::CNTFRQ::Get().ReadFrom(&timer.el0_mmio_));
        }
        if (el0acr.EL0PCTEN()) {
          DumpReg("CNTPCT", CNTEL0Base::CNTPCT::Get().ReadFrom(&timer.el0_mmio_));
        }
        if (has_virt && el0acr.EL0VCTEN()) {
          DumpReg("CNTVCT", CNTEL0Base::CNTVCT::Get().ReadFrom(&timer.el0_mmio_));
        }
        if (el0acr.EL0PTEN()) {
          DumpReg("CNTP_CVAL", CNTEL0Base::CNTP_CVAL::Get().ReadFrom(&timer.el0_mmio_));
          DumpReg("CNTP_TVAL", CNTEL0Base::CNTP_TVAL::Get().ReadFrom(&timer.el0_mmio_));
          DumpReg("CNTP_CTL", CNTEL0Base::CNTP_CTL::Get().ReadFrom(&timer.el0_mmio_));
        }
        if (has_virt && el0acr.EL0VTEN()) {
          DumpReg("CNTV_CVAL", CNTEL0Base::CNTV_CVAL::Get().ReadFrom(&timer.el0_mmio_));
          DumpReg("CNTV_TVAL", CNTEL0Base::CNTV_TVAL::Get().ReadFrom(&timer.el0_mmio_));
          DumpReg("CNTV_CTL", CNTEL0Base::CNTV_CTL::Get().ReadFrom(&timer.el0_mmio_));
        }
        DumpRegArray<CNTEL0Base::CounterID, SkipZeros::Yes>("CounterID", timer.el0_mmio_);
        printf("\n");
      }
    }
  }

  return 0;
}

int Armv7MmioTimer::ShowStatus(uint8_t timer_mask) {
  for (uint32_t i = 0; i < Armv7MmioTimer::kMaxTimers; ++i) {
    if (timer_mask & (1u << i)) {
      const Armv7MmioTimer* timer = Armv7MmioTimer::Get(i);
      if (timer == nullptr) {
        if (ktl::popcount(timer_mask) == 1) {
          printf("Timer[%u] : does not exist\n", i);
        }
        continue;
      }

      if (!timer->pct_timer_.supported() && !timer->vct_timer_.supported()) {
        printf("Timer[%u] : no timers supported\n", i);
        continue;
      }

      ktl::array pvtimers{&timer->pct_timer(), &timer->vct_timer()};
      for (const Armv7MmioTimer::Timer* t : pvtimers) {
        if (t->supported()) {
          zx::result<zx_duration_t> res = t->TimeUntilDeadline();
          if (res.is_ok()) {
            printf("Timer[%u] : %s %lu fires in %ld.%06ld sec\n", i, t->type_name(), t->ticks(),
                   res.value() / ZX_SEC(1), (res.value() % ZX_SEC(1)) / 1000);
          } else {
            printf("Timer[%u] : %s %lu <disabled>\n", i, t->type_name(), t->ticks());
          }
        }
      }
    }
  }

  return 0;
}

static uint8_t mask_from_arg(const cmd_args& arg) {
  if (!strcmp(arg.str, "all")) {
    return 0xFF;
  } else if (arg.u < Armv7MmioTimer::kMaxTimers) {
    return static_cast<uint8_t>(0x1 << arg.u);
  }
  return 0;
}

static int cmd_av7t(int argc, const cmd_args* argv, uint32_t flags) {
  auto usage = [&argv]() -> int {
    printf("usage:\n");
    printf("%s dump <all|0-7>\n", argv[0].str);
    printf("%s status <all|0-7>\n", argv[0].str);
    printf("%s set <0-7> (pct|vct) <msec>\n", argv[0].str);
    return ZX_OK;
  };

  if (argc < 2) {
    return usage();
  }

  if (!strcmp(argv[1].str, "dump")) {
    const uint8_t timer_mask = (argc == 3) ? mask_from_arg(argv[2]) : 0;
    return timer_mask != 0 ? Armv7MmioTimer::Dump(timer_mask) : usage();
  } else if (!strcmp(argv[1].str, "status")) {
    const uint8_t timer_mask = (argc == 3) ? mask_from_arg(argv[2]) : 0;
    return timer_mask != 0 ? Armv7MmioTimer::ShowStatus(timer_mask) : usage();
  } else if (!strcmp(argv[1].str, "set")) {
    if (argc != 5) {
      printf("Bad argument count %d for set command", argc);
      return usage();
    }

    if (argv[2].u >= Armv7MmioTimer::kMaxTimers) {
      printf("Bad timer index %lu\n", argv[2].u);
      return usage();
    }

    Armv7MmioTimer::Type type;
    if (!strcmp(argv[3].str, "pct")) {
      type = Armv7MmioTimer::Type::PCT;
    } else if (!strcmp(argv[3].str, "vct")) {
      type = Armv7MmioTimer::Type::VCT;
    } else {
      printf("Unrecognized timer type \"%s\"\n", argv[3].str);
      return usage();
    }

    Armv7MmioTimer* timer = Armv7MmioTimer::Get(argv[2].u);
    if (timer == nullptr) {
      printf("Timer %lu not found\n", argv[2].u);
      return ZX_OK;
    }

    Armv7MmioTimer::Timer& inner_timer =
        (type == Armv7MmioTimer::Type::PCT) ? timer->pct_timer() : timer->vct_timer();

    if (!inner_timer.supported()) {
      printf("Timer %lu:%s is not supported.\n", argv[2].u, argv[3].str);
      return ZX_ERR_NOT_SUPPORTED;
    }

    zx_status_t status = inner_timer.SetHandler([&inner_timer]() {
      printf("%s Timer FIRED (IRQ %u)!\n", inner_timer.type_name(), inner_timer.irq());
      return Armv7MmioTimer::IrqHandlerResult::Unregister;
    });

    if (status != ZX_OK) {
      printf("Timer %lu:%s is busy!\n", argv[2].u, argv[3].str);
      return status;
    }

    printf("Setting timer %lu:%s to fire in %lu mSec\n", argv[2].u, argv[3].str, argv[4].u);
    status = inner_timer.SetRelativeTimer(ZX_MSEC(argv[4].u));

    if (status != ZX_OK) {
      printf("Failed to set timer %lu:%s (err = %d)\n", argv[2].u, argv[3].str, status);
    }
    return status;
  }

  printf("unknown command\n");
  return usage();
}

STATIC_COMMAND_START
STATIC_COMMAND_MASKED("av7t", "ARMv7 Timer Command", &cmd_av7t, CMD_AVAIL_ALWAYS)
STATIC_COMMAND_END(suspend)
