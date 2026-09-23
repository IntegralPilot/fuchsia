// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_server/fake_server.h"

#include <span>

namespace block_server {

/// Implementation of Interface backed by a VMO.
class FakeServer::FakeInterface : public Interface {
 public:
  FakeInterface(const FakeServer& server, zx::vmo data, uint64_t block_size)
      : server_(server), data_(std::move(data)), block_size_(block_size) {
    uint64_t size;
    ZX_ASSERT(data_.get_size(&size) == ZX_OK);
    ZX_ASSERT(size % block_size_ == 0);
  }
  FakeInterface(const FakeServer& server, uint64_t blocks, uint64_t block_size)
      : server_(server), block_size_(block_size) {
    ZX_ASSERT(zx::vmo::create(blocks * block_size, 0, &data_) == ZX_OK);
  }

  FakeInterface(const FakeInterface&) = delete;
  FakeInterface& operator=(const FakeInterface&) = delete;

  void StartThread(std::unique_ptr<Thread> thread) override {
    std::thread([thread = std::move(thread)]() mutable { thread->Run(); }).detach();
  }
  void OnNewSession(std::unique_ptr<Session> session) override {
    std::thread([session = std::move(session)]() mutable { session->Run(); }).detach();
  }
  void OnRequests(std::span<Request> requests) override {
    std::vector<uint8_t> buf;
    size_t len;
    for (const Request& request : requests) {
      switch (request.operation.tag) {
        case Operation::Tag::Read:
          len = request.operation.read.block_count * block_size_;
          buf.resize(len);
          ZX_ASSERT(data_.read(buf.data(), request.operation.read.device_block_offset * block_size_,
                               len) == ZX_OK);
          ZX_ASSERT(request.vmo->write(buf.data(), request.operation.read.vmo_offset, len) ==
                    ZX_OK);
          break;

        case Operation::Tag::Write:
          len = request.operation.write.block_count * block_size_;
          buf.resize(len);
          ZX_ASSERT(request.vmo->read(buf.data(), request.operation.write.vmo_offset, len) ==
                    ZX_OK);
          ZX_ASSERT(data_.write(buf.data(),
                                request.operation.write.device_block_offset * block_size_,
                                len) == ZX_OK);
          break;

        case Operation::Tag::Flush:
        case Operation::Tag::Trim:
          break;
        case Operation::Tag::CloseVmo:
        case Operation::Tag::StartDecompressedRead:
        case Operation::Tag::ContinueDecompressedRead:
          __UNREACHABLE;
      }
      const BlockServer* server = server_.server_.get();
      server->SendReply(request.request_id, zx::ok());
    }
  }

  const zx::vmo& vmo() const { return data_; }

 private:
  const FakeServer& server_;
  zx::vmo data_;
  uint64_t block_size_;
};

FakeServer::FakeServer(const PartitionInfo& info, zx::vmo data) {
  uint64_t size;
  interface_ = data ? std::make_unique<FakeInterface>(*this, std::move(data),
                                                      static_cast<uint64_t>(info.block_size))
                    : std::make_unique<FakeInterface>(*this, info.block_count,
                                                      static_cast<uint64_t>(info.block_size));
  ZX_ASSERT(interface_->vmo().get_size(&size) == ZX_OK);
  server_ = std::make_unique<BlockServer>(info, interface_.get());
  ZX_ASSERT(info.block_size * info.block_count == size);
}

FakeServer::~FakeServer() = default;
FakeServer::FakeServer(FakeServer&&) = default;
FakeServer& FakeServer::operator=(FakeServer&&) = default;

}  // namespace block_server
