// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_AML_CLK_H_
#define SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_AML_CLK_H_

#include <fidl/fuchsia.hardware.clock.measure/cpp/wire.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.clockimpl/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.clockimpl/cpp/natural_types.h>
#include <lib/ddk/io-buffer.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/syscalls/smc.h>

#include <fbl/array.h>
#include <fbl/mutex.h>
#include <hwreg/mmio.h>
#include <soc/aml-a1/a1-hiu.h>
#include <soc/aml-a5/a5-hiu.h>
#include <soc/aml-s905d2/s905d2-hiu.h>

#include "aml-clk-blocks.h"
#include "meson-cpu-clock.h"
#include "meson-pll-clock.h"
#include "meson-rate-clock.h"

namespace amlogic_clock {

class AmlClock : public fdf::DriverBase2,
                 public fdf::WireServer<fuchsia_hardware_clockimpl::ClockImpl>,
                 public fidl::WireServer<fuchsia_hardware_clock_measure::Measurer> {
 public:
  static constexpr char kDriverName[] = "aml-clk";
  static constexpr char kChildNodeName[] = "clocks";
  // MMIO Indexes
  static constexpr uint32_t kHiuMmio = 0;
  static constexpr uint32_t kDosbusMmio = 1;
  static constexpr uint32_t kMsrMmio = 2;
  static constexpr uint32_t kCpuCtrlMmio = 3;

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(AmlClock);
  AmlClock() : fdf::DriverBase2(kDriverName) {}
  ~AmlClock() override;

  zx::result<> Start(fdf::DriverContext context) override;

  // CLK protocol implementation.
  void Enable(EnableRequestView request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override;
  void Disable(DisableRequestView request, fdf::Arena& arena,
               DisableCompleter::Sync& completer) override;
  void IsEnabled(IsEnabledRequestView request, fdf::Arena& arena,
                 IsEnabledCompleter::Sync& completer) override;
  void SetRate(SetRateRequestView request, fdf::Arena& arena,
               SetRateCompleter::Sync& completer) override;
  void QuerySupportedRate(QuerySupportedRateRequestView request, fdf::Arena& arena,
                          QuerySupportedRateCompleter::Sync& completer) override;
  void GetRate(GetRateRequestView request, fdf::Arena& arena,
               GetRateCompleter::Sync& completer) override;
  void SetInput(SetInputRequestView request, fdf::Arena& arena,
                SetInputCompleter::Sync& completer) override;
  void GetNumInputs(GetNumInputsRequestView request, fdf::Arena& arena,
                    GetNumInputsCompleter::Sync& completer) override;
  void GetInput(GetInputRequestView request, fdf::Arena& arena,
                GetInputCompleter::Sync& completer) override;
  void GetClockProperties(fdf::Arena& arena, GetClockPropertiesCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_clockimpl::ClockImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unexpected clockimpl FIDL call: 0x{:x}", metadata.method_ordinal);
  }

  // CLK FIDL implementation.
  void Measure(MeasureRequestView request, MeasureCompleter::Sync& completer) override;
  void GetCount(GetCountCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_clock_measure::Measurer> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unexpected Clock FIDL call: 0x{:x}", metadata.method_ordinal);
  }

 private:
  // Called by devfs_connector_ when a client connects.
  void DevfsConnect(fidl::ServerEnd<fuchsia_hardware_clock_measure::Measurer> request);

  zx_status_t PopulateRegisterBlocks(uint32_t device_id, fdf::PDev& pdev);

  zx_status_t InitChildNode();

  // Toggle clocks enable bit.
  zx_status_t ClkToggle(uint32_t id, bool enable);
  void ClkToggleHw(const meson_clk_gate_t* gate, bool enable) __TA_REQUIRES(lock_);

  // Clock measure helper API.
  zx_status_t ClkMeasureUtil(uint32_t id, uint64_t* clk_freq);

  // Toggle enable bit for PLL clocks.
  zx_status_t ClkTogglePll(uint32_t id, bool enable);

  // Checks the preconditions for SetInput, GetNumInputs and GetInput and
  // returns ZX_OK if the preconditions are met.
  zx_status_t IsSupportedMux(uint32_t id, uint16_t supported_mask);

  // Find the MesonRateClock that corresponds to clk. If ZX_OK is returned
  // `out` is populated with a pointer to the target clock.
  zx_status_t GetMesonRateClock(uint32_t id, MesonRateClock** out);

  void InitHiu();

  void InitHiuA5();

  void InitHiuA1();

  // IO MMIO
  std::optional<fdf::MmioBuffer> hiu_mmio_;
  std::optional<fdf::MmioBuffer> dosbus_mmio_;
  std::optional<fdf::MmioBuffer> msr_mmio_;
  std::optional<fdf::MmioBuffer> cpuctrl_mmio_;

  // Protects clock gate registers.
  // Clock gates.
  fbl::Mutex lock_;
  const meson_clk_gate_t* gates_ = nullptr;
  size_t gate_count_ = 0;
  std::vector<uint32_t> meson_gate_enable_count_;

  // Clock muxes.
  const meson_clk_mux_t* muxes_ = nullptr;
  size_t mux_count_ = 0;

  // Cpu Clocks.
  std::vector<MesonCpuClock> cpu_clks_;

  std::optional<fdf::MmioBuffer> hiudev_;
  std::vector<MesonPllClock> pllclk_;
  size_t pll_count_ = HIU_PLL_COUNT;

  // Clock Table
  const char* const* clk_table_ = nullptr;
  size_t clk_table_count_ = 0;
  // MSR_CLK offsets/
  meson_clk_msr_t clk_msr_offsets_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> child_node_controller_;
  driver_devfs::Connector<fuchsia_hardware_clock_measure::Measurer> devfs_connector_{
      fit::bind_member<&AmlClock::DevfsConnect>(this)};
  fidl::ServerBindingGroup<fuchsia_hardware_clock_measure::Measurer> measurer_binding_group_;
  fdf::ServerBindingGroup<fuchsia_hardware_clockimpl::ClockImpl> clock_impl_binding_group_;

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  fdf_metadata::MetadataServer<fuchsia_hardware_clockimpl::ClockIdsMetadata>
      clock_ids_metadata_server_;
#endif
  fdf_metadata::MetadataServer<fuchsia_hardware_clockimpl::InitMetadata>
      clock_init_metadata_server_;
};

}  // namespace amlogic_clock

#endif  // SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_AML_CLK_H_
