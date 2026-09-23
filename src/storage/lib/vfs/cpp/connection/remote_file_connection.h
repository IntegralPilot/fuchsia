// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_CONNECTION_REMOTE_FILE_CONNECTION_H_
#define SRC_STORAGE_LIB_VFS_CPP_CONNECTION_REMOTE_FILE_CONNECTION_H_

#ifndef __Fuchsia__
#error "Fuchsia-only header"
#endif

#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/connection/file_connection.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs::internal {

class RemoteFileConnection final : public FileConnection {
 public:
  // Refer to documentation for |Connection::Connection|.
  RemoteFileConnection(fs::FuchsiaVfs* vfs, fbl::RefPtr<fs::Vnode> vnode, fuchsia_io::Rights rights,
                       bool append, zx_koid_t koid);

  ~RemoteFileConnection() final = default;

 private:
  bool GetAppend() const final { return append_; }
  zx::result<> SetAppend(bool append) final {
    append_ = append;
    return zx::ok();
  }

  //
  // |fuchsia.io/File| operations.
  //

  void Read(ReadRequestView request, ReadCompleter::Sync& completer) final;
  void ReadAt(ReadAtRequestView request, ReadAtCompleter::Sync& completer) final;
  void Write(WriteRequestView request, WriteCompleter::Sync& completer) final;
  void WriteAt(WriteAtRequestView request, WriteAtCompleter::Sync& completer) final;
  void Seek(SeekRequestView request, SeekCompleter::Sync& completer) final;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_io::File> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) final {}

  zx_status_t ReadInternal(void* data, size_t len, size_t* out_actual);
  zx_status_t ReadAtInternal(void* data, size_t len, size_t offset, size_t* out_actual);
  zx_status_t WriteInternal(const void* data, size_t len, size_t* out_actual);
  zx_status_t WriteAtInternal(const void* data, size_t len, size_t offset, size_t* out_actual);
  zx_status_t SeekInternal(fuchsia_io::wire::SeekOrigin origin, int64_t offset);

  // Current seek offset.
  size_t offset_ = 0;
  bool append_;
};

}  // namespace fs::internal

#endif  // SRC_STORAGE_LIB_VFS_CPP_CONNECTION_REMOTE_FILE_CONNECTION_H_
