// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef DEVICE_REQUEST_H
#define DEVICE_REQUEST_H

#include <lib/magma/platform/platform_event.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/status.h>

#include <memory>

template <class Processor>
class DeviceRequest {
 public:
  virtual ~DeviceRequest() {}

  class Reply {
   public:
    Reply() : status_(MAGMA_STATUS_OK), event_(magma::PlatformEvent::Create()) { DASSERT(event_); }

    void Signal(magma::Status status) {
      status_ = status;
      event_->Signal();
    }

    magma::Status Wait() {
      event_->Wait();
      return status_;
    }

   private:
    magma::Status status_;
    std::unique_ptr<magma::PlatformEvent> event_;
  };

  std::shared_ptr<Reply> GetReply() {
    if (!reply_)
      reply_ = std::shared_ptr<Reply>(new Reply());
    return reply_;
  }

  void ProcessAndReply(Processor* processor) {
    magma::Status status = Process(processor);

    if (reply_)
      reply_->Signal(status);
  }

  virtual uint8_t RequestType() = 0;

 protected:
  virtual magma::Status Process(Processor* processor) { return MAGMA_STATUS_OK; }

 private:
  std::shared_ptr<Reply> reply_;
};

#endif
