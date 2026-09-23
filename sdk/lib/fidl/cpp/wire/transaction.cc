// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/wire/message.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/transaction.h>

namespace fidl {

CompleterBase& CompleterBase::operator=(CompleterBase&& other) noexcept {
  if (this != &other) {
    DropTransaction();
    transaction_ = other.transaction_;
    owned_ = other.owned_;
    needs_to_reply_ = other.needs_to_reply_;
    other.transaction_ = nullptr;
    other.owned_ = false;
    other.needs_to_reply_ = false;
  }
  return *this;
}

void CompleterBase::Close(zx_status_t status) {
  ScopedLock lock(lock_);
  EnsureHasTransaction(&lock);
  transaction_->Close(status);
  DropTransaction();
}

bool CompleterBase::is_reply_needed() const {
  ScopedLock lock(lock_);
  if (!needs_to_reply_)
    return false;
  if (!transaction_ || transaction_->DidOrGoingToUnbind())
    return false;
  return true;
}

void CompleterBase::EnableNextDispatch() {
  ScopedLock lock(lock_);
  EnsureHasTransaction(&lock);
  transaction_->EnableNextDispatch();
}

CompleterBase::CompleterBase(CompleterBase&& other) noexcept
    : transaction_(other.transaction_),
      reply_status_(other.reply_status_),
      owned_(other.owned_),
      needs_to_reply_(other.needs_to_reply_) {
  other.transaction_ = nullptr;
  other.reply_status_ = kReplyNotSent;
  other.owned_ = false;
  other.needs_to_reply_ = false;
}

CompleterBase::~CompleterBase() {
  if (!owned_) {
    ZX_ASSERT_MSG(!is_reply_needed(), "Completer expected a Reply to be sent.");
  } else {
    if (is_reply_needed()) {
      transaction_->InternalError(UnbindInfo::AbandonedAsyncReply(), ErrorOrigin::kReceive);
    }
  }
  DropTransaction();
}

std::unique_ptr<Transaction> CompleterBase::TakeOwnership() {
  ScopedLock lock(lock_);
  EnsureHasTransaction(&lock);
  std::unique_ptr<Transaction> clone = transaction_->TakeOwnership();
  DropTransaction();
  return clone;
}

fidl::Status CompleterBase::result_of_reply() const {
  if (reply_status_ == kReplyNotSent) {
    ZX_PANIC("Did not make a reply on this completer.");
  }
  return fidl::Status(reply_status_, fidl::Reason::kReply, "Sending reply failed");
}

void CompleterBase::SendReply(::fidl::OutgoingMessage* message,
                              internal::OutgoingTransportContext transport_context) {
  ScopedLock lock(lock_);
  EnsureHasTransaction(&lock);
  if (unlikely(!needs_to_reply_)) {
    lock.release();  // Avoid crashing on death tests.
    ZX_PANIC("Repeated or unexpected Reply.");
  }
  // At this point we are either replying or calling InternalError, so no need for
  // further replies.
  needs_to_reply_ = false;
  if (!message->ok()) {
    transaction_->InternalError(fidl::UnbindInfo{*message}, fidl::ErrorOrigin::kSend);
    reply_status_ = message->status();
    return;
  }
  fidl::WriteOptions write_options = {
      .outgoing_transport_context = std::move(transport_context),
  };
  zx_status_t status = transaction_->Reply(message, std::move(write_options));
  if (status != ZX_OK) {
    auto error = fidl::Status::TransportError(status);
    transaction_->InternalError(fidl::UnbindInfo{error}, fidl::ErrorOrigin::kSend);
    reply_status_ = status;
    return;
  }
  reply_status_ = ZX_OK;
}

void CompleterBase::EnsureHasTransaction(ScopedLock* lock) {
  if (unlikely(!transaction_)) {
    lock->release();  // Avoid crashing on death tests.
    ZX_PANIC("ToAsync() was already called.");
  }
}

void CompleterBase::DropTransaction() {
  if (owned_) {
    owned_ = false;
    delete transaction_;
  }
  transaction_ = nullptr;
  needs_to_reply_ = false;
}

}  // namespace fidl
