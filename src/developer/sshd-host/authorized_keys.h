// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_SSHD_HOST_AUTHORIZED_KEYS_H_
#define SRC_DEVELOPER_SSHD_HOST_AUTHORIZED_KEYS_H_

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <zircon/status.h>

namespace sshd_host {

// This function is primarily for debugging. In some cases the system can get
// into a state where we can't access the ssh keys, so we want to explicitly log
// when this is the case.
void check_authorized_keys();

// Looks for an authorized_keys file passed from the bootloader, and if found persists it to disk.
//
// If keys already exist on disk, this is a no-op and will not overwrite them.
//
// Returns `ZX_OK` on the normal cases: no authorized_keys were passed from the bootloader, or the
// keys were passed and were successfully written to disk.
zx_status_t provision_authorized_keys_from_bootloader_file(
    fidl::SyncClient<fuchsia_boot::Items>& boot_items);

}  // namespace sshd_host

#endif  // SRC_DEVELOPER_SSHD_HOST_AUTHORIZED_KEYS_H_
