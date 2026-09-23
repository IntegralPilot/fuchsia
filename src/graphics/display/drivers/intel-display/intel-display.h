// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTEL_DISPLAY_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTEL_DISPLAY_H_

#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/fidl.h>
#include <fuchsia/hardware/intelgpucore/cpp/banjo.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <lib/driver/component/cpp/stop_completer.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/pci/constants.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zx/channel.h>

#include <memory>
#include <optional>
#include <span>

#include <fbl/mutex.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/intel-display/clock/cdclk.h"
#include "src/graphics/display/drivers/intel-display/display-device.h"
#include "src/graphics/display/drivers/intel-display/dp-aux-channel-impl.h"
#include "src/graphics/display/drivers/intel-display/dpll.h"
#include "src/graphics/display/drivers/intel-display/gtt.h"
#include "src/graphics/display/drivers/intel-display/i2c/gmbus-i2c.h"
#include "src/graphics/display/drivers/intel-display/igd.h"
#include "src/graphics/display/drivers/intel-display/interrupts.h"
#include "src/graphics/display/drivers/intel-display/pch-engine.h"
#include "src/graphics/display/drivers/intel-display/pipe-manager.h"
#include "src/graphics/display/drivers/intel-display/pipe.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"

namespace intel_display {

typedef struct buffer_allocation {
  uint16_t start;
  uint16_t end;
} buffer_allocation_t;

struct ControllerResources {
  // Must be of type `ZX_RSRC_KIND_MMIO` with access to all valid ranges.
  zx::unowned_resource mmio;

  // Must be of type `ZX_RSRC_KIND_IOPORT` with access to all valid ranges.
  zx::unowned_resource ioport;
};

class Controller final : public display::DisplayEngineInterface,
                         public ddk::IntelGpuCoreProtocol<Controller> {
 public:
  // Creates a `Controller` instance and performs short-running initialization
  // of all subcomponents.
  //
  // Long-running initialization is performed in the Start() hook.
  //
  // `sysmem` must be non-null.
  // `pci` must be non-null.
  // `resources` must be valid while the Controller instance is alive.
  // `engine_events` must be non-null and must outlive the created `Controller`
  // instance.
  static zx::result<std::unique_ptr<Controller>> Create(
      fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem,
      fidl::ClientEnd<fuchsia_hardware_pci::Device> pci, ControllerResources resources,
      std::optional<zbi_swfb_t> framebuffer_info,
      display::DisplayEngineEventsInterface* engine_events, inspect::Inspector inspector);

  // Prefer to use the `Create()` factory function in production code.
  explicit Controller(fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem,
                      fidl::ClientEnd<fuchsia_hardware_pci::Device> pci,
                      ControllerResources resources, std::optional<zbi_swfb_t> framebuffer_info,
                      display::DisplayEngineEventsInterface* engine_events,
                      inspect::Inspector inspector);

  // Creates a Controller with no valid FIDL clients and no resources for
  // testing purpose only.
  //
  // `engine_events` must be non-null and must outlive the created `Controller`
  // instance.
  explicit Controller(display::DisplayEngineEventsInterface* engine_events,
                      inspect::Inspector inspector);

  ~Controller();

  // Corresponds to DFv2 `Start()`.
  void Start(fdf::StartCompleter completer);

  // Corresponds to DFv2 `PrepareStop()`.
  void PrepareStopOnPowerOn(fdf::StopCompleter completer);

  // Corresponds to DFv2 `PrepareStop()`.
  void PrepareStopOnPowerStateTransition(fuchsia_system_state::SystemPowerState power_state,
                                         fdf::StopCompleter completer);

  // `display::DisplayEngineInterface`:
  display::EngineInfo CompleteCoordinatorConnection() override;
  zx::result<> ImportBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) override;
  zx::result<> ReleaseBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id) override;
  zx::result<display::DriverImageId> ImportImage(
      const display::ImageMetadata& image_metadata,
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override;
  zx::result<display::DriverCaptureImageId> ImportImageForCapture(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override;
  void ReleaseImage(display::DriverImageId driver_image_id) override;
  display::ConfigCheckResult CheckConfiguration(
      display::DisplayId display_id, display::ModeId mode_id,
      display::ColorConversion color_conversion,
      std::span<const display::DriverLayer> layers) override;
  void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                           display::ColorConversion color_conversion,
                           std::span<const display::DriverLayer> layers,
                           display::DriverConfigStamp driver_config_stamp) override;
  zx::result<> SetBufferCollectionConstraints(
      const display::ImageBufferUsage& image_buffer_usage,
      display::DriverBufferCollectionId buffer_collection_id) override;
  zx::result<> SetDisplayPowerMode(display::DisplayId display_id,
                                   display::PowerMode power_mode) override;

  // gpu core ops
  zx_status_t IntelGpuCoreReadPciConfig16(uint16_t addr, uint16_t* value_out);
  zx_status_t IntelGpuCoreMapPciMmio(uint32_t pci_bar, uint8_t** addr_out, uint64_t* size_out);
  zx_status_t IntelGpuCoreUnmapPciMmio(uint32_t pci_bar);
  zx_status_t IntelGpuCoreGetPciBti(uint32_t index, zx::bti* bti_out);
  zx_status_t IntelGpuCoreRegisterInterruptCallback(const intel_gpu_core_interrupt_t* callback,
                                                    uint32_t interrupt_mask);
  zx_status_t IntelGpuCoreUnregisterInterruptCallback();
  uint64_t IntelGpuCoreGttGetSize();
  zx_status_t IntelGpuCoreGttAlloc(uint64_t page_count, uint64_t* addr_out);
  zx_status_t IntelGpuCoreGttFree(uint64_t addr);
  zx_status_t IntelGpuCoreGttClear(uint64_t addr);
  zx_status_t IntelGpuCoreGttInsert(uint64_t addr, zx::vmo buffer, uint64_t page_offset,
                                    uint64_t page_count);

  fdf::MmioBuffer* mmio_space() { return mmio_space_.has_value() ? &*mmio_space_ : nullptr; }
  Interrupts* interrupts() { return &interrupts_; }
  uint16_t device_id() const { return device_id_; }
  const IgdOpRegion& igd_opregion() const { return igd_opregion_; }
  Power* power() { return power_.get(); }
  PipeManager* pipe_manager() { return pipe_manager_.get(); }
  DisplayPllManager* dpll_manager() { return dpll_manager_.get(); }

  // Non-const getter to allow unit tests to modify the IGD.
  // TODO(https://fxbug.dev/42164736): Consider making a fake IGD object injectable as allowing
  // mutable access to internal state that is intended to be externally immutable can be source of
  // bugs if used incorrectly. The various "ForTesting" methods are a typical anti-pattern that
  // exposes internal state and makes the class state machine harder to reason about.
  IgdOpRegion* igd_opregion_for_testing() { return &igd_opregion_; }

  void HandleHotplug(DdiId ddi_id, bool long_pulse);
  void HandlePipeVsync(PipeId pipe_id_num, zx::time_monotonic timestamp);

  void ResetPipePlaneBuffers(PipeId pipe_id);
  bool ResetDdi(DdiId ddi_id, std::optional<TranscoderId> transcoder_id);

  void SetDpllManagerForTesting(std::unique_ptr<DisplayPllManager> dpll_manager) {
    dpll_manager_ = std::move(dpll_manager);
  }
  void SetPipeManagerForTesting(std::unique_ptr<PipeManager> pipe_manager) {
    pipe_manager_ = std::move(pipe_manager);
  }
  void SetPowerWellForTesting(std::unique_ptr<Power> power_well) { power_ = std::move(power_well); }
  void SetMmioForTesting(const fdf::MmioView& mmio_space) { mmio_space_.emplace(mmio_space); }

  void ResetMmioSpaceForTesting() { mmio_space_.reset(); }

  zx_status_t SetAndInitSysmemForTesting(fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem) {
    sysmem_ = std::move(sysmem);
    return ZX_OK;
  }

  zx_status_t InitGttForTesting(const ddk::Pci& pci, fdf::MmioBuffer buffer, uint32_t fb_offset);

  // For every frame, in order to use the imported image, it is required to set
  // up the image based on given rotation in GTT and use the handle offset in
  // GTT. Returns the Gtt region representing the image.
  const GttRegion& SetupGttImage(const display::ImageMetadata& image_metadata,
                                 display::DriverImageId image_id,
                                 display::CoordinateTransformation coordinate_transformation);

  // The pixel format negotiated by sysmem for an imported image.
  //
  // `image_id` must correspond to an image that was successfully imported, and
  // was not released.
  PixelFormatAndModifier GetImportedImagePixelFormat(display::DriverImageId image_id) const;

  zx::result<ddk::AnyProtocol> GetProtocol(uint32_t proto_id);

  inspect::Inspector& inspector() { return inspector_; }

 private:
  // Perform short-running initialization of all subcomponents and instruct the DDK to publish the
  // device. On success, returns ZX_OK and the ownership of the Controller instance is claimed by
  // the DDK.
  //
  // Long-running initialization is performed in the DdkInit hook.
  zx_status_t Init();

  const std::unique_ptr<GttRegionImpl>& GetGttRegionImpl(display::DriverImageId image_id);
  void InitDisplays();

  // Reads the memory latency information needed to confiugre pipes and planes.
  //
  // Returns false if a catastrophic error occurred, and pipes and planes cannot
  // be safely configured.
  bool ReadMemoryLatencyInfo();

  // Disables the PCU (power controller)'s automated voltage adjustments.
  void DisableSystemAgentGeyserville();

  ControllerResources resources_;
  std::optional<zbi_swfb_t> framebuffer_info_;

  std::unique_ptr<DisplayDevice> QueryDisplay(DdiId ddi_id, display::DisplayId display_id)
      __TA_REQUIRES(display_lock_);
  bool LoadHardwareState(DdiId ddi_id, DisplayDevice* device) __TA_REQUIRES(display_lock_);
  zx_status_t AddDisplay(std::unique_ptr<DisplayDevice> display) __TA_REQUIRES(display_lock_);
  void RemoveDisplay(std::unique_ptr<DisplayDevice> display) __TA_REQUIRES(display_lock_);
  bool BringUpDisplayEngine(bool resume) __TA_REQUIRES(display_lock_);
  void InitDisplayBuffers();
  DisplayDevice* FindDevice(display::DisplayId display_id) __TA_REQUIRES(display_lock_);

  // Returns the `Pipe` that `display_id` is bound to.
  //
  // Returns nullptr if the `display_id` is not bound to any Pipe.
  Pipe* GetPipeForDisplay(display::DisplayId display_id) __TA_REQUIRES(display_lock_);

  // Gets the DriverLayer config for the given pipe/plane. Return false if there is no layer.
  std::optional<display::DriverLayer> GetDriverLayerForPlane(
      uint32_t plane, std::span<const display::DriverLayer> layers) __TA_REQUIRES(display_lock_);

  uint16_t CalculateBuffersPerPipe(size_t active_pipe_count);
  // Returns false if no allocation is possible. When that happens,
  // plane 0 of the failing displays will be set to UINT16_MAX.
  bool CalculateMinimumAllocations(
      display::DisplayId display_id, std::span<const display::DriverLayer> layers,
      uint16_t min_allocs[PipeIds<registers::Platform::kKabyLake>().size()]
                         [registers::kImagePlaneCount]) __TA_REQUIRES(display_lock_);
  // Updates plane_buffers_ based pipe_buffers_ and the given parameters
  void UpdateAllocations(
      const uint16_t min_allocs[PipeIds<registers::Platform::kKabyLake>().size()]
                               [registers::kImagePlaneCount],
      const uint64_t data_rate_bytes_per_frame[PipeIds<registers::Platform::kKabyLake>().size()]
                                              [registers::kImagePlaneCount])
      __TA_REQUIRES(display_lock_);
  // Reallocates the pipe buffers when a pipe comes online/goes offline. This is a
  // long-running operation, as shifting allocations between pipes requires waiting
  // for vsync.
  void DoPipeBufferReallocation(
      buffer_allocation_t active_allocation[PipeIds<registers::Platform::kKabyLake>().size()])
      __TA_REQUIRES(display_lock_);
  // Reallocates plane buffers based on the given layer config.
  void ReallocatePlaneBuffers(display::DisplayId display_id,
                              std::span<const display::DriverLayer> layers, bool reallocate_pipes)
      __TA_REQUIRES(display_lock_);

  // Validates that a basic layer configuration can be supported for the
  // given modes of the displays.
  bool CheckDisplayLimits(display::DisplayId display_id,
                          const display::DisplayTiming& display_timing,
                          std::span<const display::DriverLayer> layers)
      __TA_REQUIRES(display_lock_);

  bool CalculatePipeAllocation(display::DisplayId display_id,
                               std::span<display::DisplayId> display_allocated_to_pipe)
      __TA_REQUIRES(display_lock_);

  // The number of DBUF (Data Buffer) blocks that can be allocated to planes.
  //
  // This number depends on the display engine and the number of DBUF slices
  // that are powered up.
  uint16_t DataBufferBlockCount() const;

  // The sysmem allocator client used to bind incoming buffer collection tokens.
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_;

  // Imported sysmem buffer collections.
  std::unordered_map<display::DriverBufferCollectionId,
                     fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>>
      buffer_collections_;

  display::DisplayEngineEventsInterface& engine_events_ __TA_GUARDED(display_lock_);

  // True iff the driver initialization is fully completed.
  bool driver_initialized_ __TA_GUARDED(display_lock_) = false;

  // Pixel formats of imported images.
  std::unordered_map<display::DriverImageId, PixelFormatAndModifier> imported_image_pixel_formats_
      __TA_GUARDED(gtt_lock_);

  IgdOpRegion igd_opregion_;  // Read only, no locking
  Interrupts interrupts_;     // Internal locking

  ddk::Pci pci_;
  std::array<std::optional<fdf::MmioBuffer>, pci::kMaxBarCount> mapped_bars_
      __TA_GUARDED(bar_lock_);
  fbl::Mutex bar_lock_;
  // The mmio_space_ is read only. The internal registers are guarded by various locks where
  // appropriate.
  //
  // Must outlive `gtt_`.
  std::optional<fdf::MmioView> mmio_space_;

  Gtt gtt_ __TA_GUARDED(gtt_lock_);
  mutable fbl::Mutex gtt_lock_;
  // These regions' VMOs are not owned
  fbl::Vector<std::unique_ptr<GttRegionImpl>> imported_images_ __TA_GUARDED(gtt_lock_);
  // These regions' VMOs are owned
  fbl::Vector<std::unique_ptr<GttRegionImpl>> imported_gtt_regions_ __TA_GUARDED(gtt_lock_);

  std::optional<PchEngine> pch_engine_;
  std::unique_ptr<Power> power_;

  std::unique_ptr<DdiManager> ddi_manager_;

  // Must outlive `display_devices_`.
  //
  // The `DisplayDevice` destructor calls into the `PipeManager`.
  std::unique_ptr<PipeManager> pipe_manager_;

  PowerWellRef cd_clk_power_well_;
  std::unique_ptr<CoreDisplayClock> cd_clk_;

  std::unique_ptr<DisplayPllManager> dpll_manager_;

  std::span<const DdiId> ddis_;
  fbl::Vector<GMBusI2c> gmbus_i2cs_;
  fbl::Vector<DpAuxChannelImpl> dp_aux_channels_;

  // References to displays. References are owned by devmgr, but will always
  // be valid while they are in this vector.
  fbl::Vector<std::unique_ptr<DisplayDevice>> display_devices_ __TA_GUARDED(display_lock_);
  // Display ID can't be kInvalidDisplayId.
  display::DisplayId next_id_ __TA_GUARDED(display_lock_) = display::DisplayId{1};
  fbl::Mutex display_lock_;

  // Plane buffer allocation. If no alloc, start == end == registers::PlaneBufCfg::kBufferCount.
  buffer_allocation_t plane_buffers_[PipeIds<registers::Platform::kKabyLake>().size()]
                                    [registers::kImagePlaneCount] __TA_GUARDED(
                                        plane_buffers_lock_) = {};
  fbl::Mutex plane_buffers_lock_;

  // Buffer allocations for pipes
  buffer_allocation_t pipe_buffers_[PipeIds<registers::Platform::kKabyLake>().size()] __TA_GUARDED(
      display_lock_) = {};
  bool initial_alloc_ = true;

  uint16_t device_id_;
  uint32_t flags_;

  // Various configuration values set by the BIOS which need to be carried across suspend.
  bool ddi_e_disabled_ = true;

  // Debug
  inspect::Inspector inspector_;
  inspect::Node root_node_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTEL_DISPLAY_H_
