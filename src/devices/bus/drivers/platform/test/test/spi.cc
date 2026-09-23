// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.spi.businfo/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spiimpl/cpp/driver/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata_server.h>

#include <memory>

namespace spi {

class TestSpiDriver : public fdf::DriverBase2,
                      public fdf::WireServer<fuchsia_hardware_spiimpl::SpiImpl> {
 public:
  static constexpr std::string_view kChildNodeName = "test-spi";
  static constexpr std::string_view kDriverName = "test-spi";

  TestSpiDriver() : DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    {
      zx::result<> result =
          compat_server_.Initialize(incoming_ptr, outgoing(), context.node_name(), kChildNodeName,
                                    compat::ForwardMetadata::None());
      if (result.is_error()) {
        return result.take_error();
      }
    }
    zx_status_t status =
        compat_server_.inner().AddMetadata(DEVICE_METADATA_PRIVATE, &bus_id_, sizeof bus_id_);
    if (status != ZX_OK) {
      fdf::error("Failed to add metadata: {}", zx_status_get_string(status));
      return zx::error(status);
    }

    zx::result pdev = incoming_ptr->Connect<fuchsia_hardware_platform_device::Service::Device>();
    if (zx::result result =
            spi_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), pdev.value());
        result.is_error()) {
      fdf::error("Failed to forward SPI metadata: {}", result);
      return result.take_error();
    }

    {
      fuchsia_hardware_spiimpl::Service::InstanceHandler handler({
          .device = bindings_.CreateHandler(this, driver_dispatcher()->get(),
                                            fidl::kIgnoreBindingClosure),
      });
      auto result = outgoing()->AddService<fuchsia_hardware_spiimpl::Service>(std::move(handler));
      if (result.is_error()) {
        fdf::error("AddService failed: {}", result);
        return result.take_error();
      }
    }

    std::vector offers = compat_server_.CreateOffers2();
    offers.push_back(fdf::MakeOffer2<fuchsia_hardware_spiimpl::Service>());
    std::optional metadata_offer = spi_metadata_server_.CreateOffer();
    if (metadata_offer.has_value()) {
      offers.push_back(std::move(metadata_offer.value()));
    }
    zx::result child =
        AddChild(kChildNodeName, std::vector<fuchsia_driver_framework::NodeProperty>{}, offers);
    if (child.is_error()) {
      fdf::error("Failed to add child: {}", child);
      return child.take_error();
    }
    child_ = std::move(child.value());

    return zx::ok();
  }

  void GetChipSelectCount(fdf::Arena& arena,
                          GetChipSelectCountCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(1);
  }

  void TransmitVector(fuchsia_hardware_spiimpl::wire::SpiImplTransmitVectorRequest* request,
                      fdf::Arena& arena, TransmitVectorCompleter::Sync& completer) override {
    // TX only, ignore
    completer.buffer(arena).ReplySuccess();
  }

  void ReceiveVector(fuchsia_hardware_spiimpl::wire::SpiImplReceiveVectorRequest* request,
                     fdf::Arena& arena, ReceiveVectorCompleter::Sync& completer) override {
    fidl::VectorView<uint8_t> rxdata(arena, request->size);
    // RX only, fill with pattern
    for (size_t i = 0; i < rxdata.size(); i++) {
      rxdata[i] = i & 0xff;
    }
    completer.buffer(arena).ReplySuccess(rxdata);
  }

  void ExchangeVector(fuchsia_hardware_spiimpl::wire::SpiImplExchangeVectorRequest* request,
                      fdf::Arena& arena, ExchangeVectorCompleter::Sync& completer) override {
    fidl::VectorView<uint8_t> rxdata(arena, request->txdata.size());
    // Both TX and RX; copy
    memcpy(rxdata.data(), request->txdata.data(), request->txdata.size());
    completer.buffer(arena).ReplySuccess(rxdata);
  }

  void LockBus(fuchsia_hardware_spiimpl::wire::SpiImplLockBusRequest* request, fdf::Arena& arena,
               LockBusCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void UnlockBus(fuchsia_hardware_spiimpl::wire::SpiImplUnlockBusRequest* request,
                 fdf::Arena& arena, UnlockBusCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void RegisterVmo(fuchsia_hardware_spiimpl::wire::SpiImplRegisterVmoRequest* request,
                   fdf::Arena& arena, RegisterVmoCompleter::Sync& completer) override {}

  void UnregisterVmo(fuchsia_hardware_spiimpl::wire::SpiImplUnregisterVmoRequest* request,
                     fdf::Arena& arena, UnregisterVmoCompleter::Sync& completer) override {}

  void ReleaseRegisteredVmos(
      fuchsia_hardware_spiimpl::wire::SpiImplReleaseRegisteredVmosRequest* request,
      fdf::Arena& arena, ReleaseRegisteredVmosCompleter::Sync& completer) override {}

  void TransmitVmo(fuchsia_hardware_spiimpl::wire::SpiImplTransmitVmoRequest* request,
                   fdf::Arena& arena, TransmitVmoCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void ReceiveVmo(fuchsia_hardware_spiimpl::wire::SpiImplReceiveVmoRequest* request,
                  fdf::Arena& arena, ReceiveVmoCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void ExchangeVmo(fuchsia_hardware_spiimpl::wire::SpiImplExchangeVmoRequest* request,
                   fdf::Arena& arena, ExchangeVmoCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  uint32_t bus_id_ = 0;
  fdf::ServerBindingGroup<fuchsia_hardware_spiimpl::SpiImpl> bindings_;
  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  fdf_metadata::MetadataServer<fuchsia_hardware_spi_businfo::SpiBusMetadata> spi_metadata_server_;
};

}  // namespace spi

FUCHSIA_DRIVER_EXPORT2(spi::TestSpiDriver);
