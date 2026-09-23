// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_ANONYMOUS_PAGE_REQUEST_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_ANONYMOUS_PAGE_REQUEST_H_

#include <lib/zx/result.h>
#include <zircon/types.h>

#include <vm/page.h>

// Helper around tracking and performing waits for the PMM to be able to succeed waitable
// allocations. This is intended to a have a similar Wait method as a regular PageRequest to provide
// a consistent interface. Users are unlikely to want to use this directly, and instead probably
// want a MultiPageRequest.
// This class is not thread safe.
class AnonymousPageRequest {
 public:
  AnonymousPageRequest() = default;
  ~AnonymousPageRequest() {
    if (has_page()) {
      FreePage();
    }
  }

  // Make the request active, causing any future Wait calls to block on the PMM.
  // Caller must make sure no page is allocated (`!has_page()`) before calling `MakeActive()`.
  void MakeActive() {
    ZX_DEBUG_ASSERT(!has_page());
    active_ = true;
  }

  // Returns whether the request is active or not.
  bool is_active() const { return active_; }

  // Make the request inactive, if it was currently active. As this class is not thread safe, it
  // assumes there are no parallel calls to Wait that would need to be interrupted.
  void Cancel() { active_ = false; }

  // On success, a page is allocated.
  zx::result<> Allocate(bool suspendable = true);

  // Retrieves a possibly allocated `page_request`.
  vm_page_t* take_page() { return ktl::exchange(page_, nullptr); }

  constexpr bool has_page() const { return page_ != nullptr; }

 private:
  void FreePage();

  bool active_ = false;
  vm_page_t* page_ = nullptr;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_ANONYMOUS_PAGE_REQUEST_H_
