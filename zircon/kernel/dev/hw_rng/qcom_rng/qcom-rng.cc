// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/time.h>

#include <arch/arm64/periphmap.h>
#include <dev/hw_rng.h>
#include <dev/hw_rng/qcom_rng/init.h>
#include <explicit-memory/bytes.h>
#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>
#include <kernel/mutex.h>
#include <kernel/thread.h>

namespace {

DECLARE_SINGLETON_MUTEX(QcomRngLock);

class StatusRegister : public hwreg::RegisterBase<StatusRegister, uint32_t> {
 public:
  DEF_BIT(0, data_ready);

  static auto Get() { return hwreg::RegisterAddr<StatusRegister>(0x4); }
};

class DataRegister : public hwreg::RegisterBase<DataRegister, uint32_t> {
 public:
  DEF_FIELD(31, 0, data);

  static auto Get() { return hwreg::RegisterAddr<DataRegister>(0x0); }
};

constexpr zx_duration_t kRetryDelay = ZX_USEC(440);
constexpr int kMaxRetries = 5;

static vaddr_t mmio_base = 0;

size_t GetEntropy(void* buf, size_t len) {
  Guard<Mutex> guard(QcomRngLock::Get());
  ktl::span<ktl::byte> buffer = {reinterpret_cast<ktl::byte*>(buf), len};
  hwreg::RegisterMmio mmio(reinterpret_cast<void*>(mmio_base));
  uint32_t data = 0;
  // RNG data that exists in the current stack frame must be scrubbed before returning, preventing
  // any sort of "leaks". `explicit_memory` ensures none of these cleanup operations are elided by
  // the compiler.
  explicit_memory::ZeroDtor scrubber(&data, 1);
  int retries = 0;
  while (!buffer.empty()) {
    auto status_register = StatusRegister::Get().ReadFrom(&mmio);
    // When the HW RNG unit runs out of data, it must produce more. This bit is set when the unit
    // has produced data, and it can be read from the `DataRegister`.
    if (!status_register.data_ready()) {
      if (retries++ == kMaxRetries) {
        // We failed to generate enough random data, scrub everything and bail out.
        mandatory_memset(buf, 0, len);
        return 0;
      }
      Thread::Current::SleepRelative(kRetryDelay);
      continue;
    }

    data = DataRegister::Get().ReadFrom(&mmio).data();
    size_t copy_size = ktl::min(buffer.size(), sizeof(data));
    memcpy(buffer.data(), &data, copy_size);
    buffer = buffer.subspan(copy_size);
  }

  // Return the number of bytes provided.
  return len - buffer.size();
}

const struct hw_rng_ops kOps = {
    .hw_rng_get_entropy = &GetEntropy,
};

}  // namespace

void QcomRngInit(const zbi_dcfg_qcom_rng_t& config) {
  // Check that the HWRNG unit has been handed off ready for use, otherwise complain and move on.
  if ((config.flags & ZBI_QCOM_RNG_FLAGS_ENABLED) == 0) {
    printf("QCOM HWRNG: Handed off disabled.\n");
    return;
  }
  mmio_base = periph_paddr_to_vaddr(config.mmio_phys);
  hw_rng_register(&kOps);
  printf("QCOM HWRNG: Registered {mmio=%zu,flags=%zu}.\n", config.mmio_phys, config.mmio_phys);
}
