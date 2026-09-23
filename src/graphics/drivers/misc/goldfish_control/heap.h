// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_HEAP_H_
#define SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_HEAP_H_

#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/debug.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/function.h>

#include <memory>
#include <string>

#include <fbl/intrusive_double_list.h>

namespace goldfish {

class Control;
using HeapServer = fidl::WireServer<fuchsia_hardware_sysmem::Heap>;

using BufferKey = std::pair<uint64_t, uint32_t>;
struct BufferKeyHash {
 public:
  size_t operator()(const BufferKey& buffer_key) const {
    static std::hash<decltype(BufferKey::first)> first_hasher;
    static std::hash<decltype(BufferKey::second)> second_hasher;
    // unsigned overflow isn't UB; relying on that here
    return first_hasher(buffer_key.first) + second_hasher(buffer_key.second);
  }
};

// LLCPP synchronous server of a goldfish device-local Fuchsia sysmem Heap
// interface.
//
// Each Heap service runs on its own thread and has its own async loop.
class Heap : public HeapServer, public fbl::DoublyLinkedListable<std::unique_ptr<Heap>> {
 public:
  ~Heap() override;

  // |fidl::WireServer<fuchsia_sysmem2::Heap>|
  void AllocateVmo(AllocateVmoRequestView request,
                   AllocateVmoCompleter::Sync& completer) override = 0;

  // |fidl::WireServer<fuchsia_sysmem2::Heap>|
  void DeleteVmo(DeleteVmoRequestView request, DeleteVmoCompleter::Sync& completer) override = 0;

  // Bind the server to a FIDL channel.
  // The server should not be bound to any channel when Bind() is called.
  virtual void Bind(zx::channel server_request) = 0;

 protected:
  // This constructor is used only by its subclasses. To create a Heap
  // instance, use |Create()| static method of each subclass instead.
  explicit Heap(Control* control, const char* tag);

  // This helper method is used only by subclasses to bind to sysmem using
  // given channel and send |heap_properties| to sysmem.
  void BindWithHeapProperties(zx::channel server_request,
                              std::unique_ptr<fidl::Arena<512>> allocator,
                              fuchsia_hardware_sysmem::wire::HeapProperties heap_properties);

  Control* control() const { return control_; }

  async::Loop* loop() { return &loop_; }

 private:
  void OnClose(fidl::UnbindInfo info, zx::channel channel);

  Control* control_;

  async::Loop loop_;

  std::string tag_;
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_HEAP_H_
