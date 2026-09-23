// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_AML_SDMMC_AML_SDMMC_H_
#define SRC_DEVICES_BLOCK_DRIVERS_AML_SDMMC_AML_SDMMC_H_

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/driver/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/driver/power/cpp/suspend.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/stdcompat/span.h>
#include <lib/trace/event.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/compiler.h>

#include <array>
#include <limits>
#include <mutex>
#include <vector>

#include <soc/aml-common/aml-sdmmc.h>

#include "src/devices/block/drivers/aml-sdmmc/aml_sdmmc_config.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace aml_sdmmc {

class AmlSdmmc : public fdf::DriverBase2,
                 public fdf::WireServer<fuchsia_hardware_sdmmc::Sdmmc>,
                 public fidl::Server<fuchsia_hardware_power::PowerTokenProvider>,
                 public fdf_power::Suspendable<AmlSdmmc> {
 public:
  // Note: This name can't be changed without migrating users in other repos.
  static constexpr char kDriverName[] = "aml-sd-emmc";

  // Limit maximum number of descriptors to 512 for now
  static constexpr size_t kMaxDmaDescriptors = 512;

  // Levels for hardware power element.
  static constexpr fuchsia_power_broker::PowerLevel kPowerLevelOff = 0;
  static constexpr fuchsia_power_broker::PowerLevel kPowerLevelOn = 1;

  explicit AmlSdmmc()
      : fdf::DriverBase2(kDriverName),
        registered_vmos_{
            // clang-format off
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            SdmmcVmoStore{vmo_store::Options{}},
            // clang-format on
        } {}

  ~AmlSdmmc() override {
    if (irq_.is_valid()) {
      irq_.destroy();
    }
  }

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) __TA_EXCLUDES(lock_) override;

  // fuchsia_hardware_sdmmc::Sdmmc implementation
  void HostInfo(fdf::Arena& arena, HostInfoCompleter::Sync& completer) override;
  void SetSignalVoltage(SetSignalVoltageRequestView request, fdf::Arena& arena,
                        SetSignalVoltageCompleter::Sync& completer) override;
  void SetBusWidth(SetBusWidthRequestView request, fdf::Arena& arena,
                   SetBusWidthCompleter::Sync& completer) override;
  void SetBusFreq(SetBusFreqRequestView request, fdf::Arena& arena,
                  SetBusFreqCompleter::Sync& completer) override;
  void SetTiming(SetTimingRequestView request, fdf::Arena& arena,
                 SetTimingCompleter::Sync& completer) override;
  void HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) override;
  void PerformTuning(PerformTuningRequestView request, fdf::Arena& arena,
                     PerformTuningCompleter::Sync& completer) override;
  void RegisterInBandInterrupt(RegisterInBandInterruptRequestView request, fdf::Arena& arena,
                               RegisterInBandInterruptCompleter::Sync& completer) override;
  void AckInBandInterrupt(fdf::Arena& arena,
                          AckInBandInterruptCompleter::Sync& completer) override {}
  void RegisterVmo(RegisterVmoRequestView request, fdf::Arena& arena,
                   RegisterVmoCompleter::Sync& completer) override;
  void UnregisterVmo(UnregisterVmoRequestView request, fdf::Arena& arena,
                     UnregisterVmoCompleter::Sync& completer) override;
  void Request(RequestRequestView request, fdf::Arena& arena,
               RequestCompleter::Sync& completer) override;

  void EnableCqhci(fdf::Arena& arena, EnableCqhciCompleter::Sync& completer) override;
  void DisableCqhci(fdf::Arena& arena, DisableCqhciCompleter::Sync& completer) override;
  void InitializeCommandQueueing(InitializeCommandQueueingRequestView request, fdf::Arena& arena,
                                 InitializeCommandQueueingCompleter::Sync& completer) override;

  // fuchsia_hardware_power::PowerTokenProvider implementation
  void GetToken(GetTokenCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power::PowerTokenProvider> md,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  zx_status_t SuspendPower() __TA_REQUIRES(lock_);
  zx_status_t ResumePower() __TA_REQUIRES(lock_);
  // fdf_power::Suspendable implementation
  void Suspend(fdf_power::SuspendCompleter completer) override;
  void Resume(fdf_power::ResumeCompleter completer) override;
  bool SuspendEnabled() override;
  // Used by fdf_power::Suspendable.
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> take_power_element_runner() {
    return std::move(power_element_runner_);
  }



  // Visible for tests
  zx_status_t Init(const std::string& instance_identifier) __TA_EXCLUDES(lock_);

 protected:
  std::optional<inspect::ComponentInspector> component_inspector_;
  virtual zx_status_t WaitForInterruptImpl();
  virtual void WaitForBus() const __TA_REQUIRES(lock_);
  virtual std::optional<compat::DeviceServer::BanjoConfig> get_banjo_config() {
    return std::nullopt;
  }

  zx_status_t SetBusWidthImpl(fuchsia_hardware_sdmmc::wire::SdmmcBusWidth bus_width)
      __TA_REQUIRES(lock_);
  zx_status_t SetBusFreqImpl(uint32_t freq) __TA_REQUIRES(lock_);
  zx_status_t SetTimingImpl(fuchsia_hardware_sdmmc::wire::SdmmcTiming timing) __TA_REQUIRES(lock_);
  zx_status_t HwResetImpl() __TA_REQUIRES(lock_);
  zx_status_t PerformTuningImpl(uint32_t tuning_cmd_idx) __TA_REQUIRES(tuning_lock_);
  zx_status_t RequestImpl(const fuchsia_hardware_sdmmc::wire::SdmmcReq& req,
                          uint32_t out_response[4]) __TA_REQUIRES(lock_);
  zx_status_t RegisterVmoImpl(uint32_t vmo_id, uint8_t client_id, zx::vmo vmo, uint64_t offset,
                              uint64_t size, fuchsia_hardware_sdmmc::wire::SdmmcVmoRight rights)
      __TA_EXCLUDES(lock_);
  zx_status_t UnregisterVmoImpl(uint32_t vmo_id, uint8_t client_id, zx::vmo* out_vmo)
      __TA_EXCLUDES(lock_);

  // Visible for tests
  const zx::bti& bti() const { return bti_; }
  const fdf::MmioBuffer& mmio() __TA_EXCLUDES(lock_) {
    std::lock_guard<std::mutex> lock(lock_);
    return *mmio_;
  }
  void* descs_buffer() __TA_EXCLUDES(lock_) {
    std::lock_guard<std::mutex> lock(lock_);
    return descs_buffer_->virt();
  }

  fuchsia_hardware_sdmmc::wire::SdmmcHostInfo dev_info_;
  bool power_suspended_ __TA_GUARDED(lock_) = false;

  // TODO(https://fxbug.dev/42084501): Remove redundant locking when Banjo is removed.
  std::mutex lock_ __TA_ACQUIRED_AFTER(tuning_lock_);
  std::mutex tuning_lock_ __TA_ACQUIRED_BEFORE(lock_);

 private:
  constexpr static size_t kResponseCount = 4;

  struct TuneResults {
    uint64_t results = 0;

    std::string ToString(const uint32_t param_max) const {
      std::vector<char> string(param_max + 1);
      for (uint32_t i = 0; i <= param_max; i++) {
        string[i] = (results & (1ULL << i)) ? '|' : '-';
      }
      return std::string(string.data(), string.size());
    }
  };

  struct TuneWindow {
    uint32_t start = 0;
    uint32_t size = 0;

    uint32_t middle() const { return start + (size / 2); }
  };

  struct TuneSettings {
    uint32_t adj_delay = 0;
    uint32_t delay = 0;
  };

  // VMO metadata that needs to be stored in accordance with the SDMMC protocol.
  struct OwnedVmoInfo {
    uint64_t offset;
    uint64_t size;
    fuchsia_hardware_sdmmc::SdmmcVmoRight rights;
  };

  struct Inspect {
    inspect::Node root;
    inspect::UintProperty bus_clock_frequency;
    inspect::UintProperty adj_delay;
    inspect::UintProperty delay_lines;
    std::vector<inspect::Node> tuning_results_nodes;
    std::vector<inspect::StringProperty> tuning_results;
    inspect::UintProperty max_delay;
    inspect::UintProperty longest_window_start;
    inspect::UintProperty longest_window_size;
    inspect::UintProperty longest_window_adj_delay;
    inspect::UintProperty distance_to_failing_point;
    inspect::BoolProperty power_suspended;

    void Init(const std::string& instance_identifier, inspect::Node& parent,
              bool is_power_suspended);
  };

  struct TuneContext {
    zx::unowned_vmo vmo;
    cpp20::span<const uint8_t> expected_block;
    uint32_t cmd;
    TuneSettings new_settings;
    TuneSettings original_settings;
  };

  struct SdmmcRequestInfo {
    std::vector<fuchsia_hardware_sdmmc::SdmmcReq> reqs;
    fdf::Arena arena;
    RequestCompleter::Async completer;
  };

  struct SdmmcTaskInfo {
    fit::function<zx_status_t()> task;
    fdf::Arena arena;
    std::variant<SetBusWidthCompleter::Async, SetBusFreqCompleter::Async, SetTimingCompleter::Async,
                 HwResetCompleter::Async, PerformTuningCompleter::Async>
        completer;
  };

  using SdmmcVmoStore = vmo_store::VmoStore<vmo_store::HashTableStorage<uint32_t, OwnedVmoInfo>>;

  // Sweeps from zero to the max delay and creates a TuneWindow representing the largest span of
  // delay values that failed.
  static TuneWindow GetFailingWindow(TuneResults results);

  static uint32_t DistanceToFailingPoint(TuneSettings point,
                                         cpp20::span<const TuneResults> adj_delay_results);

  zx::result<> InitResources(
      fidl::ClientEnd<fuchsia_hardware_platform_device::Device> pdev_client_end,
      fdf::Namespace& incoming);

  void Serve(fdf::ServerEnd<fuchsia_hardware_sdmmc::Sdmmc> request);

  aml_sdmmc_desc_t* descs() const __TA_REQUIRES(lock_) {
    return static_cast<aml_sdmmc_desc_t*>(descs_buffer_->virt());
  }

  template <typename T>
  void DoTaskAndComplete(fit::function<zx_status_t()>, fdf::Arena& arena, T& completer);
  template <typename T>
  void DoRequestAndComplete(fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq> reqs,
                            fdf::Arena& arena, T& completer) __TA_REQUIRES(lock_);
  zx::result<TuneSettings> PerformTuning(cpp20::span<const TuneResults> adj_delay_results);
  zx_status_t TuningDoTransfer(const TuneContext& context) __TA_REQUIRES(tuning_lock_);
  bool TuningTestSettings(const TuneContext& context) __TA_REQUIRES(tuning_lock_);
  TuneResults TuneDelayLines(const TuneContext& context) __TA_REQUIRES(tuning_lock_);

  void SetTuneSettings(const TuneSettings& settings) __TA_REQUIRES(lock_);
  TuneSettings GetTuneSettings() __TA_REQUIRES(lock_);

  void ConfigureDefaultRegs() __TA_REQUIRES(lock_);
  aml_sdmmc_desc_t* SetupCmdDesc(const fuchsia_hardware_sdmmc::wire::SdmmcReq& req)
      __TA_REQUIRES(lock_);
  // Returns a pointer to the LAST descriptor used.
  zx::result<std::pair<aml_sdmmc_desc_t*, std::vector<fzl::PinnedVmo>>> SetupDataDescs(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& req, aml_sdmmc_desc_t* cur_desc)
      __TA_REQUIRES(lock_);
  // These return pointers to the NEXT descriptor to use.
  zx::result<aml_sdmmc_desc_t*> SetupOwnedVmoDescs(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& req,
      const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer,
      vmo_store::StoredVmo<OwnedVmoInfo>& vmo, aml_sdmmc_desc_t* cur_desc) __TA_REQUIRES(lock_);
  zx::result<std::pair<aml_sdmmc_desc_t*, fzl::PinnedVmo>> SetupUnownedVmoDescs(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& req,
      const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer, aml_sdmmc_desc_t* cur_desc)
      __TA_REQUIRES(lock_);
  zx::result<aml_sdmmc_desc_t*> PopulateDescriptors(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& req, aml_sdmmc_desc_t* cur_desc,
      fzl::PinnedVmo::Region region) __TA_REQUIRES(lock_);
  zx_status_t FinishReq(const fuchsia_hardware_sdmmc::wire::SdmmcReq& req);

  void ClearStatus() __TA_REQUIRES(lock_);
  zx::result<std::array<uint32_t, kResponseCount>> WaitForInterrupt(
      const fuchsia_hardware_sdmmc::wire::SdmmcReq& req) __TA_REQUIRES(lock_);

  zx_status_t SetLevel(fuchsia_power_broker::PowerLevel required_level);

  // Serves requests that were delayed because they were received during suspended state.
  void ServeDelayedRequests() __TA_REQUIRES(tuning_lock_, lock_);

  std::optional<fdf::MmioBuffer> mmio_ __TA_GUARDED(lock_);

  aml_sdmmc_config::Config config_;

  zx::bti bti_;

  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> reset_gpio_;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clock_gate_;
  zx::interrupt irq_;

  std::unique_ptr<dma_buffer::ContiguousBuffer> descs_buffer_ __TA_GUARDED(lock_);
  trace_async_id_t trace_async_id_;
  std::vector<std::variant<SdmmcRequestInfo, SdmmcTaskInfo>> delayed_requests_;
  uint32_t clk_div_saved_ = 0;

  zx::event hardware_power_assertive_token_;
  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> power_element_runner_;

  bool shutdown_ __TA_GUARDED(lock_) = false;
  std::array<SdmmcVmoStore, fuchsia_hardware_sdmmc::wire::kSdmmcMaxClientId + 1> registered_vmos_
      __TA_GUARDED(lock_);

  uint64_t consecutive_cmd_errors_ = 0;
  uint64_t consecutive_data_errors_ = 0;

  Inspect inspect_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;

  fidl::ServerBindingGroup<fuchsia_hardware_power::PowerTokenProvider> bindings_;

  compat::SyncInitializedDeviceServer compat_server_;

  // Dedicated dispatcher for inlining fuchsia_hardware_sdmmc::Sdmmc FIDL requests.
  fdf::Dispatcher worker_dispatcher_;

  fdf_metadata::MetadataServer<fuchsia_hardware_sdmmc::SdmmcMetadata> metadata_server_;
};

}  // namespace aml_sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_AML_SDMMC_AML_SDMMC_H_
