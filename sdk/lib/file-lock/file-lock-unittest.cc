// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/file-lock/file-lock.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

namespace file_lock {
namespace {

class CallbackState {
 public:
  CallbackState() : called_(false), error_(false), status_(ZX_ERR_INTERNAL) {}

  bool status_valid() const { return called_ && !error_; }

  zx_status_t status() const { return status_; }

  void Callback(zx_status_t status) {
    if (called_) {
      error_ = true;
      return;
    }
    called_ = true;
    status_ = status;
  }

  void reset() {
    called_ = false;
    status_ = false;
  }

 private:
  bool called_;
  bool error_;
  zx_status_t status_;
};

struct DummyLockClient {
  DummyLockClient(FileLock& shared_lock, zx_koid_t client_koid)
      : lock(shared_lock), koid(client_koid) {
    reset();
  }
  ~DummyLockClient() { lock.Forget(koid); }

  FileLock& lock;
  zx_koid_t koid;
  CallbackState state;
  lock_completer_t completer;

  void reset() {
    completer = lock_completer_t([this](zx_status_t status) { this->state.Callback(status); });
    state.reset();
  }

  void DoLock(LockType type, bool wait = true) {
    LockRequest req(type, wait);
    reset();
    lock.Lock(koid, req, completer);
  }
};

#define ASSERT_BLOCK(client) ASSERT_FALSE((client).state.status_valid())
#define ASSERT_RETURN(client, call_status) \
  ASSERT_TRUE((client).state.status_valid() && (call_status) == (client).state.status())

TEST(FileLock, CallbackTest) {
  FileLock lock;
  CallbackState result;
  EXPECT_FALSE(result.status_valid());

  result.Callback(ZX_ERR_SHOULD_WAIT);

  EXPECT_TRUE(result.status_valid());
  EXPECT_EQ(ZX_ERR_SHOULD_WAIT, result.status());

  result.Callback(ZX_OK);  // Callback called twice
  EXPECT_FALSE(result.status_valid());
}

TEST(FileLock, LockUnlock) {
  FileLock lock;
  DummyLockClient client1(lock, 1);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
}

TEST(FileLock, TwoReads) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);
  client2.DoLock(LockType::READ);
  ASSERT_RETURN(client2, ZX_OK);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);
}

TEST(FileLock, WriteBlocksReads) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  DummyLockClient client3(lock, 3);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);
  client2.DoLock(LockType::READ);
  ASSERT_BLOCK(client2);
  client3.DoLock(LockType::READ);
  ASSERT_BLOCK(client3);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);

  // client2 and client 3 should be unblocked
  ASSERT_RETURN(client2, ZX_OK);
  ASSERT_RETURN(client3, ZX_OK);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);
  client3.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client3, ZX_OK);
}

TEST(FileLock, ReadsBlockWrite) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  DummyLockClient client3(lock, 3);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);
  client2.DoLock(LockType::READ);
  ASSERT_RETURN(client2, ZX_OK);
  client3.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client3);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  ASSERT_BLOCK(client3);  // write lock still blocked

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  ASSERT_RETURN(client3, ZX_OK);  // write lock attained

  client3.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client3, ZX_OK);
}

TEST(FileLock, LockLeftHanging) {
  FileLock lock;
  {
    DummyLockClient client1(lock, 1);
    client1.DoLock(LockType::WRITE);
    ASSERT_RETURN(client1, ZX_OK);
  }
  {
    DummyLockClient client2(lock, 1);
    client2.DoLock(LockType::WRITE);
    ASSERT_RETURN(client2, ZX_OK);
  }
}

TEST(FileLock, ReadNoBlockWrite) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);
  client2.DoLock(LockType::WRITE, false);
  ASSERT_RETURN(client2, ZX_ERR_SHOULD_WAIT);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
}

TEST(FileLock, WriteNoBlockRead) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);
  client2.DoLock(LockType::READ, false);
  ASSERT_RETURN(client2, ZX_ERR_SHOULD_WAIT);
  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
}

TEST(FileLock, WriteToRead) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::READ);
  ASSERT_BLOCK(client2);

  client1.DoLock(LockType::READ);  // also unblock client2
  ASSERT_RETURN(client1, ZX_OK);
  ASSERT_RETURN(client2, ZX_OK);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
}

TEST(FileLock, ReadToWrite) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::READ);
  ASSERT_RETURN(client2, ZX_OK);

  client1.DoLock(LockType::WRITE, false);
  ASSERT_RETURN(client1, ZX_ERR_SHOULD_WAIT);

  client1.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client1);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);
  ASSERT_RETURN(client1, ZX_OK);  // client1 now has write lock

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
}

TEST(FileLock, OneWriteBeatsOneRead) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  DummyLockClient client3(lock, 3);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client2);

  client3.DoLock(LockType::READ);
  ASSERT_BLOCK(client3);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
  ASSERT_RETURN(client2, ZX_OK);
  ASSERT_BLOCK(client3);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);
  ASSERT_RETURN(client3, ZX_OK);

  client3.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client3, ZX_OK);
}

TEST(FileLock, TwoReadsBeatOneWrite) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  DummyLockClient client3(lock, 3);
  DummyLockClient client4(lock, 4);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client2);

  client3.DoLock(LockType::READ);
  ASSERT_BLOCK(client3);

  client4.DoLock(LockType::READ);
  ASSERT_BLOCK(client4);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client3, ZX_OK);
  ASSERT_RETURN(client4, ZX_OK);
  ASSERT_BLOCK(client2);

  client3.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client3, ZX_OK);
  ASSERT_BLOCK(client2);

  client4.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client4, ZX_OK);
  ASSERT_RETURN(client2, ZX_OK);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);
}

TEST(FileLock, UnlockWhileBlocked) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client2);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_ERR_BAD_STATE);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);
}

TEST(FileLock, ForgetPendingWriteLock) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client2);

  lock.Forget(2);
  ASSERT_RETURN(client2, ZX_ERR_CANCELED);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);

  EXPECT_TRUE(lock.NoLocksHeld());
}

TEST(FileLock, ForgetPendingReadLock) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::READ);
  ASSERT_BLOCK(client2);

  lock.Forget(2);
  ASSERT_RETURN(client2, ZX_ERR_CANCELED);

  client1.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client1, ZX_OK);

  EXPECT_TRUE(lock.NoLocksHeld());
}

TEST(FileLock, ForgetWriteLock) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);

  client1.DoLock(LockType::WRITE);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client2);

  lock.Forget(1);
  ASSERT_RETURN(client2, ZX_OK);

  client2.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client2, ZX_OK);

  EXPECT_TRUE(lock.NoLocksHeld());
}

TEST(FileLock, ForgetReadLock) {
  FileLock lock;
  DummyLockClient client1(lock, 1);
  DummyLockClient client2(lock, 2);
  DummyLockClient client3(lock, 3);

  client1.DoLock(LockType::READ);
  ASSERT_RETURN(client1, ZX_OK);

  client2.DoLock(LockType::READ);
  ASSERT_RETURN(client2, ZX_OK);

  client3.DoLock(LockType::WRITE);
  ASSERT_BLOCK(client3);

  lock.Forget(1);
  ASSERT_BLOCK(client3);

  lock.Forget(2);
  ASSERT_RETURN(client3, ZX_OK);

  client3.DoLock(LockType::UNLOCK);
  ASSERT_RETURN(client3, ZX_OK);

  EXPECT_TRUE(lock.NoLocksHeld());
}

TEST(FileLock, DestructorCancelsPending) {
  auto lock = std::make_unique<FileLock>();

  CallbackState client1_state;
  CallbackState client2_state;
  CallbackState client3_state;

  lock_completer_t client1_completer = [&client1_state](zx_status_t status) {
    client1_state.Callback(status);
  };
  lock_completer_t client2_completer = [&client2_state](zx_status_t status) {
    client2_state.Callback(status);
  };
  lock_completer_t client3_completer = [&client3_state](zx_status_t status) {
    client3_state.Callback(status);
  };

  // Client 1 gets exclusive lock
  LockRequest req_write(LockType::WRITE, true);
  lock->Lock(1, req_write, client1_completer);
  ASSERT_TRUE(client1_state.status_valid());
  ASSERT_EQ(ZX_OK, client1_state.status());

  // Client 2 blocks on write (pending exclusive)
  LockRequest req_write2(LockType::WRITE, true);
  lock->Lock(2, req_write2, client2_completer);
  ASSERT_FALSE(client2_state.status_valid());

  // Client 3 blocks on read (pending shared)
  LockRequest req_read(LockType::READ, true);
  lock->Lock(3, req_read, client3_completer);
  ASSERT_FALSE(client3_state.status_valid());

  // Destroy the lock, which should cancel pending
  lock.reset();

  ASSERT_TRUE(client2_state.status_valid());
  ASSERT_EQ(ZX_ERR_CANCELED, client2_state.status());

  ASSERT_TRUE(client3_state.status_valid());
  ASSERT_EQ(ZX_ERR_CANCELED, client3_state.status());
}

}  // namespace
}  // namespace file_lock
