// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_CODEC_FACTORY_CODEC_FACTORY_APP_H_
#define SRC_MEDIA_CODEC_FACTORY_CODEC_FACTORY_APP_H_

#include <fidl/fuchsia.gpu.magma/cpp/fidl.h>
#include <fidl/fuchsia.hardware.mediacodec/cpp/fidl.h>
#include <fuchsia/gpu/magma/cpp/fidl.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <list>
#include <memory>
#include <optional>
#include <random>

#include "codec_factory_policy.h"
#include "src/lib/fsl/io/device_watcher.h"

using CodecServiceMemberWatcher =
    component::ServiceMemberWatcher<fuchsia_hardware_mediacodec::Service::Device>;
using MagmaServiceMemberWatcher =
    component::ServiceMemberWatcher<fuchsia_gpu_magma::Service::Device>;

// CodecFactoryApp is singleton per-process.
class CodecFactoryApp {
 public:
  enum class ProdOrTest {
    kProduction,

    // In tests, the outgoing directory will not be serviced, since test
    // components do not have access to the outgoing directory server endpoint
    // in their process startup info.
    kTesting,
  };

  CodecFactoryApp(async_dispatcher_t* dispatcher, ProdOrTest prod_or_test);

  // The caller must only call this on the FIDL thread, and the returned * is
  // only valid for use until the caller returns from the caller's work on the
  // FIDL thread.  The caller must not stash the returned * beyond the caller's
  // return from the caller's work on the FIDL thread, as the next item of work
  // on the FIDL thread could ~CodecFactoryPtr or similar.
  //
  // This method can return nullptr if a HW decoder isn't found...
  [[nodiscard]] const fuchsia::mediacodec::CodecFactoryPtr* FindHwCodec(
      fit::function<bool(const fuchsia::mediacodec::DetailedCodecDescription&)> is_match);

  [[nodiscard]] std::optional<std::string> FindHwIsolate(
      fit::function<bool(const fuchsia::mediacodec::DetailedCodecDescription&)> is_match);

  [[nodiscard]] std::vector<fuchsia::mediacodec::CodecDescription> MakeCodecList() const;
  [[nodiscard]] std::vector<fuchsia::mediacodec::DetailedCodecDescription>
  MakeDetailedCodecDescriptions() const;

  [[nodiscard]] async_dispatcher_t* dispatcher() { return dispatcher_; }
  [[nodiscard]] const std::string& board_name() { return board_name_; }

  [[nodiscard]] CodecFactoryPolicy& policy() { return policy_; }

 private:
  struct CodecListEntry {
    fuchsia::mediacodec::DetailedCodecDescription detailed_description;
    inspect::Node codec_node;

    // The info here is entirely derived from detailed_description, not from info sent from the
    // codec via OnCodecList.
    fuchsia::mediacodec::CodecDescription deprecated_codec_description;
  };

  // A struct that represents a device that implements the fuchsia.mediacodec.CodecFactory protocol.
  struct CodecFactoryEntry {
   public:
    std::string component_url;

    // When a HW-accelerated CodecFactory supports more than one sort of codec,
    // the CodecFactory will have multiple entries that share the CodecFactory
    // via the shared_ptr<> here.  The relevant entries co-own the
    // CodecFactoryPtr, and a shared_ptr<> ref is only transiently held by any
    // other code (not posted; not sent across threads).
    std::shared_ptr<fuchsia::mediacodec::CodecFactoryPtr> codec_factory;

    // If the codec is a one backed by a magma device, this value will be set to the ICD loader, and
    // if not it will nullptr
    std::shared_ptr<fuchsia::gpu::magma::IcdLoaderDevicePtr> magma_device;

    // A list of hardware accelerated codecs provided by this codec device factory.
    std::list<std::unique_ptr<CodecListEntry>> hw_codecs;

    // Inspect data about this specific CodecFactory
    inspect::Node factory_node;
  };

  void DiscoverMagmaCodecDriversAndListenForMoreAsync();
  void DiscoverMediaCodecDriversAndListenForMoreAsync();

  void PublishService();
  void PostDiscoveryQueueProcessing();
  void ProcessDiscoveryQueue();
  void IdledCodecDiscovery();
  // Remove a magma codec from the discovery queue and codec lists and ensure
  // the discovery queue is processed correctly.
  void TeardownMagmaCodec(
      const std::shared_ptr<fuchsia::gpu::magma::IcdLoaderDevicePtr>& magma_device);

  [[nodiscard]] std::string GetBoardName();

  async_dispatcher_t* dispatcher_;
  ProdOrTest prod_or_test_;

  std::unique_ptr<sys::ComponentContext> startup_context_;
  std::unique_ptr<inspect::ComponentInspector> inspector_;
  inspect::Node hardware_factory_nodes_;
  std::string board_name_;

  // Per-board (or similar) policy on # of concurrent HW decoders that use
  // contiguous_memory_size before falling back to SW (or other similar sorts of
  // policy).
  CodecFactoryPolicy policy_;

  // We don't keep a fidl::BindingSet<> here, as we want each CodecFactory
  // instance to delete itself if an error occurs on its channel.
  //
  // The App layer is just here to create CodecFactory instances, each
  // independently bound to its own channel using a std::unique_ptr ImplPtr so
  // that if the channel closes, the CodecFactory instance will go away.  And
  // if the CodecFactory instance wants to self-destruct, it can delete the
  // binding, which will close the channel and delete the CodecFactory.
  // This is true despite comments in the binding code that constantly say how
  // ImplPtr isn't taking ownership; as long as we use std::unique_ptr as
  // ImplPtr it actually will take ownership.
  //
  // We create a new instance of CodecFactory for each interface request,
  // because CodecFactory's implementation isn't stateless, by design, for
  // more plausible interface evolution over time.

  // This maps from mime type to hw-based (driver-based) codec factory.  For
  // now, the first driver discovered that supports decoding a given mime type
  // will be chosen to decode that mime type, with an optional fallback to SW if
  // no driver supports the requested mime type.
  //
  // We rely on each create request being self-contained in the CodecFactory
  // interface.
  //
  // For now, items are added at the end of this list as codecs are discovered,
  // removed as channel failure is detected, and when looking for a HW codec the
  // first matching item in the list is selected, if any.
  //
  // This is only read or written from the main FIDL thread.
  //
  // As new devices are discovered, their codecs go at the start of the list and
  // will be used in favor of previously-discovered devices.  If an old device
  // devhost exits, its entry will be eventually removed from this list thanks
  // to that device's local CodecFactory channel closing.
  //
  // This list is ordered by reverse discovery order.
  std::list<std::unique_ptr<CodecFactoryEntry>> hw_factories_;

  CodecServiceMemberWatcher codec_service_watcher_;
  MagmaServiceMemberWatcher magma_service_watcher_;

  // This queue is to ensure we process discovered devices in the order
  // discovered, so that devices discovered later take priority over devices
  // discovered earlier.  We can be concurrently waiting for more than one
  // device's codec list, but we won't add a device's codec descriptions to
  // hw_codecs_ until temporally after all previously-discovered devices.
  //
  // This list is ordered by discovery order.
  struct DeviceDiscoveryEntry {
    // !driver_codec_list until GetDetailedCodecDescriptions response has been received.  This list
    // is _not_ based on the OnCodecList from each codec (which is now ignored), but rather is
    // constructed using the GetDetailedCodecDescriptions information, so that each codec only has
    // to serve the more recent GetDetailedCodecDescriptions, and can immediately stop sending the
    // deprecated OnCodecList, even while clients are still migrating from OnCodecList to
    // GetDetailedCodecDescriptions.
    std::optional<std::vector<fuchsia::mediacodec::CodecDescription>> driver_codec_list;

    // !detailed_codec_descriptions until a device is found and the |GetDetailedCodecDescriptions|
    // response is received.  Codecs must implement GetDetailedCodecDescriptions.
    std::optional<std::vector<fuchsia::mediacodec::DetailedCodecDescription>>
        detailed_codec_descriptions;

    // We don't really need a shared_ptr<> until hw_codecs_ (to allow it to be
    // just a flat list).  However, using a shared_ptr<> here seems more
    // readable than using unique_ptr<> here, especially given that
    // fuchsia::mediacodec::CodecFactoryPtr is very similar to a unique_ptr<>
    // itself.
    std::shared_ptr<fuchsia::mediacodec::CodecFactoryPtr> codec_factory;

    std::shared_ptr<fuchsia::gpu::magma::IcdLoaderDevicePtr> magma_device;

    std::string component_url;
  };
  std::list<std::unique_ptr<DeviceDiscoveryEntry>> device_discovery_queue_;
  bool existing_devices_discovered_ = false;

  sys::OutgoingDirectory outgoing_codec_aux_service_directory_parent_;
  vfs::PseudoDir* outgoing_codec_aux_service_directory_ = nullptr;

  uint32_t num_codec_discoveries_in_flight_{};

  uint64_t next_inspect_ordinal_ = 0;

  FXL_DISALLOW_COPY_AND_ASSIGN(CodecFactoryApp);
};

#endif  // SRC_MEDIA_CODEC_FACTORY_CODEC_FACTORY_APP_H_
