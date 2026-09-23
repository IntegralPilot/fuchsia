// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VFS_CPP_PSEUDO_FILE_H_
#define LIB_VFS_CPP_PSEUDO_FILE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/vfs/cpp/node.h>

#include <vector>

namespace vfs {

// Buffered pseudo-file.
//
// This variant is optimized for incrementally reading and writing properties which are larger than
// can typically be read or written by the client in a single I/O transaction.
//
// In read mode, the pseudo-file invokes its read handler when the file is opened and retains the
// content in an output buffer which the client incrementally reads from and can seek within.
//
// In write mode, the client incrementally writes into and seeks within an input buffer which the
// pseudo-file delivers as a whole to the write handler when the file is closed.  Truncation is also
// supported.
//
// Each client has its own separate output and input buffers.  Writing into the output buffer does
// not affect the contents of the client's input buffer or that of any other client.  Changes to the
// underlying state of the pseudo-file are not observed by the client until it closes and re-opens
// the file.
//
// This class is thread-safe.
class PseudoFile final : public Node {
 public:
  // Handler called to read from the pseudo-file.
  using ReadHandler = fit::function<zx_status_t(std::vector<uint8_t>* output, size_t max_bytes)>;

  // Handler called to write into the pseudo-file.
  using WriteHandler = fit::function<zx_status_t(std::vector<uint8_t> input)>;

  // Creates a buffered pseudo-file.
  //
  // `read_handler` cannot be null. If the `write_handler` is null, then the pseudo-file is
  // considered not writable. `max_file_size` determines the maximum number of bytes which can be
  // written to and read from the pseudo-file's input buffer when it it opened for writing/reading.
  explicit PseudoFile(size_t max_file_size, ReadHandler read_handler = nullptr,
                      WriteHandler write_handler = nullptr)
      : Node(MakePseudoFile(max_file_size, std::move(read_handler), std::move(write_handler))) {}

  // Serve a new connection to this pseudo-file on `server_end` using specified `flags`.
  //
  // This method must only be used with a single-threaded asynchronous dispatcher. If `dispatcher`
  // is `nullptr`, the current thread's default dispatcher will be used via
  // `async_get_default_dispatcher`. The same `dispatcher` must be used if multiple connections are
  // served for the same node, otherwise `ZX_ERR_INVALID_ARGS` will be returned.
  zx_status_t Serve(fuchsia_io::Flags flags, fidl::ServerEnd<fuchsia_io::File> server_end,
                    async_dispatcher_t* dispatcher = nullptr) {
    if (flags & (fuchsia_io::wire::kMaskKnownProtocols ^ fuchsia_io::Flags::kProtocolFile)) {
      return ZX_ERR_INVALID_ARGS;  // Only the file protocol is allowed with this signature.
    }
    return ServeInternal(flags | fuchsia_io::Flags::kProtocolFile, server_end.TakeChannel(),
                         dispatcher);
  }

 private:
  struct PseudoFileState {
    const ReadHandler read_handler;
    const WriteHandler write_handler;
    const size_t max_size;
    std::vector<uint8_t> buffer;  // Temporary buffer used to store owned data until it's copied.
  };

  static vfs_internal_node_t* MakePseudoFile(size_t max_file_size, ReadHandler read_handler,
                                             WriteHandler write_handler) {
    ZX_ASSERT(read_handler);
    vfs_internal_node_t* file;
    PseudoFileState* cookie = new PseudoFileState{
        .read_handler = std::move(read_handler),
        .write_handler = std::move(write_handler),
        .max_size = max_file_size,
    };
    vfs_internal_file_context_t context{
        .cookie = cookie,
        .read = &ReadCallback,
        .release = &ReleaseCallback,
        .write = cookie->write_handler ? &WriteCallback : nullptr,
        .destroy = &DestroyCookie,
    };

    ZX_ASSERT(vfs_internal_pseudo_file_create(max_file_size, &context, &file) == ZX_OK);
    return file;
  }

  static void DestroyCookie(void* cookie) { delete static_cast<PseudoFileState*>(cookie); }

  static zx_status_t ReadCallback(void* cookie, const char** data_out, size_t* len_out) {
    PseudoFileState& state = *static_cast<PseudoFileState*>(cookie);
    if (zx_status_t status = state.read_handler(&state.buffer, state.max_size); status != ZX_OK) {
      return status;
    }
    *data_out = reinterpret_cast<const char*>(state.buffer.data());
    *len_out = state.buffer.size();
    return ZX_OK;
  }

  static void ReleaseCallback(void* cookie) {
    PseudoFileState& state = *static_cast<PseudoFileState*>(cookie);
    state.buffer.clear();
  }

  static zx_status_t WriteCallback(const void* cookie, const char* data, size_t len) {
    const PseudoFileState& state = *static_cast<const PseudoFileState*>(cookie);
    const uint8_t* begin = reinterpret_cast<const uint8_t*>(data);
    std::vector<uint8_t> contents(begin, begin + len);
    return state.write_handler(std::move(contents));
  }
};

}  // namespace vfs

#endif  // LIB_VFS_CPP_PSEUDO_FILE_H_
