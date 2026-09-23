// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_RAMDISK_H_
#define SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_RAMDISK_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <stdlib.h>
#include <zircon/compiler.h>
#include <zircon/hw/gpt.h>
#include <zircon/types.h>

#include <array>
#include <optional>
#include <string>

__BEGIN_CDECLS

// TODO(https://446695911): Remove the C interface.  Clients should use `ramdevice_client::Ramdisk`.
// A client library for creating, configuring and manipulating ramdisks.
//
// When creating a ramdisk, ensure the fuchsia.hardware.ramdisk.Service is available.
// Then a ram device can be created and opened.
// ```
// ramdisk_client_t* client;
// ASSERT_EQ(ramdisk_create(512, 2048, &client), ZX_OK);
// zx_handle_t block_client = ramdisk_get_block_interface(client);
// ```
struct ramdisk_client;
typedef struct ramdisk_client ramdisk_client_t;

typedef struct ramdisk_options {
  uint32_t block_size;
  uint64_t block_count;
  const uint8_t* type_guid;
  zx_handle_t vmo;
  // Set to -1 to open /svc
  int svc_root_fd;
} ramdisk_options_t;

// Creates a ramdisk with the specified options.
zx_status_t ramdisk_create_with_options(const ramdisk_options_t* options, ramdisk_client_t** out);

// Returns the handle to the block device interface of the client.
//
// Does not transfer ownership of the handle.
zx_handle_t ramdisk_get_block_interface(const ramdisk_client_t* client);

// Returns the handle to the fuchsia.device/Controller interface of the block device.
//
// Does not transfer ownership of the handle.
zx_handle_t ramdisk_get_block_controller_interface(const ramdisk_client_t* client);

// Returns the path to the full block device interface of the ramdisk.
const char* ramdisk_get_path(const ramdisk_client_t* client);

// Puts the ramdisk at |ramdisk_path| to sleep after |blk_count| blocks written.
// After this, transactions will no longer be immediately persisted to disk.
// If the |RAMDISK_FLAG_RESUME_ON_WAKE| flag has been set, transactions will
// be processed when |ramdisk_wake| is called, otherwise they will fail immediately.
zx_status_t ramdisk_sleep_after(const ramdisk_client_t* client, uint64_t blk_count);

// Wake the ramdisk at |ramdisk_path| from a sleep state.
zx_status_t ramdisk_wake(const ramdisk_client_t* client);

// A struct containing the number of write operations transmitted to the ramdisk
// since the last invocation of "wake" or "sleep_after".
typedef struct ramdisk_block_write_counts {
  uint64_t received;
  uint64_t successful;
  uint64_t failed;
} ramdisk_block_write_counts_t;

// Returns the ramdisk's current failed, successful, and total block counts as |counts|.
zx_status_t ramdisk_get_block_counts(const ramdisk_client_t* client,
                                     ramdisk_block_write_counts_t* out_counts);

// Sets flags on a ramdisk. Flags are plumbed directly through IPC interface.
zx_status_t ramdisk_set_flags(const ramdisk_client_t* client, uint32_t flags);

// Rebinds a ramdisk.
zx_status_t ramdisk_rebind(ramdisk_client_t* client);

// Unbind and destroy the ramdisk, and delete |client|.
zx_status_t ramdisk_destroy(ramdisk_client_t* client);

// Delete |client| *without* unbinding/destroying the ramdisk itself.
zx_status_t ramdisk_forget(ramdisk_client_t* client);

__END_CDECLS

namespace ramdevice_client {

// Manages a ramdisk instance.
class Ramdisk {
 public:
  struct Options {
    // If set, the ram-disk will report this type guid using the partition protocol.
    std::optional<std::array<uint8_t, GPT_GUID_LEN>> type_guid;
  };
  static constexpr Options kDefaultOptions;

  // Creates a ram-disk with |block_count| blocks of |block_size| bytes.
  // `svc_root_fd` can be overridden if desired; otherwise, "/svc" is opened to find
  // fuchsia.hardware.ramdisk.Service.
  static zx::result<Ramdisk> Create(int block_size, uint64_t block_count,
                                    std::optional<int> svc_root_fd = std::nullopt,
                                    const Options& options = kDefaultOptions);

  // Creates a ram-disk with the given VMO.  If block_size is zero, a default block size is used.
  // `svc_root_fd` can be overridden if desired; otherwise, "/svc" is opened to find
  // fuchsia.hardware.ramdisk.Service.
  static zx::result<Ramdisk> CreateWithVmo(zx::vmo vmo, uint64_t block_size = 0,
                                           std::optional<int> svc_root_fd = std::nullopt,
                                           const Options& options = kDefaultOptions);

  Ramdisk() = default;
  Ramdisk(Ramdisk&& other) noexcept : client_(other.client_) { other.client_ = nullptr; }
  Ramdisk& operator=(Ramdisk&& other) noexcept {
    if (this == &other) {
      return *this;
    }
    Reset();
    client_ = other.client_;
    other.client_ = nullptr;
    return *this;
  }

  ~Ramdisk() { Reset(); }

  // Frees the resources associated with the Ramdisk.  It is an error to call any methods after this
  // has been called.
  void Reset() {
    if (client_) {
      ramdisk_destroy(client_);
    }
    client_ = nullptr;
  }

  bool is_valid() const { return client_; }

  // TODO(https://fxbug.dev/446695911: Remove.
  ramdisk_client_t* client() const { return client_; }

  // Returns the path to the device.
  std::string path() const { return ramdisk_get_path(client_); }

  // Creates a new connection to the Block protocol served by the ramdisk.
  zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> ConnectBlock() const;

  // Reinds the ramdisk (synchronously, so the ramdisk will be ready after this returns).
  zx::result<> Rebind() { return zx::make_result(ramdisk_rebind(client_)); }

  // Puts the ramdisk to sleep after |blk_count| blocks written.  After this, transactions will no
  // longer be immediately persisted to disk.
  // If the |RAMDISK_FLAG_RESUME_ON_WAKE| flag has been set, transactions will be processed when
  // |ramdisk_wake| is called, otherwise they will fail immediately.
  zx::result<> SleepAfter(uint64_t block_count) {
    return zx::make_result(ramdisk_sleep_after(client_, block_count));
  }

  // Wakes up the ramdisk.
  zx::result<> Wake() { return zx::make_result(ramdisk_wake(client_)); }

  // Sets flags on the ramdisk (see fuchsia.hardware.ramdisk.RamdiskFlag).
  zx::result<> SetFlags(uint32_t flags) {
    return zx::make_result(ramdisk_set_flags(client_, flags));
  }

  // Returns the ramdisk's current failed, successful, and total block counts.
  zx::result<ramdisk_block_write_counts_t> GetBlockCounts() const {
    ramdisk_block_write_counts_t counts;
    if (zx_status_t status = ramdisk_get_block_counts(client_, &counts); status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(counts);
  }

 private:
  explicit Ramdisk(ramdisk_client_t* client) : client_(client) {}

  ramdisk_client_t* client_ = nullptr;
};

}  // namespace ramdevice_client

#endif  // SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_RAMDISK_H_
