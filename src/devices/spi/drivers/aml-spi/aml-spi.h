// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SPI_DRIVERS_AML_SPI_AML_SPI_H_
#define SRC_DEVICES_SPI_DRIVERS_AML_SPI_AML_SPI_H_

#include <fidl/fuchsia.driver.compat/cpp/wire.h>
#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.registers/cpp/wire.h>
#include <fidl/fuchsia.hardware.spi.businfo/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spiimpl/cpp/driver/wire.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/fzl/pinned-vmo.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/profile.h>
#include <lib/zx/result.h>

#include <optional>
#include <queue>
#include <variant>

#include <fbl/array.h>

#include "src/lib/vmo_store/vmo_store.h"

namespace spi {

struct OwnedVmoInfo {
  uint64_t offset;
  uint64_t size;
  fuchsia_hardware_sharedmemory::SharedVmoRight rights;
};

using SpiVmoStore = vmo_store::VmoStore<vmo_store::HashTableStorage<uint32_t, OwnedVmoInfo>>;

// SpiRequest represents a fuchsia.hardware.spiimpl.SpiImpl request that may be completed
// asynchronously. Requests that operate on registered VMOs take a chip select value and completer
// callback, while requests that involve a data transfer additionally take a start completer and
// TX/RX data buffers.
class SpiRequest {
 private:
  static constexpr size_t kInlineCallbackSize = sizeof(fidl::CompleterBase);

 public:
  using Buffer = std::variant<std::nullopt_t, std::vector<uint8_t>,
                              fuchsia_hardware_sharedmemory::wire::SharedVmoBuffer>;
  using StartCallback = fit::callback<bool(SpiRequest)>;
  using CompleteCallback = fit::callback<void(const SpiRequest&, zx_status_t), kInlineCallbackSize>;

  // Constructor for data transfer requests.
  SpiRequest(uint32_t chip_select, Buffer tx_buffer, Buffer rx_buffer, StartCallback start,
             CompleteCallback complete)
      : cs_(chip_select),
        tx_buffer_(std::move(tx_buffer)),
        rx_buffer_(std::move(rx_buffer)),
        start_(std::move(start)),
        complete_(std::move(complete)) {}

  // Constructor for registered VMO requests.
  SpiRequest(uint32_t chip_select, CompleteCallback complete)
      : SpiRequest(chip_select, std::nullopt, std::nullopt, StartCallback([](SpiRequest request) {
                     request.complete_(request, ZX_OK);
                     return true;
                   }),
                   std::move(complete)) {}

  // Transfers ownership to the callee and starts execution of the request. Returns true if the
  // request was completed, or false if it will be completed asynchronously.
  bool Start(SpiVmoStore& vmo_store) &&;
  void Complete(zx_status_t status) { complete_(*this, status); }
  void Cancel() { complete_(*this, ZX_ERR_CANCELED); }

  uint32_t cs() const { return cs_; }
  fidl::VectorView<const uint8_t> txdata() const { return txdata_; }
  fidl::VectorView<uint8_t> rxdata() const { return rxdata_; }
  size_t size() const { return std::max(txdata_.size(), rxdata_.size()); }

  // Transfers ownership of registered VMOs to this request. They will automatically be released
  // when the request completes.
  void ReleaseVmosOnComplete(std::unique_ptr<SpiVmoStore> vmos) {
    vmos_to_release_.push_back(std::move(vmos));
  }

 private:
  static zx::result<cpp20::span<uint8_t>> GetBuffer(
      SpiVmoStore& vmo_store, Buffer& buffer, fuchsia_hardware_sharedmemory::SharedVmoRight right);

  zx_status_t PopulateBuffers(SpiVmoStore& vmo_store);

  uint32_t cs_;

  fidl::VectorView<const uint8_t> txdata_;
  fidl::VectorView<uint8_t> rxdata_;

  Buffer tx_buffer_;
  Buffer rx_buffer_;

  StartCallback start_;
  CompleteCallback complete_;

  std::vector<std::unique_ptr<SpiVmoStore>> vmos_to_release_;
};

class AmlSpi : public fdf::WireServer<fuchsia_hardware_spiimpl::SpiImpl> {
 public:
  struct ChipInfo {
    fidl::WireClient<fuchsia_hardware_gpio::Gpio> gpio;
    std::unique_ptr<SpiVmoStore> registered_vmos;
  };

  // DmaBuffer holds a contiguous VMO that is both pinned and mapped.
  struct DmaBuffer {
    static zx_status_t Create(const zx::bti& bti, size_t size, DmaBuffer* out_dma_buffer);

    zx::vmo vmo;
    fzl::PinnedVmo pinned;
    fzl::VmoMapper mapped;
  };

  AmlSpi(fdf::MmioBuffer mmio, fidl::ClientEnd<fuchsia_hardware_registers::Device> reset,
         uint32_t reset_mask, fbl::Array<ChipInfo> chips, zx::interrupt interrupt,
         fuchsia_hardware_amlogic_metadata::SpiConfig config, zx::bti bti, DmaBuffer tx_buffer,
         DmaBuffer rx_buffer)
      : mmio_(std::move(mmio)),
        dispatcher_(fdf::Dispatcher::GetCurrent()),
        reset_(std::move(reset), dispatcher_->async_dispatcher()),
        reset_mask_(reset_mask),
        chips_(std::move(chips)),
        interrupt_(std::move(interrupt)),
        config_(std::move(config)),
        bti_(std::move(bti)),
        tx_buffer_(std::move(tx_buffer)),
        rx_buffer_(std::move(rx_buffer)),
        outgoing_(fdf::Dispatcher::GetCurrent()->get()) {
    bindings_.set_empty_set_handler(fit::bind_member<&AmlSpi::OnAllClientsUnbound>(this));
  }

  void InitRegisters();

  void Serve(fdf::ServerEnd<fuchsia_hardware_spiimpl::SpiImpl> request);

  void PrepareStop(fdf::StopCompleter completer);
  void Stop();

 private:
  void GetChipSelectCount(fdf::Arena& arena,
                          GetChipSelectCountCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(static_cast<uint32_t>(chips_.size()));
  }
  void TransmitVector(fuchsia_hardware_spiimpl::wire::SpiImplTransmitVectorRequest* request,
                      fdf::Arena& arena, TransmitVectorCompleter::Sync& completer) override;
  void ReceiveVector(fuchsia_hardware_spiimpl::wire::SpiImplReceiveVectorRequest* request,
                     fdf::Arena& arena, ReceiveVectorCompleter::Sync& completer) override;
  void ExchangeVector(fuchsia_hardware_spiimpl::wire::SpiImplExchangeVectorRequest* request,
                      fdf::Arena& arena, ExchangeVectorCompleter::Sync& completer) override;
  void RegisterVmo(fuchsia_hardware_spiimpl::wire::SpiImplRegisterVmoRequest* request,
                   fdf::Arena& arena, RegisterVmoCompleter::Sync& completer) override;
  void UnregisterVmo(fuchsia_hardware_spiimpl::wire::SpiImplUnregisterVmoRequest* request,
                     fdf::Arena& arena, UnregisterVmoCompleter::Sync& completer) override;
  void ReleaseRegisteredVmos(
      fuchsia_hardware_spiimpl::wire::SpiImplReleaseRegisteredVmosRequest* request,
      fdf::Arena& arena, ReleaseRegisteredVmosCompleter::Sync& completer) override;
  void TransmitVmo(fuchsia_hardware_spiimpl::wire::SpiImplTransmitVmoRequest* request,
                   fdf::Arena& arena, TransmitVmoCompleter::Sync& completer) override;
  void ReceiveVmo(fuchsia_hardware_spiimpl::wire::SpiImplReceiveVmoRequest* request,
                  fdf::Arena& arena, ReceiveVmoCompleter::Sync& completer) override;
  void ExchangeVmo(fuchsia_hardware_spiimpl::wire::SpiImplExchangeVmoRequest* request,
                   fdf::Arena& arena, ExchangeVmoCompleter::Sync& completer) override;

  void LockBus(fuchsia_hardware_spiimpl::wire::SpiImplLockBusRequest* request, fdf::Arena& arena,
               LockBusCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void UnlockBus(fuchsia_hardware_spiimpl::wire::SpiImplUnlockBusRequest* request,
                 fdf::Arena& arena, UnlockBusCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void DumpState();

  // Returns true if the request was completed in HandleRequest, or false if it was added to the
  // queue (or will be completed asynchronously).
  bool HandleRequest(SpiRequest request);
  void ServiceRequestQueue();

  // Returns true if the request was completed, or false if it will be completed asynchronously.
  bool StartExchange(SpiRequest request);
  bool AssertCs();
  bool Exchange();
  void CompleteExchange(zx_status_t status);

  void Exchange8(const uint8_t* txdata, uint8_t* out_rxdata, size_t size);
  void Exchange64(const uint8_t* txdata, uint8_t* out_rxdata, size_t size);

  void WaitForTransferComplete();
  void WaitForDmaTransferComplete();

  zx_status_t ExchangeDma(const uint8_t* txdata, uint8_t* out_rxdata, uint64_t size);

  size_t DoDmaTransfer(size_t words_remaining);

  bool UseDma(size_t size) const;

  void OnAllClientsUnbound();

  const fidl::WireClient<fuchsia_hardware_gpio::Gpio>& gpio(uint32_t chip_select) {
    return chips_[chip_select].gpio;
  }

  std::unique_ptr<SpiVmoStore>& registered_vmos(uint32_t chip_select) {
    return chips_[chip_select].registered_vmos;
  }

  fdf::MmioBuffer mmio_;
  fdf::UnownedDispatcher dispatcher_;
  fidl::WireClient<fuchsia_hardware_registers::Device> reset_;
  const uint32_t reset_mask_;
  const fbl::Array<ChipInfo> chips_;
  bool need_reset_ = false;
  zx::interrupt interrupt_;
  const fuchsia_hardware_amlogic_metadata::SpiConfig config_;
  zx::bti bti_;
  DmaBuffer tx_buffer_;
  DmaBuffer rx_buffer_;
  bool shutdown_ = false;
  fdf::OutgoingDirectory outgoing_;
  fdf::ServerBindingGroup<fuchsia_hardware_spiimpl::SpiImpl> bindings_;
  std::optional<fdf::StopCompleter> prepare_stop_completer_;

  // When a spiimpl request is received, its completer is moved into a fit::callback which is then
  // used to create a SpiRequest. If there is no other request currently executing (i.e.
  // current_request_ is not valid), the new SpiRequest is started immediately by calling Start()
  // on it. For data transfer requests, Start() moves the SpiRequest into current_request_
  // and may handle the transfer asynchronously. For other requests (RegisterVmo, UnregisterVmo,
  // and ReleaseRegisteredVmos), Start() simply invokes the completer callback. If another request
  // is executing, the SpiRequest is pushed to the back of request_queue_ to be handled later.
  //
  // After current_request_ completes and its callback is invoked, pending requests are removed from
  // request_queue_ and started until there are no requests left in the queue, or until a request
  // must be handled asynchronously. In the latter case current_request_ is set, and the cycle
  // repeats.

  // The currently executing request. Can only be a data transfer request; other types are completed
  // immediately after being taken off the queue.
  std::optional<SpiRequest> current_request_;

  // CompleteExchange() calls ServiceRequestQueue() which calls HandleRequest(), which may call
  // CompleteExchange() if the next request is handled synchronously. executing_synchronous_request_
  // is used to prevent a recursive call back into ServiceRequestQueue() in this case.
  bool executing_synchronous_request_ = false;

  std::queue<SpiRequest> request_queue_;
};

// AmlSpiDriver is a helper class that is responsible for acquiring resources on behalf of AmlSpi so
// that it can support RAII in DFv2. It implements the driver Start hook, and forwards Stop to the
// AmlSpi instance.
class AmlSpiDriver : public fdf::DriverBase2 {
 public:
  AmlSpiDriver() : fdf::DriverBase2("aml-spi") {}

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;

  void Stop(fdf::StopCompleter completer) override {
    if (device_) {
      device_->PrepareStop(std::move(completer));
    } else {
      completer(zx::ok());
    }
  }

  ~AmlSpiDriver() override {
    if (device_) {
      device_->Stop();
    }
  }

 protected:
  std::unique_ptr<AmlSpi> device_;
  // MapMmio can be overridden by a test in order to provide an fdf::MmioBuffer backed by a fake.
  virtual fpromise::promise<fdf::MmioBuffer, zx_status_t> MapMmio(
      fidl::WireClient<fuchsia_hardware_platform_device::Device>& pdev, uint32_t mmio_id);

 private:
  // Asynchronously retrieve metadata of type |FidlType| from the platform device.
  template <typename FidlType>
  fpromise::promise<std::optional<FidlType>, zx_status_t> GetMetadata() {
    fpromise::bridge<std::optional<FidlType>, zx_status_t> bridge;

    pdev_->GetMetadata(FidlType::kSerializableName)
        .Then([completer = std::move(bridge.completer)](
                  fidl::WireUnownedResult<fuchsia_hardware_platform_device::Device::GetMetadata>&
                      result) mutable {
          if (!result.ok()) {
            fdf::error("Failed to send GetMetadata request: {}", result.status_string());
            completer.complete_error(result.status());
            return;
          }

          if (result->is_error()) {
            if (result->error_value() == ZX_ERR_NOT_FOUND) {
              completer.complete_ok(std::nullopt);
              return;
            }
            fdf::error("Failed to get metadata: {}", zx_status_get_string(result->error_value()));
            completer.complete_error(result->error_value());
            return;
          }

          std::span<uint8_t> persisted_metadata = result->value()->metadata;
          fit::result metadata = fidl::Unpersist<FidlType>(persisted_metadata);
          if (metadata.is_error()) {
            fdf::error("Failed to unpersist metadata: {}",
                       zx_status_get_string(metadata.error_value().status()));
            completer.complete_error(metadata.error_value().status());
            return;
          }

          completer.complete_ok(std::move(metadata.value()));
        });

    return bridge.consumer.promise_or(fpromise::error(ZX_ERR_INTERNAL));
  }

  zx::result<> ServeRoleName(const std::optional<fuchsia_scheduler::RoleName>& scheduler_role_name);

  void AddNode(fdf::MmioBuffer mmio, const fuchsia_hardware_amlogic_metadata::SpiConfig& config,
               zx::interrupt interrupt, zx::bti bti,
               const std::optional<fuchsia_hardware_spi_businfo::SpiBusMetadata>& spi_bus_metadata,
               fdf::StartCompleter completer);

  fpromise::promise<zx::interrupt, zx_status_t> GetInterrupt();
  fpromise::promise<zx::bti, zx_status_t> GetBti();

  fbl::Array<AmlSpi::ChipInfo> InitChips(
      const fuchsia_hardware_amlogic_metadata::SpiConfig& config);

  const fdf::Namespace& incoming() const { return *incoming_; }
  fdf::Namespace& incoming() { return *incoming_; }

  fidl::WireClient<fuchsia_driver_framework::Node> parent_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::WireClient<fuchsia_hardware_platform_device::Device> pdev_;
  fidl::WireClient<fuchsia_driver_compat::Device> compat_;
  std::optional<async::Executor> executor_;
  std::unique_ptr<fdf::Namespace> incoming_;
  fdf_metadata::MetadataServer<fuchsia_hardware_spi_businfo::SpiBusMetadata> spi_metadata_server_;
  fdf_metadata::MetadataServer<fuchsia_scheduler::RoleName> scheduler_role_name_metadata_server_;
};

}  // namespace spi

#endif  // SRC_DEVICES_SPI_DRIVERS_AML_SPI_AML_SPI_H_
