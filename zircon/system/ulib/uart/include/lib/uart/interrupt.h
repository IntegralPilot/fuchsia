// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_IRQ_H_
#define LIB_UART_IRQ_H_

#include <lib/zircon-internal/thread_annotations.h>

#include <utility>

namespace uart {

// Rx IRQ Handler's will be provided an instance of this class, providing the thread requirements
// for calling each of the supported APIs. The synchronization cannot be enforced directly as a
// protected object since the `Lock` encompasses more than the UART itself in some environments.
template <typename Lock, typename Reader, typename Disabler>
class RxInterrupt {
 public:
  RxInterrupt(Lock& lock, Reader&& reader, Disabler&& disabler)
      : lock_(lock),
        reader_(std::forward<Reader>(reader)),
        disabler_(std::forward<Disabler>(disabler)) {}

  // Returns characters from performing one read operation from the UART.
  auto ReadChar() TA_REQ(lock_) { return reader_(); }

  // Mask RX IRQ and terminates the loop, even if we could still read more characters from the uart.
  // Usually means that the buffer where characters are being written is full.
  auto DisableInterrupt() TA_REQ(lock_) { return disabler_(); }

  // In some cases it is desirable to control the locking sequence. Some of these caes involve
  // making sure certain TOCTOU operations do not leave the UART in an invalid state.
  Lock& lock() TA_RET_CAP(lock_) { return lock_; }

 private:
  Lock& lock_;
  Reader reader_;
  Disabler disabler_;
};

// Tx IRQ Handler's will be provided an instance of this class, providing the thread requirements
// for calling each of the supported APIs. The synchronization cannot be enforced directly as a
// protected object since the `Lock` encompasses more than the UART itself in some environments.
template <typename Lock, typename Waiter, typename Disabler>
class TxInterrupt {
 public:
  TxInterrupt(Lock& lock, Waiter& waiter, Disabler&& disabler)
      : lock_(lock), waiter_(waiter), disabler_(std::forward<Disabler>(disabler)) {}

  // Notifies blocked threads, that there is space available in the UART TX Fifo.
  auto Notify() TA_EXCL(lock_) { return waiter_.Wake(); }

  // Disables TX IRQ, usually done when the TX HW Fifo is not empty, such that we can
  // efficientlly write larger chunks of data without having to block on individual characters.
  auto DisableInterrupt() TA_REQ(lock_) { return disabler_(); }

  // In some scenarios it is desirable to control the locking sequence. While unlikely in the TX
  // path, it shows symmertry with the RX handler.
  auto& lock() TA_RET_CAP(lock_) { return lock_; }

 private:
  Lock& lock_;
  Waiter& waiter_;
  Disabler disabler_;
};

}  // namespace uart

#endif  // LIB_UART_IRQ_H_
