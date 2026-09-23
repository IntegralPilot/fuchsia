// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/lib/usb-monitor-util/include/usb-monitor-util/usb-monitor-util.h"

#include <fuchsia/hardware/usb/request/c/banjo.h>
#include <lib/trace/event.h>

#include <fbl/auto_lock.h>
#include <usb/usb.h>

void UsbMonitor::Start() {
  fbl::AutoLock start_lock(&mutex_);
  if (!started_) {
    TRACE_INSTANT("UsbMonitorUtil", "START", TRACE_SCOPE_PROCESS);
    started_ = true;
  }
}

void UsbMonitor::Stop() {
  fbl::AutoLock<fbl::Mutex> start_lock(&mutex_);
  if (started_) {
    TRACE_INSTANT("UsbMonitorUtil", "STOP", TRACE_SCOPE_PROCESS);
    started_ = false;
  }
}

bool UsbMonitor::Started() const {
  fbl::AutoLock start_lock(&mutex_);
  return started_;
}

void UsbMonitor::AddRecord(usb_request_t* request) {
  fbl::AutoLock<fbl::Mutex> start_lock(&mutex_);

  if (started_) {
    ++num_records_;
    TRACE_INSTANT("UsbMonitorUtil", "Record Added", TRACE_SCOPE_GLOBAL, "ep_num",
                  TA_UINT32(usb_ep_num2(request->header.ep_address)), "device_id",
                  request->header.device_id, "length", request->header.length, "frame",
                  TA_UINT64(request->header.frame), "direct", TA_BOOL(request->direct));
  }
}

UsbMonitorStats UsbMonitor::GetStats() const { return UsbMonitorStats{num_records_.load()}; }
