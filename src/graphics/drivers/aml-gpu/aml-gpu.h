// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_DRIVERS_AML_GPU_AML_GPU_H_
#define SRC_GRAPHICS_DRIVERS_AML_GPU_AML_GPU_H_

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.gpu.mali/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.registers/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>

#include <memory>
#include <optional>

constexpr uint32_t kPwrKey = 0x14;
constexpr uint32_t kPwrOverride1 = 0x16;

constexpr uint32_t kClkEnabledBitShift = 8;

static inline uint32_t CalculateClockMux(bool enabled, uint32_t base, uint32_t divisor) {
  return (enabled << kClkEnabledBitShift) | (base << 9) | (divisor - 1);
}

constexpr uint32_t kClockMuxMask = 0xfff;
constexpr uint32_t kMaxGpuClkFreq = 6;
constexpr uint32_t kFinalMuxBitShift = 31;
constexpr uint32_t kClockInputs = 8;

constexpr uint32_t kMmioGpuIndex = 0;
constexpr uint32_t kMmioHiuIndex = 1;

typedef struct {
  // Byte offsets of the reset registers in the reset mmio region.
  uint32_t reset0_level_offset;
  uint32_t reset0_mask_offset;
  uint32_t reset2_level_offset;
  uint32_t reset2_mask_offset;
  // Offset of the Mali control register in the hiubus, in units of dwords.
  uint32_t hhi_clock_cntl_offset;
  // THe index into gpu_clk_freq that will be used upon booting.
  uint32_t initial_clock_index;
  // True if the driver needs to use GP0.
  bool enable_gp0;
  // Initial clock index to use if initializing GP0 fails.
  uint32_t non_gp0_index;
  // Map from the clock index to the mux source to use.
  uint32_t gpu_clk_freq[kMaxGpuClkFreq];
  // Map from the mux source to the frequency in Hz.
  uint32_t input_freq_map[kClockInputs];
} aml_gpu_block_t;

namespace aml_gpu {
class TestAmlGpu;

class AmlGpu final : public fdf::DriverBase2,
                     public fdf::WireServer<fuchsia_hardware_gpu_mali::ArmMali> {
 public:
  explicit AmlGpu();

  ~AmlGpu();

  zx::result<> Start(fdf::DriverContext context) override;

  void GetProperties(fdf::Arena& arena, GetPropertiesCompleter::Sync& completer) override;
  void EnterProtectedMode(fdf::Arena& arena, EnterProtectedModeCompleter::Sync& completer) override;
  void StartExitProtectedMode(fdf::Arena& arena,
                              StartExitProtectedModeCompleter::Sync& completer) override;
  void FinishExitProtectedMode(fdf::Arena& arena,
                               FinishExitProtectedModeCompleter::Sync& completer) override;

 private:
  friend class TestAmlGpu;

  zx_status_t Gp0Init(const fdf::Namespace& incoming);
  void InitClock();
  void SetClkFreqSource(int32_t clk_source);
  void SetInitialClkFreqSource(int32_t clk_source);
  zx_status_t SetProtected(uint32_t protection_mode);

  void UpdateClockProperties();

  fidl::Arena<> arena_;

  fuchsia_hardware_gpu_mali::wire::MaliProperties properties_;

  std::optional<fdf::MmioBuffer> hiu_buffer_;
  std::optional<fdf::MmioBuffer> gpu_buffer_;

  fidl::WireSyncClient<fuchsia_hardware_registers::Device> reset_register_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> gp0_clock_;
  // Resource used to perform SMC calls. Only needed on SM1.
  zx::resource secure_monitor_;

  aml_gpu_block_t* gpu_block_;
  std::optional<fdf::MmioBuffer> hiu_dev_;
  int32_t current_clk_source_ = -1;
  // bootstrap/driver_manager:root/aml-gpu
  inspect::Node root_;
  inspect::Inspector inspector_;
  fdf::SynchronizedDispatcher loop_dispatcher_;
  bool gp0_init_succeeded_ = false;

  inspect::UintProperty current_clk_source_property_;
  inspect::UintProperty current_clk_mux_source_property_;
  inspect::UintProperty current_clk_freq_hz_property_;
  inspect::IntProperty current_protected_mode_property_;
};
}  // namespace aml_gpu

#endif  // SRC_GRAPHICS_DRIVERS_AML_GPU_AML_GPU_H_
