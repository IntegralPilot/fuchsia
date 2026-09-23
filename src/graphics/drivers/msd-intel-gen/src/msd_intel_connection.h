// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_INTEL_CONNECTION_H
#define MSD_INTEL_CONNECTION_H

#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

#include <list>
#include <memory>

#include "command_buffer.h"
#include "engine_command_streamer.h"
#include "msd_intel_pci_device.h"

class MsdIntelContext;

class MsdIntelConnection {
 public:
  class Owner : public PerProcessGtt::Owner {
   public:
    virtual ~Owner() = default;

    virtual void SubmitBatch(std::unique_ptr<MappedBatch> batch) = 0;
    virtual void DestroyContext(std::shared_ptr<MsdIntelContext> client_context) = 0;
  };

  static std::unique_ptr<MsdIntelConnection> Create(Owner* owner, msd::msd_client_id_t client_id);

  virtual ~MsdIntelConnection() {}

  std::shared_ptr<PerProcessGtt> per_process_gtt() { return ppgtt_; }

  msd::msd_client_id_t client_id() { return client_id_; }

  void SubmitBatch(std::unique_ptr<MappedBatch> batch) { owner_->SubmitBatch(std::move(batch)); }

  static std::shared_ptr<MsdIntelContext> CreateContext(
      std::shared_ptr<MsdIntelConnection> connection);

  void DestroyContext(std::shared_ptr<MsdIntelContext> context);

  void SetNotificationCallback(msd::NotificationHandler* handler) { notifications_.Set(handler); }

  // Called by the device thread when command buffers complete.
  void SendNotification(std::vector<uint64_t>& buffer_ids) {
    notifications_.SendBufferIds(buffer_ids);
  }

  void SendContextKilled() {
    notifications_.SendContextKilled();
    sent_context_killed_ = true;
  }

  // `Callback` should be of type zx_status_t(async_dispatcher_t*). It's called while holding the
  // notifications_ lock.
  template <typename Callback>
  std::optional<zx_status_t> CallWithDispatcher(Callback callback) {
    return notifications_.CallWithDispatcher(callback);
  }

  // Maps |page_count| pages of the given |buffer| at |page_offset| to |gpu_addr| into the
  // GPU address space belonging to this connection.
  magma::Status MapBufferGpu(std::shared_ptr<MsdIntelBuffer> buffer, uint64_t gpu_addr,
                             uint64_t page_offset, uint64_t page_count);

  void ReleaseBuffer(magma::PlatformBuffer* buffer);

  // A value chosen for historical reasons, just want to prevent sending channel messages
  // that are too large.
  static constexpr size_t kMaxUint64PerChannelSend = 510;

 private:
  MsdIntelConnection(Owner* owner, std::shared_ptr<PerProcessGtt> ppgtt,
                     msd::msd_client_id_t client_id)
      : owner_(owner), ppgtt_(std::move(ppgtt)), client_id_(client_id) {}

  bool sent_context_killed() { return sent_context_killed_; }

  // The given callback should return when any of the given semaphores are signaled.
  void ReleaseBuffer(
      magma::PlatformBuffer* buffer,
      std::function<magma::Status(
          std::vector<std::shared_ptr<magma::PlatformSemaphore>>& semaphores, uint32_t timeout_ms)>
          wait_callback);

  Owner* owner_;
  std::shared_ptr<PerProcessGtt> ppgtt_;
  msd::msd_client_id_t client_id_;
  bool sent_context_killed_ = false;
  std::list<std::shared_ptr<MsdIntelContext>> context_list_;

  class Notifications {
   public:
    void SendBufferIds(std::vector<uint64_t>& buffer_ids) {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!handler_)
        return;

      for (size_t src_index = 0; src_index < buffer_ids.size();) {
        size_t count = std::min(buffer_ids.size() - src_index, kMaxUint64PerChannelSend);

        auto start = reinterpret_cast<uint8_t*>(&buffer_ids[src_index]);
        auto end = reinterpret_cast<uint8_t*>(&buffer_ids[src_index + count]);

        handler_->NotificationChannelSend(cpp20::span(start, end));

        src_index += count;
      }
    }

    void SendContextKilled() {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!handler_)
        return;

      handler_->ContextKilled();
    }

    template <typename Callback>
    std::optional<zx_status_t> CallWithDispatcher(Callback callback) {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!handler_)
        return std::nullopt;
      return {callback(handler_->GetAsyncDispatcher())};
    }

    void Set(msd::NotificationHandler* handler) {
      std::lock_guard<std::mutex> lock(mutex_);
      handler_ = handler;
    }

   private:
    msd::NotificationHandler* handler_ = nullptr;
    std::mutex mutex_;
  };

  Notifications notifications_;

  friend class TestMsdIntelConnection;
};

class MsdIntelAbiConnection : public msd::Connection {
 public:
  explicit MsdIntelAbiConnection(std::shared_ptr<MsdIntelConnection> ptr) : ptr_(std::move(ptr)) {}

  // msd::Connection impl
  magma_status_t MsdMapBuffer(msd::Buffer& buffer, uint64_t gpu_va, uint64_t offset,
                              uint64_t length, uint64_t flags) override;
  void MsdReleaseBuffer(msd::Buffer& buffer, bool shutting_down) override;

  void MsdSetNotificationCallback(msd::NotificationHandler* handler) override;

  std::unique_ptr<msd::Context> MsdCreateContext() override;

  std::shared_ptr<MsdIntelConnection> ptr() { return ptr_; }

 private:
  std::shared_ptr<MsdIntelConnection> ptr_;
};

#endif  // MSD_INTEL_CONNECTION_H
