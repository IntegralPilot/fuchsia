// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_USB_DEVICE_STREAM_IMPL_H_
#define SRC_CAMERA_BIN_USB_DEVICE_STREAM_IMPL_H_

#include <fuchsia/camera/cpp/fidl.h>
#include <fuchsia/camera3/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include <memory>
#include <queue>
#include <set>
#include <vector>

#include "src/camera/bin/usb_device/frame_waiter.h"
#include "src/camera/bin/usb_device/util_fidl.h"
#include "src/camera/bin/usb_device/uvc_hack.h"
#include "src/camera/lib/actor/actor_base.h"
#include "src/camera/lib/hanging_get_helper/hanging_get_helper.h"

namespace camera {

// Represents a specific stream in a camera device's configuration. Serves multiple clients of the
// camera3.Stream protocol.
class StreamImpl : public actor::ActorBase {
 public:
  template <typename RetT_, typename ErrT_ = void>
  using promise = fpromise::promise<RetT_, ErrT_>;

  // Called by the stream when it needs to connect to its associated legacy stream.
  using StreamRequestedCallback =
      fit::function<promise<void>(fuchsia::sysmem::BufferCollectionInfo, fuchsia::camera::FrameRate,
                                  fidl::InterfaceRequest<fuchsia::camera::Stream>, zx::eventpair)>;
  // Called by the stream when it needs to call BindSharedCollection.
  using AllocatorBindSharedCollectionCallback =
      fit::function<promise<void>(fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken>,
                                  fidl::InterfaceRequest<fuchsia::sysmem2::BufferCollection>)>;
  // Called by the stream to register a BufferCollection deallocation devent.
  using RegisterDeallocationEvent = fit::function<promise<void>(zx::eventpair deallocation_event)>;
  // Called by the stream when it has no more clients.
  using NoClientsCallback = fit::function<promise<void>()>;

  StreamImpl(async_dispatcher_t* dispatcher, const fuchsia::camera3::StreamProperties2& properties,
             fidl::InterfaceRequest<fuchsia::camera3::Stream> request,
             StreamRequestedCallback on_stream_requested,
             AllocatorBindSharedCollectionCallback allocator_bind_shared_collection,
             RegisterDeallocationEvent register_deallocation_event, NoClientsCallback on_no_clients,
             std::optional<std::string> description = std::nullopt);
  ~StreamImpl() = default;

  // Ask UVC driver to stop streaming.
  promise<void> StopStreaming();

  // Close all client connections with given status as epitaph.
  promise<void> CloseAllClients(zx_status_t status);

 private:
  // Schedules a cleanup.
  void OnError(zx_status_t status, const std::string& message);

  // Cleanup promise. Unmaps and closes VMOs and closes client BufferCollection. Disconnects all
  // clients and calls the on_no_clients_ callback. If status != ZX_OK, logs at error level.
  promise<void> Cleanup(zx_status_t status, const std::string& message);

  // Called when a client calls Rebind.
  void OnNewRequest(fidl::InterfaceRequest<fuchsia::camera3::Stream> request);

  // Remove the client with the given id.
  promise<void> RemoveClient(uint64_t id);

  // Called when the legacy stream's OnFrameAvailable event fires.
  void OnFrameAvailable(fuchsia::camera::FrameAvailableEvent info);

  // Calls Sync on the given token and then produces that token when the sync is complete.
  promise<fuchsia::sysmem2::BufferCollectionTokenPtr> TokenSync(
      fuchsia::sysmem2::BufferCollectionTokenPtr token);

  // Calls Sync on the given BufferCollection and the promise completes when the sync is complete.
  promise<void> BufferCollectionSync(fuchsia::sysmem2::BufferCollectionPtr& collection);

  // Calls WaitForBuffersAllocated on client_buffer_collection_ and then returns either a status in
  // the case of an error or the BufferCollectionInfo if it was successful.
  promise<fuchsia::sysmem2::BufferCollectionInfo, zx_status_t>
  WaitForClientBufferCollectionAllocated();

  // Given an InterfaceHandle<BufferCollectionToken> with a valid underlying channel, the client
  // will be set as a participant and the returned promise will yield the bound token for further
  // use. Otherwise, the client will not be a participant and the promise will yield nullopt.
  promise<std::optional<fuchsia::sysmem2::BufferCollectionTokenPtr>> SetClientParticipation(
      uint64_t id, fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token_handle);

  // Renegotiate buffers or opt out of buffer renegotiation for the client with the given id.
  promise<fuchsia::sysmem::BufferCollectionInfo, zx_status_t> InitBufferCollections(
      fuchsia::sysmem2::BufferCollectionTokenPtr token);

  // The following functions are async daisy-chained to execute the steps to initialize the
  // client-facing buffer collection:
  //
  // 1. InitializeSharedCollection - Bind shared collection and set attributes
  // 2. SetClientBufferCollectionConstraints - Set constraints
  // 3. WaitForClientBufferCollectionAllocated - Wait for the allocation to finish
  // 4. RegisterDeallocationEvent - so it is known when the client BufferCollection is deallocated
  // 5. InitializeClientBuffers - Initialize individual buffers
  promise<void> InitializeClientSharedCollection(fuchsia::sysmem2::BufferCollectionTokenPtr token);
  promise<void> SetClientBufferCollectionConstraints();
  promise<void> RegisterClientBufferCollectionDeallocationEvent();
  promise<void> InitializeClientBuffers(
      fuchsia::sysmem2::BufferCollectionInfo buffer_collection_info);

  // Allocate the driver-facing buffer collection.
  promise<fuchsia::sysmem::BufferCollectionInfo, zx_status_t> AllocateDriverBufferCollection(
      fuchsia::camera::VideoFormat video_format);

  // Connect to UVC camera stream and start streaming.
  promise<void> ConnectAndStartStream(fuchsia::sysmem::BufferCollectionInfo buffer_collection_info);

  // Setup our end of the UVC camera stream and connect to the UVC camera stream.
  promise<void> ConnectToStream(fuchsia::sysmem::BufferCollectionInfo buffer_collection_info,
                                fuchsia::camera::FrameRate frame_rate);

  // Ask UVC driver to start streaming.
  promise<void> StartStreaming();

  // Our local stub to call BindSharedCollection at DeviceImpl.
  void AllocatorBindSharedCollection(
      fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token_handle,
      fidl::InterfaceRequest<fuchsia::sysmem2::BufferCollection> request);

  // Process incoming UVC camera frame in preparation for sending to client.
  void ProcessFrameForSend(uint32_t buffer_id);

  // Tell UVC driver that a driver-facing buffer can be returned to the free pool.
  void ReleaseClientFrame(uint32_t buffer_id);

  // Represents a single client connection to the StreamImpl class.
  class Client : public fuchsia::camera3::Stream {
   public:
    Client(StreamImpl& stream, uint64_t id,
           fidl::InterfaceRequest<fuchsia::camera3::Stream> request);
    ~Client() override;

    // Add a frame to the queue of available frames.
    void AddFrame(fuchsia::camera3::FrameInfo2 frame);

    // Send a frame to the client if one is available and has been requested.
    void MaybeSendFrame();

    // Closes |binding_| with the provided |status| epitaph, and removes the client instance from
    // the parent |clients_| map.
    void CloseConnection(zx_status_t status);

    // Add the given token to the client's token queue.
    void ReceiveBufferCollection(fuchsia::sysmem2::BufferCollectionTokenHandle token);

    // Update the client's resolution.
    void ReceiveResolution(fuchsia::math::Size coded_size);

    // Update the client's crop region.
    void ReceiveCropRegion(std::unique_ptr<fuchsia::math::RectF> region);

    // Returns a mutable reference to this client's state as a participant in buffer renegotiation.
    bool& Participant();

    // Clears the client's queue of unsent frames.
    void ClearFrames();

   private:
    // Called when the client endpoint of |binding_| is closed.
    void OnClientDisconnected(zx_status_t status);

    // |fuchsia::camera3::Stream|
    void GetProperties(GetPropertiesCallback callback) override;
    void GetProperties2(GetProperties2Callback callback) override;
    void SetCropRegion(std::unique_ptr<fuchsia::math::RectF> region) override;
    void WatchCropRegion(WatchCropRegionCallback callback) override;
    void SetResolution(fuchsia::math::Size coded_size) override;
    void WatchResolution(WatchResolutionCallback callback) override;
    void SetBufferCollection2(
        fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) override;
    void SetBufferCollection(
        fidl::InterfaceHandle<fuchsia::sysmem::BufferCollectionToken> token) override;
    void WatchBufferCollection2(WatchBufferCollection2Callback callback) override;
    void WatchBufferCollection(WatchBufferCollectionCallback callback) override;
    void WatchOrientation(WatchOrientationCallback callback) override;
    void GetNextFrame(GetNextFrameCallback callback) override;
    void GetNextFrame2(GetNextFrame2Callback callback) override;
    void Rebind(fidl::InterfaceRequest<Stream> request) override;

    void SetBufferCollectionCommon(
        fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token);
    void WatchBufferCollectionCommon(
        fit::function<void(fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken>)>
            callback);

    StreamImpl& stream_;
    std::string log_prefix_;
    // Tracking for whether a message has already been logged for the named stage of client
    // progress. This is used to ensure that only one such message is logged per transition per
    // client, as they are high-frequency events that would otherwise spam syslog.
    struct {
      // The client has called camera3::Stream::GetNextFrame.
      bool requested = false;
      // The parent stream has added a frame to this client's available frame queue by calling
      // AddFrame.
      bool available = false;
      // A frame has been sent to the client by invoking the callback provided in a previous call to
      // GetNextFrame.
      bool sent = false;
    } frame_logging_state_;
    uint64_t id_;
    fidl::Binding<fuchsia::camera3::Stream> binding_;
    HangingGetHelper<fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken>> buffers_;
    HangingGetHelper<fuchsia::math::Size,
                     fit::function<bool(fuchsia::math::Size, fuchsia::math::Size)>>
        resolution_;
    HangingGetHelper<std::unique_ptr<fuchsia::math::RectF>> crop_region_;
    GetNextFrame2Callback frame_callback_;
    bool participant_ = false;
    std::queue<fuchsia::camera3::FrameInfo2> frames_;
    WatchOrientationCallback orientation_callback_;
  };

  async_dispatcher_t* dispatcher_;
  fuchsia::sysmem2::AllocatorPtr allocator_;
  const fuchsia::camera3::StreamProperties2& properties_;
  std::map<uint64_t, std::unique_ptr<Client>> clients_;
  uint64_t client_id_next_ = 1;
  StreamRequestedCallback on_stream_requested_;
  AllocatorBindSharedCollectionCallback allocator_bind_shared_collection_;
  RegisterDeallocationEvent register_deallocation_event_;
  NoClientsCallback on_no_clients_;
  uint32_t max_camping_buffers_ = kUvcHackClientMaxBufferCountForCamping;

  // USB video stream
  fuchsia::camera::StreamPtr stream_;

  // USB video stream token used to signal connection drop
  zx::eventpair stream_token_;

  // Client-facing buffer collection
  fuchsia::sysmem2::BufferCollectionInfo client_buffer_collection_info_;
  fuchsia::sysmem2::BufferCollectionPtr client_buffer_collection_;

  // Driver-facing buffer collection
  fuchsia::sysmem2::BufferCollectionInfo driver_buffer_collection_info_;

  // Maps client-facing buffer ID to the buffer's virtual address.
  std::map<uint32_t, uintptr_t> client_buffer_id_to_virt_addr_;

  // Maps driver-facing buffer ID to the buffer's virtual address.
  std::map<uint32_t, uintptr_t> driver_buffer_id_to_virt_addr_;

  // Buffer ID's of client-facing buffers that are not in use.
  std::queue<uint32_t> client_buffer_free_queue_;

  // Is the USB video driver streaming?
  bool streaming_ = false;

  uint64_t frame_counter_ = 0;
  std::map<uint32_t, std::unique_ptr<FrameWaiter>> frame_waiters_;
  fuchsia::math::Size current_resolution_;
  std::unique_ptr<fuchsia::math::RectF> current_crop_region_;
  std::string description_;

  // This should always be the last thing in the object. Otherwise scheduled tasks within this scope
  // which reference members of this object may be allowed to run after destruction of this object
  // has started. Keeping this at the end ensures that the scope is destroyed first, cancelling any
  // scheduled tasks before the rest of the members are destroyed.
  fpromise::scope scope_;
  friend class Client;
};

}  // namespace camera

#endif  // SRC_CAMERA_BIN_USB_DEVICE_STREAM_IMPL_H_
