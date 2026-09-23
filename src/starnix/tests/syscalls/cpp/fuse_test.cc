// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <dirent.h>
#include <fcntl.h>
#include <lib/fit/defer.h>
#include <poll.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/xattr.h>
#include <unistd.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <optional>
#include <thread>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/fuse.h>
#include <linux/magic.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#ifndef FUSE_SUPER_MAGIC
#define FUSE_SUPER_MAGIC 0x65735546
#endif  // FUSE_SUPER_MAGIC

#define OK_OR_RETURN(x) \
  {                     \
    auto result = (x);  \
    if (!result) {      \
      return result;    \
    }                   \
  }

constexpr char kOverlayFsPath[] = "OVERLAYFS_PATH";

class FuseTest : public ::testing::Test {
 public:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
    }
  }

  void TearDown() override {
    if (base_dir_) {
      if (umount2(GetMountDir().c_str(), MNT_DETACH) != 0) {
        FAIL() << "Unable to umount: " << strerror(errno);
      }
      base_dir_.reset();
    }
  }

 protected:
  std::string GetOverlayFsPath() {
    if (getenv(kOverlayFsPath)) {
      return getenv(kOverlayFsPath);
    } else {
      return test_helper::GetTestResourcePath("fuse-overlayfs");
    }
  }

  std::string GetMountDir() { return *base_dir_ + "/merge"; }

  testing::AssertionResult MkDir(const std::string& directory) {
    if (mkdir(directory.c_str(), 0700) != 0) {
      return testing::AssertionFailure()
             << "Unable to create '" << directory << "': " << strerror(errno);
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult Mount() {
    if (access(GetOverlayFsPath().c_str(), R_OK | X_OK) != 0) {
      return testing::AssertionFailure()
             << "Unable to find fuse binary at: " << GetOverlayFsPath() << "(set OVERLAYFS_PATH)";
    }

    std::string base_dir = "/tmp/fuse_base_dir_XXXXXX";
    if (mkdtemp(const_cast<char*>(base_dir.c_str())) == nullptr) {
      return testing::AssertionFailure()
             << "Unable to create temporary directory: " << strerror(errno);
    }
    std::string lowerdir = base_dir + "/lower";
    OK_OR_RETURN(MkDir(lowerdir));
    std::string upperdir = base_dir + "/upper";
    OK_OR_RETURN(MkDir(upperdir));
    std::string workdir = base_dir + "/work";
    OK_OR_RETURN(MkDir(workdir));
    std::string mergedir = base_dir + "/merge";
    OK_OR_RETURN(MkDir(mergedir));
    std::string witness_name = "witness";
    {
      fbl::unique_fd witness(open((lowerdir + "/" + witness_name).c_str(), O_RDWR | O_CREAT, 0600));
      if (!witness.is_valid()) {
        return testing::AssertionFailure() << "Unable to create witness file: " << strerror(errno);
      }
      if (write(witness.get(), "hello\n", 6) != 6) {
        return testing::AssertionFailure()
               << "Unable to insert data in witness file: " << strerror(errno);
      }
    }
    pid_t child_pid = fork_helper_.RunInForkedProcess([&] {
      std::string configuration =
          "lowerdir=" + lowerdir + ",upperdir=" + upperdir + ",workdir=" + workdir;
      execl(GetOverlayFsPath().c_str(), GetOverlayFsPath().c_str(), "-f", "-o",
            configuration.c_str(), mergedir.c_str(), nullptr);
    });
    if (child_pid <= 0) {
      return testing::AssertionFailure() << "Unable to fork to start the fuse server process";
    }
    base_dir_ = base_dir;

    std::string witness = mergedir + "/" + witness_name;
    for (int i = 0; i < 100 && access(witness.c_str(), R_OK) != 0; ++i) {
      usleep(100000);
    }
    if (access(witness.c_str(), R_OK) != 0) {
      return testing::AssertionFailure() << "Unable to see witness file. Mount failed?";
    }
    return testing::AssertionSuccess();
  }

  test_helper::ForkHelper fork_helper_;
  std::optional<std::string> base_dir_;
};

class Node {
 public:
  virtual ~Node() {}

  void SetPermissions(uint32_t perms) {
    std::lock_guard guard(mtx_);
    perms_ = perms;
  }

  void SetSize(uint64_t size) {
    std::lock_guard guard(mtx_);
    size_ = size;
  }

  void SetEntryValidDuration(uint64_t secs) {
    std::lock_guard guard(mtx_);
    entry_valid_secs_ = secs;
  }

  uint64_t UpdateNodeid(uint64_t id) {
    std::lock_guard guard(mtx_);
    std::swap(id, id_);
    return id;
  }

  void IncrementGeneration() {
    std::lock_guard guard(mtx_);
    generation_++;
  }

  void PopulateAttrLocked(fuse_attr& attr) __TA_REQUIRES(mtx_) {
    attr = {
        .ino = id_,
        .size = size_,
        .mode = type_ | perms_,
    };
  }

  void PopulateAttr(fuse_attr& attr) {
    std::lock_guard guard(mtx_);
    PopulateAttrLocked(attr);
  }

  void PopulateEntry(fuse_entry_out& entry_out) {
    std::lock_guard guard(mtx_);
    entry_out = {
        .nodeid = id_,
        .generation = generation_,
        .entry_valid = entry_valid_secs_,
    };
    PopulateAttrLocked(entry_out.attr);
  }

 protected:
  Node(uint32_t type, uint64_t id) : type_(type), id_(id) {}

 private:
  uint32_t type_;

  std::mutex mtx_;
  uint64_t id_ __TA_GUARDED(mtx_) = 0;
  uint64_t generation_ __TA_GUARDED(mtx_) = 1;
  uint64_t entry_valid_secs_ __TA_GUARDED(mtx_) = 0;
  uint32_t perms_ __TA_GUARDED(mtx_) = S_IRWXU | S_IRWXG | S_IRWXO;
  uint64_t size_ __TA_GUARDED(mtx_) = 0;
};

class Directory : public Node {
 public:
  Directory(uint64_t id) : Node(S_IFDIR, id) {}

  void AddChild(const std::string& name, std::shared_ptr<Node> node) { children_[name] = node; }

  std::shared_ptr<Node> RemoveChild(const std::string& name) {
    auto iter = children_.find(name);
    if (iter == children_.end()) {
      return nullptr;
    } else {
      std::shared_ptr<Node> node = iter->second;
      children_.erase(iter);
      return node;
    }
  }

  std::shared_ptr<Node> Lookup(const std::string& name) {
    auto it = children_.find(name);
    if (it == children_.end()) {
      return nullptr;
    }
    return it->second;
  }

 private:
  std::unordered_map<std::string, std::shared_ptr<Node>> children_;
};

class File : public Node {
 public:
  File(uint64_t id) : Node(S_IFREG, id) {}
};

class FileSystem {
 public:
  FileSystem() {
    root_ = std::shared_ptr<Directory>(new Directory(FUSE_ROOT_ID));

    std::lock_guard guard(mtx_);
    nodes_[FUSE_ROOT_ID] = root_;
  }

  void UpdateNodeId(const std::shared_ptr<Node>& node) {
    std::lock_guard guard(mtx_);
    uint64_t new_nodeid = AllocateNodeIdLocked();
    nodes_[new_nodeid] = node;
    uint64_t old_nodeid = node->UpdateNodeid(new_nodeid);
    EXPECT_EQ(nodes_.erase(old_nodeid), 1u);
  }

  const std::shared_ptr<Directory>& RootDir() const { return root_; }

  template <class T, typename F>
  std::shared_ptr<T> AddNodeAt(const std::shared_ptr<Directory>& at, const std::string& name,
                               F builder) {
    std::lock_guard guard(mtx_);
    uint64_t nodeid = AllocateNodeIdLocked();
    std::shared_ptr<T> node = builder(nodeid);
    nodes_[nodeid] = node;
    at->AddChild(name, node);
    return node;
  }

  std::shared_ptr<Directory> AddDirAt(const std::shared_ptr<Directory>& at,
                                      const std::string& name) {
    return AddNodeAt<Directory>(
        at, name, [](uint64_t nodeid) { return std::make_shared<Directory>(nodeid); });
  }

  std::shared_ptr<Directory> AddDirAtRoot(const std::string& name) {
    return AddDirAt(RootDir(), name);
  }

  std::shared_ptr<File> AddFileAt(const std::shared_ptr<Directory>& at, const std::string& name) {
    return AddNodeAt<File>(at, name,
                           [](uint64_t nodeid) { return std::make_shared<File>(nodeid); });
  }

  std::shared_ptr<File> AddFileAtRoot(const std::string& name) {
    return AddFileAt(RootDir(), name);
  }

  std::shared_ptr<Node> Lookup(uint64_t nodeid) {
    std::lock_guard guard(mtx_);
    auto it = nodes_.find(nodeid);
    if (it == nodes_.end()) {
      return nullptr;
    }
    return it->second;
  }

  bool Rename(const std::shared_ptr<Directory>& source_dir, const std::string& name,
              const std::shared_ptr<Directory>& target_dir, const std::string& target_name) {
    auto maybe_node = source_dir->RemoveChild(name);
    if (!maybe_node) {
      return false;
    }
    target_dir->AddChild(target_name, maybe_node);
    return true;
  }

 private:
  uint64_t AllocateNodeIdLocked() __TA_REQUIRES(mtx_) { return next_nodeid_++; }

  std::shared_ptr<Directory> root_;

  std::mutex mtx_;
  uint64_t next_nodeid_ __TA_GUARDED(mtx_) = FUSE_ROOT_ID + 1;
  std::unordered_map<uint64_t, std::shared_ptr<Node>> nodes_ __TA_GUARDED(mtx_);
};

class FuseServer {
 public:
  FuseServer() : FuseServer(0) {}
  FuseServer(uint32_t want_init_flags) : want_init_flags_(want_init_flags) {
    int pipefds[2];
    EXPECT_EQ(pipe(pipefds), 0);
    interrupt_pipe_read_.reset(pipefds[0]);
    interrupt_pipe_write_.reset(pipefds[1]);
  }

  virtual ~FuseServer() = default;

  // Core IO function, which reads incoming FUSE messages and processes them, returning true if
  // FUSE data was processed (and therefore this function should be called again), or false if
  // there was either an exception during processing or the interrupt_pipe_ signaled that we should
  // no longer service requests.
  bool ServeOnce() {
    std::vector<std::byte> buffer;

    // Poll both the FUSE node and the interrupt pipe. The latter will be written to during Stop()
    // in order to signal that we should flush the FUSE node and exit from the serving IO process.
    struct pollfd fds[2];
    fds[0].fd = fuse_fd_.get();
    fds[0].events = POLLIN;
    fds[1].fd = interrupt_pipe_read_.get();
    fds[1].events = POLLIN;
    HANDLE_EINTR(poll(fds, 2, -1));

    bool continue_server_processing = true;
    if (fds[0].revents & POLLIN) {
      // When the poll event came from the fuse_fd_, process it normally.
      continue_server_processing = true;
      EXPECT_TRUE(ReadRequest(&buffer));
      EXPECT_TRUE(HandleFuseMessage(buffer));
    } else if (fds[0].revents & POLLERR) {
      // POLLERR on the fuse_fd_ is semi-expected when the file descriptor has been closed, for
      // instance when the kernel unmounts the backing directory. This is exercised specifically
      // in the InvalidateMountDir test case, which implicitly unmounts the backing dir.
      continue_server_processing = false;
    } else if (fds[1].revents & POLLIN) {
      // If the poll event came from interrupt_pipe_, then it's a signal to flush any remaining
      // requests from the FUSE node and complete the server loop;
      continue_server_processing = false;
      while (poll(&fds[0], 1, 0) > 0) {
        if (!ReadRequest(&buffer))
          break;
        EXPECT_TRUE(HandleFuseMessage(buffer));
      }
    } else {
      // For unexpected events, add a failure and log. This could mean, for example, that the
      // fuse_fd_ received an unexpected event which may indicate a change in behavior. It could
      // also mean that the interrupt_pipe_ closed unexpectedly, indicating a FuseServer teardown
      // that we didn't anticipate (e.g. the test didn't gracefully Stop()).
      continue_server_processing = false;
      ADD_FAILURE() << "Unexpected poll() signal. fuse_fd_ events: " << fds[0].revents
                    << ", interrupt_pipe_ events: " << fds[1].revents;
    }

    return continue_server_processing;
  }

  void Stop() {
    // Write data to the interrupt pipe. The actual data is arbitrary, and only used as a
    // signaling mechanism in the ServeOnce() polling of incoming data.
    char c = 'Q';
    ssize_t actual = HANDLE_EINTR(write(interrupt_pipe_write_.get(), &c, 1));
    EXPECT_EQ(1, actual) << "Graceful stop of the FuseServer has failed!";
  }

  FileSystem& fs() { return fs_; }
  const fbl::unique_fd& fuse_fd() { return fuse_fd_; }

  testing::AssertionResult Mount(const std::string& path) {
    OK_OR_RETURN(OpenFuseDevice());

    struct stat stat_buffer;
    if (stat(path.c_str(), &stat_buffer) == -1) {
      return testing::AssertionFailure() << "Failed to stat mount path: " << strerror(errno);
    }

    std::string options =
        fxl::StringPrintf("fd=%i,rootmode=%o,user_id=%u,group_id=%u", fuse_fd_.get(),
                          stat_buffer.st_mode & S_IFMT, getuid(), getgid());
    if (mount("fuse", path.c_str(), "fuse", 0, options.c_str()) == -1) {
      return testing::AssertionFailure() << "Failed to mount fuse device: " << strerror(errno);
    }
    return testing::AssertionSuccess();
  }

  template <typename R = void>
  R WaitForInit(std::function<R()> f = std::function<R()>()) {
    std::unique_lock guard(init_mtx_);
    init_cv_.wait(guard, [&]() { return init_done_; });
    if (f) {
      return f();
    }
    return R();
  }

  testing::AssertionResult SendInitResponse(const fuse_in_header& in_header, uint32_t flags) {
    fuse_init_out init_out = {
        .major = FUSE_KERNEL_VERSION,
        .minor = FUSE_KERNEL_MINOR_VERSION,
        .flags = flags,
    };
    return WriteStructResponse(in_header, init_out);
  }

 protected:
  testing::AssertionResult HandleFuseMessage(const std::vector<std::byte>& message) {
    if (message.size() < sizeof(fuse_in_header)) {
      return testing::AssertionFailure() << "Message size too small; got " << message.size()
                                         << ", want at least " << sizeof(fuse_in_header);
    }

    // Copy out of |message| to make sure we access an aligned |fuse_in_header|.
    struct fuse_in_header in_header;
    memcpy(&in_header, message.data(), sizeof(in_header));

    // The operation-specific payload for the fuse request begins after the header.
    const void* in_payload = message.data() + sizeof(in_header);

    std::shared_ptr<Node> node;
    // Attempt node lookup, as it is necessary for most operations. If lookup fails, we will reply
    // with ENOENT for the request. However, for init and forget operations, we can expect the node
    // to be nonexistent and therefore skip the lookup.
    if (in_header.opcode != FUSE_INIT && in_header.opcode != FUSE_FORGET &&
        in_header.opcode != FUSE_BATCH_FORGET) {
      node = fs_.Lookup(in_header.nodeid);
      if (!node) {
        return WriteDataFreeResponse(in_header, -ENOENT);
      }
    }

    switch (in_header.opcode) {
      case FUSE_INIT: {
        struct fuse_init_in init_in = {};
        memcpy(&init_in, in_payload, sizeof(init_in));
        OK_OR_RETURN(HandleInit(in_header, &init_in));
        break;
      }
      case FUSE_ACCESS: {
        struct fuse_access_in access_in = {};
        memcpy(&access_in, in_payload, sizeof(access_in));
        OK_OR_RETURN(HandleAccess(node, in_header, &access_in));
        break;
      }
      case FUSE_CREATE: {
        struct fuse_create_in create_in = {};
        memcpy(&create_in, in_payload, sizeof(create_in));
        OK_OR_RETURN(HandleCreate(node, in_header, &create_in,
                                  reinterpret_cast<const char*>(in_payload) + sizeof(create_in)));
        break;
      }
      case FUSE_GETATTR: {
        struct fuse_getattr_in getattr_in = {};
        memcpy(&getattr_in, in_payload, sizeof(getattr_in));
        OK_OR_RETURN(HandleGetAttr(node, in_header, &getattr_in));
        break;
      }
      case FUSE_LOOKUP: {
        OK_OR_RETURN(HandleLookup(node, in_header, reinterpret_cast<const char*>(in_payload)));
        break;
      }
      case FUSE_MKNOD: {
        struct fuse_mknod_in mknod_in = {};
        memcpy(&mknod_in, in_payload, sizeof(mknod_in));
        OK_OR_RETURN(HandleMknod(node, in_header, &mknod_in,
                                 reinterpret_cast<const char*>(in_payload) + sizeof(mknod_in)));
        break;
      }
      case FUSE_MKDIR: {
        struct fuse_mkdir_in mkdir_in = {};
        memcpy(&mkdir_in, in_payload, sizeof(mkdir_in));
        OK_OR_RETURN(HandleMkdir(node, in_header, &mkdir_in,
                                 reinterpret_cast<const char*>(in_payload) + sizeof(mkdir_in)));
        break;
      }
      case FUSE_OPENDIR:
      case FUSE_OPEN: {
        struct fuse_open_in open_in = {};
        memcpy(&open_in, in_payload, sizeof(open_in));
        OK_OR_RETURN(HandleOpen(node, in_header, &open_in));
        break;
      }
      case FUSE_FLUSH: {
        struct fuse_flush_in flush_in = {};
        memcpy(&flush_in, in_payload, sizeof(flush_in));
        OK_OR_RETURN(HandleFlush(node, in_header, &flush_in));
        break;
      }
      case FUSE_READ: {
        struct fuse_read_in read_in = {};
        memcpy(&read_in, in_payload, sizeof(read_in));
        OK_OR_RETURN(HandleRead(node, in_header, &read_in));
        break;
      }
      case FUSE_WRITE:
        OK_OR_RETURN(WriteDataFreeResponse(in_header, -ENOTSUP));
        break;
      case FUSE_RELEASEDIR:
      case FUSE_RELEASE: {
        struct fuse_release_in release_in = {};
        memcpy(&release_in, in_payload, sizeof(release_in));
        OK_OR_RETURN(HandleRelease(node, in_header, &release_in));
        break;
      }
      case FUSE_GETXATTR: {
        OK_OR_RETURN(WriteDataFreeResponse(in_header, -ENOSYS));
        break;
      }
      case FUSE_BATCH_FORGET:
      case FUSE_FORGET:
        // no-op; these don't expect a response.
        break;
      case FUSE_RENAME2: {
        struct fuse_rename2_in rename_in = {};
        memcpy(&rename_in, in_payload, sizeof(rename_in));
        const char* name = reinterpret_cast<const char*>(in_payload) + sizeof(rename_in);
        const char* target_name = name + strlen(name) + 1;
        OK_OR_RETURN(HandleRename(node, in_header, &rename_in, name, target_name));
        break;
      }
      default:
        return testing::AssertionFailure() << "Unknown FUSE opcode: " << in_header.opcode;
    }
    return testing::AssertionSuccess();
  }

  void NotifyInitWaiters(std::function<void()> f = std::function<void()>()) {
    std::unique_lock guard(init_mtx_);
    init_done_ = true;
    if (f) {
      f();
    }
    init_cv_.notify_all();
  }

  virtual testing::AssertionResult HandleInit(const struct fuse_in_header& in_header,
                                              const struct fuse_init_in* init_in) {
    EXPECT_EQ(init_in->flags & want_init_flags_, want_init_flags_);
    OK_OR_RETURN(SendInitResponse(in_header, want_init_flags_));
    NotifyInitWaiters();
    return testing::AssertionSuccess();
  }

  virtual testing::AssertionResult HandleAccess(const std::shared_ptr<Node>& node,
                                                const struct fuse_in_header& in_header,
                                                const struct fuse_access_in* access_in) {
    return WriteAckResponse(in_header);
  }

  virtual testing::AssertionResult HandleGetAttr(const std::shared_ptr<Node>& node,
                                                 const struct fuse_in_header& in_header,
                                                 const struct fuse_getattr_in* getattr_in) {
    fuse_attr_out attr_out = {};
    node->PopulateAttr(attr_out.attr);
    return WriteStructResponse(in_header, attr_out);
  }

  virtual testing::AssertionResult HandleLookup(const std::shared_ptr<Node>& node,
                                                const struct fuse_in_header& in_header,
                                                const char* name) {
    fuse_entry_out entry_out;
    if (HandleLookupInner(node, in_header, name, entry_out)) {
      return WriteStructResponse(in_header, entry_out);
    }
    return testing::AssertionSuccess();
  }

  virtual testing::AssertionResult HandleMknod(const std::shared_ptr<Node>& dir_node,
                                               const struct fuse_in_header& in_header,
                                               const struct fuse_mknod_in* mknod_in,
                                               const char* name) {
    const std::shared_ptr dir = std::dynamic_pointer_cast<Directory>(dir_node);
    if (!dir) {
      return WriteDataFreeResponse(in_header, -ENOTDIR);
    }

    std::shared_ptr<File> node = fs_.AddFileAt(dir, std::string(name));
    fuse_entry_out entry_out;
    node->PopulateEntry(entry_out);
    return WriteStructResponse(in_header, entry_out);
  }

  virtual testing::AssertionResult HandleCreate(const std::shared_ptr<Node>& dir_node,
                                                const struct fuse_in_header& in_header,
                                                const struct fuse_create_in* create_in,
                                                const char* name) {
    const std::shared_ptr dir = std::dynamic_pointer_cast<Directory>(dir_node);
    if (!dir) {
      return WriteDataFreeResponse(in_header, -ENOTDIR);
    }

    std::shared_ptr<File> node = fs_.AddFileAt(dir, std::string(name));

    struct response {
      fuse_entry_out entry_out;
      fuse_open_out open_out;
    } response = {};
    node->PopulateEntry(response.entry_out);
    response.open_out.fh = GetNextFileHandle();

    return WriteStructResponse(in_header, response);
  }

  virtual testing::AssertionResult HandleMkdir(const std::shared_ptr<Node>& dir_node,
                                               const struct fuse_in_header& in_header,
                                               const struct fuse_mkdir_in* mkdir_in,
                                               const char* name) {
    const std::shared_ptr dir = std::dynamic_pointer_cast<Directory>(dir_node);
    if (!dir) {
      return WriteDataFreeResponse(in_header, -ENOTDIR);
    }

    std::shared_ptr<Directory> node = fs_.AddDirAt(dir, std::string(name));
    fuse_entry_out entry_out;
    node->PopulateEntry(entry_out);
    return WriteStructResponse(in_header, entry_out);
  }

  virtual testing::AssertionResult HandleOpen(const std::shared_ptr<Node>& node,
                                              const struct fuse_in_header& in_header,
                                              const struct fuse_open_in* open_in) {
    struct fuse_open_out open_out = {};
    open_out.fh = GetNextFileHandle();
    return WriteStructResponse(in_header, open_out);
  }

  virtual testing::AssertionResult HandleFlush(const std::shared_ptr<Node>& node,
                                               const struct fuse_in_header& in_header,
                                               const struct fuse_flush_in* flush_in) {
    return WriteAckResponse(in_header);
  }

  virtual testing::AssertionResult HandleRelease(const std::shared_ptr<Node>& node,
                                                 const struct fuse_in_header& in_header,
                                                 const struct fuse_release_in* release_in) {
    return WriteAckResponse(in_header);
  }

  virtual testing::AssertionResult HandleRename(const std::shared_ptr<Node>& source_dir_node,
                                                const struct fuse_in_header& in_header,
                                                const struct fuse_rename2_in* rename_in,
                                                const char* name, const char* target_name) {
    const std::shared_ptr source_dir = std::dynamic_pointer_cast<Directory>(source_dir_node);
    if (!source_dir)
      return WriteDataFreeResponse(in_header, -ENOTDIR);

    auto node = fs_.Lookup(rename_in->newdir);
    if (!node)
      return WriteDataFreeResponse(in_header, -EINVAL);

    std::shared_ptr<Directory> target_dir = std::dynamic_pointer_cast<Directory>(node);
    if (!target_dir)
      return WriteDataFreeResponse(in_header, -ENOTDIR);

    if (!fs_.Rename(source_dir, name, target_dir, target_name))
      return WriteDataFreeResponse(in_header, -ENOENT);

    return WriteAckResponse(in_header);
  }

  virtual testing::AssertionResult HandleRmdir(const std::shared_ptr<Node>& dir_node,
                                               const struct fuse_in_header& in_header,
                                               const char* name) {
    const std::shared_ptr dir = std::dynamic_pointer_cast<Directory>(dir_node);
    if (!dir)
      return WriteDataFreeResponse(in_header, -ENOTDIR);

    if (dir->RemoveChild(name)) {
      return WriteAckResponse(in_header);
    } else {
      return WriteDataFreeResponse(in_header, -ENOENT);
    }
  }

  testing::AssertionResult WriteDataFreeResponse(const struct fuse_in_header& in_header,
                                                 int32_t error) {
    fuse_out_header out_header = {
        .len = sizeof(fuse_out_header),
        .error = error,
        .unique = in_header.unique,
    };

    auto data = reinterpret_cast<std::byte*>(&out_header);
    std::vector<std::byte> response(data, data + sizeof(out_header));
    return WriteResponse(response);
  }

  testing::AssertionResult WriteAckResponse(const struct fuse_in_header& in_header) {
    return WriteDataFreeResponse(in_header, /* error= */ 0);
  }

  template <typename Data>
  testing::AssertionResult WriteStructResponse(const struct fuse_in_header& in_header, Data data) {
    struct fuse_out_header out_header = {};
    uint32_t payload_len = sizeof(Data);
    uint32_t response_len = payload_len + sizeof(out_header);
    out_header.len = response_len;
    out_header.unique = in_header.unique;
    std::vector<std::byte> response(response_len);
    memcpy(response.data(), &out_header, sizeof(out_header));
    memcpy(response.data() + sizeof(out_header), &data, sizeof(Data));
    return WriteResponse(response);
  }

  testing::AssertionResult WriteDataResponse(const struct fuse_in_header& in_header,
                                             const void* data, size_t size) {
    struct fuse_out_header out_header = {};
    uint32_t payload_len = static_cast<uint32_t>(size);
    uint32_t response_len = payload_len + static_cast<uint32_t>(sizeof(out_header));
    out_header.len = response_len;
    out_header.unique = in_header.unique;
    std::vector<std::byte> response(response_len);
    memcpy(response.data(), &out_header, sizeof(out_header));
    memcpy(response.data() + sizeof(out_header), data, size);
    return WriteResponse(response);
  }

  testing::AssertionResult WriteResponse(std::vector<std::byte> response) {
    ssize_t actual = HANDLE_EINTR(write(fuse_fd_.get(), response.data(), response.size()));
    if (actual != static_cast<ssize_t>(response.size())) {
      return testing::AssertionFailure()
             << "Failed to write FUSE response: Got " << actual << " Expected " << response.size()
             << ": " << strerror(errno);
    }
    return testing::AssertionSuccess();
  }

  uint64_t GetNextFileHandle() { return next_fh_++; }

 protected:
  bool HandleLookupInner(const std::shared_ptr<Node>& dir_node,
                         const struct fuse_in_header& in_header, const char* name,
                         fuse_entry_out& entry_out) {
    const std::shared_ptr dir = std::dynamic_pointer_cast<Directory>(dir_node);
    if (!dir) {
      EXPECT_TRUE(WriteDataFreeResponse(in_header, -ENOENT));
      return false;
    }

    std::shared_ptr<Node> node = dir->Lookup(name);
    if (!node) {
      EXPECT_TRUE(WriteDataFreeResponse(in_header, -ENOENT));
      return false;
    }

    node->PopulateEntry(entry_out);
    return true;
  }

 private:
  testing::AssertionResult ReadRequest(std::vector<std::byte>* request) {
    // There doesn't seem to be a good value to use for the max request size. We just pick
    // something large that works for our cases.
    const size_t kMaxRequestSize = 64ul * FUSE_MIN_READ_BUFFER;
    request->resize(kMaxRequestSize);
    ssize_t actual = HANDLE_EINTR(read(fuse_fd_.get(), request->data(), request->size()));
    if (actual == -1) {
      return testing::AssertionFailure() << "Failed to read FUSE request: " << strerror(errno);
    }
    request->resize(actual);
    return testing::AssertionSuccess();
  }

  testing::AssertionResult OpenFuseDevice() {
    fuse_fd_ = fbl::unique_fd(open("/dev/fuse", O_RDWR));
    if (!fuse_fd_.is_valid()) {
      return testing::AssertionFailure() << "Failed to open /dev/fuse: " << strerror(errno);
    }
    return testing::AssertionSuccess();
  }

  fbl::unique_fd fuse_fd_;
  uint64_t next_fh_ = 1;

  fbl::unique_fd interrupt_pipe_read_;
  fbl::unique_fd interrupt_pipe_write_;

  std::mutex init_mtx_;
  std::condition_variable init_cv_;
  bool init_done_ = false;

  uint32_t want_init_flags_;

  FileSystem fs_;

  virtual testing::AssertionResult HandleRead(std::shared_ptr<Node> node,
                                              const struct fuse_in_header& in_header,
                                              const struct fuse_read_in* read_in) {
    return WriteDataFreeResponse(in_header, -ENOTSUP);
  }
};

class FuseServerTest : public ::testing::Test {
 public:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
    }
  }

  void TearDown() override {
    // If we never mounted, then there's no cleanup to be done.
    if (!mount_dir_) {
      return;
    }

    // Tear down and join the server thread, which will flush any pending requests and responses.
    server_->Stop();
    server_thread_.join();

    // Finally, we can gracefully unmount the FUSE server.
    if (umount2(mount_dir_->c_str(), MNT_DETACH) != 0) {
      FAIL() << "Unable to umount: " << strerror(errno);
    }
    mount_dir_.reset();
  }

 protected:
  std::string GetMountDir() { return *mount_dir_; }

  testing::AssertionResult Mount(std::shared_ptr<FuseServer> server) {
    std::string mount_dir = "/tmp/fuse_mount_dir_XXXXXX";
    if (mkdtemp(const_cast<char*>(mount_dir.c_str())) == nullptr) {
      return testing::AssertionFailure()
             << "Unable to create temporary directory: " << strerror(errno);
    }

    OK_OR_RETURN(server->Mount(mount_dir));
    mount_dir_ = mount_dir;
    server_ = std::move(server);

    server_thread_ = std::thread([this] {
      while (server_->ServeOnce()) {
      }
    });

    return testing::AssertionSuccess();
  }

 private:
  std::optional<std::string> mount_dir_;
  std::shared_ptr<FuseServer> server_;
  std::thread server_thread_;
};

TEST_F(FuseTest, ReadWriteUnMountedDevFuse) {
  fbl::unique_fd fuse_fd(open("/dev/fuse", O_RDWR));
  ASSERT_TRUE(fuse_fd.is_valid());
  char buffer[1024];
  ASSERT_EQ(read(fuse_fd.get(), buffer, 1024), -1);
  ASSERT_EQ(errno, EPERM);
  ASSERT_EQ(write(fuse_fd.get(), buffer, 1024), -1);
  ASSERT_EQ(errno, EPERM);
}

TEST_F(FuseTest, Mount) { ASSERT_TRUE(Mount()); }

TEST_F(FuseTest, Stats) {
  ASSERT_TRUE(Mount());
  std::string mounted_witness = GetMountDir() + "/witness";
  std::string original_witness = *base_dir_ + "/lower/witness";
  fbl::unique_fd fd(open(mounted_witness.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  struct stat mounted_stats;
  ASSERT_EQ(fstat(fd.get(), &mounted_stats), 0);
  fd = fbl::unique_fd(open(original_witness.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  struct stat original_stats;
  ASSERT_EQ(fstat(fd.get(), &original_stats), 0);
  fd.reset();

  // Check that the stat of the mounted file are the same as the origin one,
  // except for the fs id.
  ASSERT_NE(mounted_stats.st_dev, original_stats.st_dev);
  // Clobber st_dev and check the rest of the data is the same.
  mounted_stats.st_dev = 0;
  original_stats.st_dev = 0;
  ASSERT_EQ(memcmp(&mounted_stats, &original_stats, sizeof(struct stat)), 0);
}

TEST_F(FuseTest, Read) {
  ASSERT_TRUE(Mount());
  std::string mounted_witness = GetMountDir() + "/witness";
  fbl::unique_fd fd(open(mounted_witness.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  char buffer[100];
  ASSERT_EQ(read(fd.get(), buffer, 100), 6);
  ASSERT_EQ(strncmp(buffer, "hello\n", 6), 0);
}

TEST_F(FuseTest, NoFile) {
  ASSERT_TRUE(Mount());
  std::string mounted_witness = GetMountDir() + "/unexistent";
  fbl::unique_fd fd(open(mounted_witness.c_str(), O_RDONLY));
  ASSERT_FALSE(fd.is_valid());
  ASSERT_EQ(errno, ENOENT);
}

TEST_F(FuseTest, Mknod) {
  ASSERT_TRUE(Mount());
  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_WRONLY | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
}

TEST_F(FuseTest, Write) {
  ASSERT_TRUE(Mount());
  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_WRONLY | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
  EXPECT_EQ(write(fd.get(), "hello\n", 6), 6);
}

TEST_F(FuseTest, HugeWrite) {
  constexpr ssize_t kSize = 1024 * 1024;
  ASSERT_TRUE(Mount());
  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_WRONLY | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
  auto data = std::make_unique<char[]>(kSize);
  memset(data.get(), 0, kSize);
  ssize_t write_result = write(fd.get(), data.get(), kSize);
  ASSERT_GT(write_result, 0);
  ASSERT_LE(write_result, kSize);
}

TEST_F(FuseTest, WriteAppend) {
  ASSERT_TRUE(Mount());
  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_WRONLY | O_CREAT | O_APPEND));
  ASSERT_TRUE(fd.is_valid());
  EXPECT_EQ(write(fd.get(), "hello\n", 6), 6);
  fd.reset(open(filename.c_str(), O_WRONLY | O_APPEND));
  ASSERT_TRUE(fd.is_valid());
  EXPECT_EQ(write(fd.get(), "hello\n", 6), 6);
  EXPECT_EQ(write(fd.get(), "hello\n", 6), 6);

  fd.reset(open(filename.c_str(), O_RDONLY));
  char buffer[1024];
  EXPECT_EQ(read(fd.get(), buffer, 1024), 18);
  EXPECT_EQ(strncmp(buffer, "hello\nhello\nhello\n", 18), 0);
}

TEST_F(FuseTest, Statfs) {
  ASSERT_TRUE(Mount());
  struct statfs stats;
  ASSERT_EQ(statfs((GetMountDir() + "/witness").c_str(), &stats), 0);
  ASSERT_EQ(stats.f_type, FUSE_SUPER_MAGIC);
}

TEST_F(FuseTest, Seek) {
  ASSERT_TRUE(Mount());
  fbl::unique_fd fd(open((GetMountDir() + "/witness").c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  char buffer[100];
  ASSERT_EQ(read(fd.get(), buffer, 100), 6);
  ASSERT_EQ(strncmp(buffer, "hello\n", 6), 0);
  ASSERT_EQ(lseek(fd.get(), 0, SEEK_CUR), 6);
  ASSERT_EQ(lseek(fd.get(), -5, SEEK_END), 1);
  ASSERT_EQ(read(fd.get(), buffer, 100), 5);
  ASSERT_EQ(strncmp(buffer, "ello\n", 5), 0);

  ASSERT_EQ(lseek(fd.get(), 1, SEEK_DATA), 1);
  ASSERT_EQ(lseek(fd.get(), 7, SEEK_DATA), -1);
  ASSERT_EQ(errno, ENXIO);
  ASSERT_EQ(lseek(fd.get(), 0, SEEK_HOLE), 6);
  ASSERT_EQ(lseek(fd.get(), 7, SEEK_HOLE), -1);
  ASSERT_EQ(errno, ENXIO);
}

TEST_F(FuseTest, Poll) {
  ASSERT_TRUE(Mount());
  fbl::unique_fd fd(open((GetMountDir() + "/witness").c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  struct pollfd poll_struct;
  poll_struct.fd = fd.get();
  poll_struct.events = -1;
  ASSERT_EQ(poll(&poll_struct, 1, 0), 1);
  ASSERT_EQ(poll_struct.revents, POLLIN | POLLOUT | POLLRDNORM | POLLWRNORM);
}

TEST_F(FuseTest, Mkdir) {
  ASSERT_TRUE(Mount());
  std::string dirname = GetMountDir() + "/dir";
  ASSERT_EQ(open(dirname.c_str(), O_RDONLY), -1);
  ASSERT_EQ(mkdir(dirname.c_str(), 0777), 0);
  fbl::unique_fd fd(open(dirname.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  struct stat stats;
  ASSERT_EQ(fstat(fd.get(), &stats), 0);
  ASSERT_TRUE(S_ISDIR(stats.st_mode));
}

TEST_F(FuseTest, Symlink) {
  ASSERT_TRUE(Mount());
  std::string witness = GetMountDir() + "/witness";
  std::string link = GetMountDir() + "/symlink";
  ASSERT_EQ(symlink(witness.c_str(), link.c_str()), 0);
  struct stat stats;
  ASSERT_EQ(lstat(link.c_str(), &stats), 0);
  ASSERT_TRUE(S_ISLNK(stats.st_mode));
  std::vector<char> buffer;
  buffer.resize(100);
  ASSERT_EQ(readlink(link.c_str(), &buffer[0], buffer.size()),
            static_cast<ssize_t>(witness.size()));
  buffer.resize(witness.size());
  ASSERT_EQ(memcmp(&buffer[0], &witness[0], witness.size()), 0);
}

TEST_F(FuseTest, Link) {
  ASSERT_TRUE(Mount());
  std::string witness = GetMountDir() + "/witness";
  std::string linkname = GetMountDir() + "/link";
  ASSERT_EQ(link(witness.c_str(), linkname.c_str()), 0);
  struct stat stats;
  ASSERT_EQ(lstat(linkname.c_str(), &stats), 0);
  ASSERT_TRUE(S_ISREG(stats.st_mode));
  ino_t ino = stats.st_ino;
  ASSERT_EQ(lstat(witness.c_str(), &stats), 0);
  ASSERT_EQ(ino, stats.st_ino);
}

TEST_F(FuseTest, Unlink) {
  ASSERT_TRUE(Mount());
  std::string witness = GetMountDir() + "/witness";
  ASSERT_TRUE(fbl::unique_fd(open(witness.c_str(), O_RDONLY)).is_valid());
  ASSERT_EQ(unlink(witness.c_str()), 0) << strerror(errno);
  ASSERT_FALSE(fbl::unique_fd(open(witness.c_str(), O_RDONLY)).is_valid());
}

TEST_F(FuseTest, Truncate) {
  ASSERT_TRUE(Mount());
  std::string file = GetMountDir() + "/file";
  fbl::unique_fd fd(open(file.c_str(), O_WRONLY | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
  ASSERT_EQ(write(fd.get(), "hello", 5), 5);
  fd.reset();
  ASSERT_EQ(truncate(file.c_str(), 2), 0);
  fd = fbl::unique_fd(open(file.c_str(), O_RDONLY));
  char buffer[10];
  ASSERT_EQ(read(fd.get(), buffer, 10), 2);
}

TEST_F(FuseTest, Readdir) {
  // Create enough file to ensure more than one call to the fuse operation is
  // needed to read the full content of a directory. Experimentally, the libc
  // creates a buffer of 32k bytes when the user calls readdir.
  const size_t kFileCount = (32768 / (sizeof(struct fuse_dirent) + 6)) + 1;
  ASSERT_TRUE(Mount());
  std::string root = GetMountDir();
  for (size_t i = 0; i < kFileCount; ++i) {
    std::string value = std::to_string(i / 2);
    if (i % 2 == 0) {
      ASSERT_EQ(mkdir((root + "/dir_" + value).c_str(), 0777), 0);
    } else {
      fbl::unique_fd fd(open((root + "/file" + value).c_str(), O_WRONLY | O_CREAT));
      ASSERT_TRUE(fd.is_valid());
    }
  }

  std::map<std::string, struct dirent> files;
  DIR* dir = opendir(root.c_str());
  ASSERT_TRUE(dir);
  while (struct dirent* entry = readdir(dir)) {
    std::string name = entry->d_name;
    files[name] = *entry;
  }
  closedir(dir);
  ASSERT_EQ(files.size(), 2u + kFileCount);
  ASSERT_NE(files.find("witness"), files.end());
  ASSERT_NE(files.find("."), files.end());
  // fuse-overlayfs doesn't contain .. on root
  ASSERT_EQ(files.find(".."), files.end());

  files.clear();
  std::string dir1 = GetMountDir() + "/dir_0";
  dir = opendir(dir1.c_str());
  ASSERT_TRUE(dir);
  while (struct dirent* entry = readdir(dir)) {
    std::string name = entry->d_name;
    files[name] = *entry;
  }
  closedir(dir);
  ASSERT_EQ(files.size(), 2u);
  ASSERT_NE(files.find("."), files.end());
  ASSERT_NE(files.find(".."), files.end());
}

TEST_F(FuseTest, Getdents) {
  ASSERT_TRUE(Mount());
  std::string root = GetMountDir();
  fbl::unique_fd fd(open(root.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  char buffer[4096];
  ASSERT_GT(syscall(SYS_getdents64, fd.get(), buffer, 4096), 0);
}

TEST_F(FuseTest, XAttr) {
  const char attribute_name[] = "user.comment\0";
  ASSERT_TRUE(Mount());

  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_RDWR | O_CREAT));
  ASSERT_TRUE(fd.is_valid());

  ASSERT_EQ(fgetxattr(fd.get(), attribute_name, nullptr, 0), -1);
  ASSERT_EQ(errno, ENODATA);
  ASSERT_EQ(fsetxattr(fd.get(), attribute_name, "hello", 5, XATTR_CREATE), 0);
  ASSERT_EQ(fgetxattr(fd.get(), attribute_name, nullptr, 1), -1);
  ASSERT_EQ(errno, ERANGE);
  ASSERT_EQ(fgetxattr(fd.get(), attribute_name, nullptr, 0), 5);
  char buffer[5];
  ASSERT_EQ(fgetxattr(fd.get(), attribute_name, buffer, 5), 5);
  ASSERT_EQ(memcmp(buffer, "hello", 5), 0);

  ssize_t list_size = flistxattr(fd.get(), nullptr, 0);
  ASSERT_GE(list_size, 0);
  char list[list_size];
  ASSERT_EQ(flistxattr(fd.get(), list, list_size), list_size);
  ASSERT_EQ(list[list_size - 1], '\0');
  std::set<std::string> attributes;
  ssize_t index = 0;
  while (index < list_size) {
    std::string content = std::string(&list[index]);
    attributes.insert(content);
    index += content.size() + 1;
  }
  ASSERT_NE(attributes.find(attribute_name), attributes.end());

  ASSERT_EQ(fremovexattr(fd.get(), attribute_name), 0);
  ASSERT_EQ(fgetxattr(fd.get(), attribute_name, nullptr, 0), -1);
  ASSERT_EQ(errno, ENODATA);
}

TEST_F(FuseTest, Rename) {
  ASSERT_TRUE(Mount());
  std::string file = GetMountDir() + "/file";
  fbl::unique_fd fd(open(file.c_str(), O_WRONLY | O_CREAT));
  ASSERT_TRUE(fd.is_valid());

  std::string dir = GetMountDir() + "/dir";
  ASSERT_EQ(mkdir(dir.c_str(), 0777), 0);

  std::string new_path = dir + "/new_name";
  EXPECT_EQ(rename(file.c_str(), new_path.c_str()), 0);

  struct stat stat_buf;

  EXPECT_EQ(stat(file.c_str(), &stat_buf), -1);
  EXPECT_EQ(errno, ENOENT);

  EXPECT_EQ(stat(new_path.c_str(), &stat_buf), 0);
}

TEST_F(FuseTest, Rmdir) {
  // Validate removal of empty dir
  ASSERT_TRUE(Mount());
  std::string dir = GetMountDir() + "/dir";
  ASSERT_EQ(mkdir(dir.c_str(), 0777), 0);
  EXPECT_EQ(rmdir(dir.c_str()), 0) << strerror(errno);

  // Validate failure of non-empty dir
  ASSERT_EQ(mkdir(dir.c_str(), 0777), 0);
  std::string file_path = dir + "/new_name";
  fbl::unique_fd fd(open(file_path.c_str(), O_WRONLY | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
  ASSERT_NE(rmdir(dir.c_str()), 0);
}

TEST_F(FuseServerTest, NoReqsUntilInitResponse) {
  class NoReqsUntilInitResponseServer : public FuseServer {
   public:
    fuse_in_header WaitForInitAndReturnRequestHeader() {
      return WaitForInit(std::function<fuse_in_header()>([&]() { return init_hdr_; }));
    }

   protected:
    testing::AssertionResult HandleInit(const struct fuse_in_header& in_header,
                                        const struct fuse_init_in* init_in) {
      // Don't actually complete the request, just store the init request's header so
      // that we can respond to it later.
      NotifyInitWaiters([&]() { init_hdr_ = in_header; });
      return testing::AssertionSuccess();
    }

   private:
    fuse_in_header init_hdr_;
  };

  std::shared_ptr<NoReqsUntilInitResponseServer> server(new NoReqsUntilInitResponseServer());
  ASSERT_TRUE(server->fs().AddFileAtRoot("file"));
  ASSERT_TRUE(Mount(server));
  const fuse_in_header init_hdr = server->WaitForInitAndReturnRequestHeader();

  // Create a new thread to perform a request against the FUSE server.
  std::atomic_bool access_done = false;
  std::thread thrd([&]() {
    std::string filename = GetMountDir() + "/file";
    EXPECT_EQ(access(filename.c_str(), R_OK), 0) << strerror(errno);
    access_done = true;
  });
  // Make sure that the request is not completed.
  sleep(1);
  EXPECT_FALSE(access_done);

  // Send our (delayed) response to the FUSE_INIT request and make sure that the
  // access request is now completed.
  ASSERT_TRUE(server->SendInitResponse(init_hdr, 0));
  thrd.join();
  EXPECT_TRUE(access_done);
}

TEST_F(FuseServerTest, OpenAndClose) {
  std::shared_ptr<FuseServer> server(new FuseServer());
  ASSERT_TRUE(server->fs().AddFileAtRoot("file"));
  ASSERT_TRUE(Mount(server));

  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_RDWR | O_CREAT));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);
  fd.reset();
}

TEST_F(FuseServerTest, HeaderLengthUnderflow) {
  class HeaderLengthUnderflowServer : public FuseServer {
    testing::AssertionResult HandleAccess(const std::shared_ptr<Node>& node,
                                          const struct fuse_in_header& in_header,
                                          const struct fuse_access_in* access_in) override {
      uint32_t response_len = sizeof(struct fuse_out_header);
      std::vector<std::byte> response;
      response.resize(response_len);
      struct fuse_out_header* out_header =
          reinterpret_cast<struct fuse_out_header*>(response.data());
      out_header->len = 0;
      out_header->unique = in_header.unique;
      return WriteResponse(response);
    }
  };

  std::shared_ptr<HeaderLengthUnderflowServer> server(new HeaderLengthUnderflowServer());
  server->fs().AddFileAtRoot("file");
  ASSERT_TRUE(Mount(server));

  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_RDWR | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
  fd.reset();
}

TEST_F(FuseServerTest, OverlongHeaderLength) {
  class OverlongHeaderLengthServer : public FuseServer {
    testing::AssertionResult HandleOpen(const std::shared_ptr<Node>& node,
                                        const struct fuse_in_header& in_header,
                                        const struct fuse_open_in* open_in) override {
      const uint32_t kBogusHeaderLengthAddition = 1024;

      struct fuse_open_out open_out = {};
      open_out.fh = GetNextFileHandle();

      struct fuse_out_header out_header = {};
      uint32_t payload_len = sizeof(open_out);
      uint32_t response_len = payload_len + sizeof(out_header);
      std::vector<std::byte> response(response_len);
      out_header.unique = in_header.unique;

      // Test an length longer than what we wil write.
      out_header.len = response_len + kBogusHeaderLengthAddition;
      memcpy(response.data(), &out_header, sizeof(out_header));
      memcpy(response.data() + sizeof(out_header), &open_out, sizeof(open_out));
      EXPECT_FALSE(WriteResponse(response));

      // Test the right length.
      out_header.len -= kBogusHeaderLengthAddition;
      memcpy(response.data(), &out_header, sizeof(out_header));
      EXPECT_TRUE(WriteResponse(response));

      return testing::AssertionSuccess();
    }
  };

  std::shared_ptr<OverlongHeaderLengthServer> server(new OverlongHeaderLengthServer());
  server->fs().AddFileAtRoot("file");
  ASSERT_TRUE(Mount(server));

  std::string filename = GetMountDir() + "/file";
  fbl::unique_fd fd(open(filename.c_str(), O_RDWR | O_CREAT));
  ASSERT_TRUE(fd.is_valid());
  fd.reset();
}

TEST_F(FuseServerTest, RevalidateEnoent) {
  auto server = std::make_shared<FuseServer>();
  ASSERT_TRUE(Mount(server));

  std::string file = GetMountDir() + "/file";
  {
    fbl::unique_fd fd(open(file.c_str(), O_WRONLY | O_CREAT));
    ASSERT_TRUE(fd.is_valid());
  }

  server->fs().RootDir()->RemoveChild("file");

  // We removed `file` from behind Starnix's back; we should be able to recreate the file.  Starnix
  // should try and revalidate the file which will result in ENOENT, which should cause it to loop
  // around.
  {
    fbl::unique_fd fd(open(file.c_str(), O_WRONLY | O_CREAT | O_EXCL));
    ASSERT_TRUE(fd.is_valid());
  }
}

// Run the checks in a separate thread where we drop the capabilities which bypasses discretionary
// file permission checks. See capabilities(7).
template <typename F>
void InThreadWithoutCapDacOverride(F f) {
  std::thread thrd([&]() {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);

    f();
  });
  thrd.join();
}

struct BypassAccessTestCase {
  uint32_t want_init_flags;
  int access_reply;
  uint64_t max_access_count;
};

class CountingFuseServer : public FuseServer {
 public:
  CountingFuseServer(uint32_t want_init_flags, int access_reply)
      : FuseServer(want_init_flags), access_reply_(access_reply) {}

  uint64_t LookupCount() { return calls_to_lookup_.load(std::memory_order_relaxed); }
  uint64_t AccessCount() { return calls_to_access_.load(std::memory_order_relaxed); }
  uint64_t NonRootGetAttrCount() {
    return calls_to_non_root_getattr_.load(std::memory_order_relaxed);
  }

 protected:
  testing::AssertionResult HandleLookup(const std::shared_ptr<Node>& node,
                                        const struct fuse_in_header& in_header,
                                        const char* name) override {
    calls_to_lookup_.fetch_add(1, std::memory_order_relaxed);
    return FuseServer::HandleLookup(node, in_header, name);
  }

  testing::AssertionResult HandleAccess(const std::shared_ptr<Node>& node,
                                        const struct fuse_in_header& in_header,
                                        const struct fuse_access_in* access_in) override {
    calls_to_access_.fetch_add(1, std::memory_order_relaxed);
    return WriteDataFreeResponse(in_header, access_reply_);
  }

  testing::AssertionResult HandleGetAttr(const std::shared_ptr<Node>& node,
                                         const struct fuse_in_header& in_header,
                                         const struct fuse_getattr_in* getattr_in) override {
    if (in_header.nodeid != FUSE_ROOT_ID) {
      calls_to_non_root_getattr_.fetch_add(1, std::memory_order_relaxed);
    }
    return FuseServer::HandleGetAttr(node, in_header, getattr_in);
  }

 private:
  int access_reply_;
  std::atomic_uint64_t calls_to_lookup_ = 0;
  std::atomic_uint64_t calls_to_access_ = 0;
  std::atomic_uint64_t calls_to_non_root_getattr_ = 0;
};

struct PermissionCheckTestCase {
  std::optional<int> need_cap;
  uint32_t want_init_flags;
  uint32_t file_type;
  std::function<void(const std::string&)> fn;
  uint64_t expected_lookup_count;
  uint64_t expected_access_count;
  uint64_t expected_non_root_getattr_count;
};

class FuseServerPermissionCheck : public FuseServerTest,
                                  public ::testing::WithParamInterface<PermissionCheckTestCase> {};

TEST_P(FuseServerPermissionCheck, PermissionCheck) {
  const PermissionCheckTestCase& test_case = GetParam();

  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (test_case.need_cap && !test_helper::HasCapability(test_case.need_cap.value())) {
    GTEST_SKIP() << "Need extra capability " << test_case.need_cap.value();
  }

  std::shared_ptr<CountingFuseServer> server(new CountingFuseServer(test_case.want_init_flags, 0));
  switch (test_case.file_type) {
    case S_IFREG:
      ASSERT_TRUE(server->fs().AddFileAtRoot("node"));
      break;
    case S_IFDIR:
      ASSERT_TRUE(server->fs().AddDirAtRoot("node"));
      break;
    default:
      FAIL() << "Unexpected file type = " << test_case.file_type;
  }
  ASSERT_TRUE(Mount(server));
  server->WaitForInit();
  EXPECT_EQ(server->LookupCount(), 0u);
  EXPECT_EQ(server->AccessCount(), 0u);
  EXPECT_EQ(server->NonRootGetAttrCount(), 0u);

  std::string path = GetMountDir() + "/node";
  ASSERT_NO_FATAL_FAILURE(test_case.fn(path));
  // TODO(https://fxbug.dev/331965426): Don't perform an extra lookup.
  const uint64_t lookup_count_offset = test_helper::IsStarnix() ? 1 : 0;
  EXPECT_EQ(server->LookupCount(), test_case.expected_lookup_count + lookup_count_offset);
  EXPECT_EQ(server->AccessCount(), test_case.expected_access_count);
  EXPECT_EQ(server->NonRootGetAttrCount(), test_case.expected_non_root_getattr_count);
}

void TestChdir(const std::string& path) {
  test_helper::ForkHelper fork_helper;
  // Run in a forked process to not modify the state of the current
  // process which may run other tests.
  fork_helper.RunInForkedProcess([&] { ASSERT_EQ(chdir(path.c_str()), 0) << strerror(errno); });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

void TestChroot(const std::string& path) {
  test_helper::ForkHelper fork_helper;
  // Run in a forked process to not modify the state of the current
  // process which may run other tests.
  fork_helper.RunInForkedProcess([&] { ASSERT_EQ(chroot(path.c_str()), 0) << strerror(errno); });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

void TestAccess(const std::string& path) {
  ASSERT_EQ(access(path.c_str(), R_OK), 0) << strerror(errno);
}

void TestExec(const std::string& path) {
  int ret = execl(path.c_str(), path.c_str(), nullptr);
  ASSERT_EQ(ret, -1);
  // Access check passes but we don't exec because there is no memory for this file.
  EXPECT_EQ(errno, ENOEXEC);
}

void TestStat(const std::string& path) {
  struct stat s;
  ASSERT_EQ(stat(path.c_str(), &s), 0) << strerror(errno);
}

void TestOpenWithFlags(const std::string& path, int flags) {
  fbl::unique_fd fd(open(path.c_str(), flags));
  ASSERT_TRUE(fd.is_valid());
}

INSTANTIATE_TEST_SUITE_P(FuseServerPermissionCheck, FuseServerPermissionCheck,
                         testing::Values(
                             // When performing a path walk, we should only use |FUSE_LOOKUP|
                             // for _initial_ permission/access checking.
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFREG,
                                 .fn =
                                     [](const std::string& path) {
                                       ASSERT_NO_FATAL_FAILURE(TestOpenWithFlags(path, O_RDWR));
                                     },
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 0,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFDIR,
                                 .fn =
                                     [](const std::string& path) {
                                       ASSERT_NO_FATAL_FAILURE(TestOpenWithFlags(path, O_RDONLY));
                                     },
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 0,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFREG,
                                 .fn = TestStat,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFDIR,
                                 .fn = TestStat,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             // These are the same as the above, but with the `FUSE_POSIX_ACL`
                             // init flag set.
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFREG,
                                 .fn =
                                     [](const std::string& path) {
                                       ASSERT_NO_FATAL_FAILURE(TestOpenWithFlags(path, O_RDWR));
                                     },
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFDIR,
                                 .fn =
                                     [](const std::string& path) {
                                       ASSERT_NO_FATAL_FAILURE(TestOpenWithFlags(path, O_RDONLY));
                                     },
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFREG,
                                 .fn = TestStat,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFDIR,
                                 .fn = TestStat,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },

                             // Only the |access|, |chdir| and |chroot| family of
                             // syscalls may trigger |FUSE_ACCESS|.
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFREG,
                                 .fn = TestAccess,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 1,
                                 .expected_non_root_getattr_count = 0,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFREG,
                                 .fn = TestExec,
                                 .expected_lookup_count = test_helper::IsStarnix() ? 1u : 2u,
                                 // Importantly, we don't do any access checks during exec.
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = test_helper::IsStarnix() ? 1u
                                                                                             : 2u,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFDIR,
                                 .fn = TestAccess,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 1,
                                 .expected_non_root_getattr_count = 0,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .file_type = S_IFDIR,
                                 .fn = TestChdir,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 1,
                                 .expected_non_root_getattr_count = 0,
                             },
                             PermissionCheckTestCase{
                                 .need_cap = CAP_SYS_CHROOT,
                                 .want_init_flags = 0,
                                 .file_type = S_IFDIR,
                                 .fn = TestChroot,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 1,
                                 .expected_non_root_getattr_count = 0,
                             },
                             // These are the same as the above, but with the `FUSE_POSIX_ACL`
                             // init flag set.
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFREG,
                                 .fn = TestAccess,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFDIR,
                                 .fn = TestAccess,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFDIR,
                                 .fn = TestChdir,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             },
                             PermissionCheckTestCase{
                                 .need_cap = CAP_SYS_CHROOT,
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .file_type = S_IFDIR,
                                 .fn = TestChroot,
                                 .expected_lookup_count = 1,
                                 .expected_access_count = 0,
                                 .expected_non_root_getattr_count = 1,
                             }));

class FuseServerBypassAccessTest : public FuseServerTest,
                                   public ::testing::WithParamInterface<BypassAccessTestCase> {};

TEST_P(FuseServerBypassAccessTest, BypassAccess) {
  constexpr char kSomeFile1Name[] = "somefile1";
  constexpr char kSomeFile2Name[] = "somefile2";
  const BypassAccessTestCase& test_case = GetParam();

  std::shared_ptr<CountingFuseServer> server(
      new CountingFuseServer(test_case.want_init_flags, test_case.access_reply));
  FileSystem& fs = server->fs();
  ASSERT_TRUE(fs.AddFileAtRoot(kSomeFile1Name));
  ASSERT_TRUE(fs.AddFileAtRoot(kSomeFile2Name));
  ASSERT_TRUE(Mount(server));

  server->WaitForInit();
  EXPECT_EQ(server->AccessCount(), 0u);

  InThreadWithoutCapDacOverride([&]() {
    auto check_access = [&, max_access_count = test_case.max_access_count](const char* file) {
      const std::string filename = GetMountDir() + "/" + file;
      ASSERT_EQ(access(filename.c_str(), R_OK), 0) << strerror(errno);
      EXPECT_EQ(server->AccessCount(), max_access_count);
    };

    // No matter how many times we access a file, we should have only ever made
    // the |FUSE_ACCESS| request |params.max_access_count| times for the lifetime
    // of the connection/server.
    for (int i = 0; i < 3; ++i) {
      ASSERT_NO_FATAL_FAILURE(check_access(kSomeFile1Name));
    }

    // Accessing another file shouldn't change the access count since it is still
    // part of the same connection.
    ASSERT_NO_FATAL_FAILURE(check_access(kSomeFile2Name));
  });
}

INSTANTIATE_TEST_SUITE_P(
    FuseServerBypassAccessTest, FuseServerBypassAccessTest,
    testing::Values(
        // The kernel should stop sending |FUSE_ACCESS| requests once we send an
        // |ENOSYS| response.
        BypassAccessTestCase{.want_init_flags = 0, .access_reply = -ENOSYS, .max_access_count = 1},
        // The kernel should never send a |FUSE_ACCESS| request if we set the
        // |FUSE_POSIX_ACL| init flag.
        BypassAccessTestCase{
            .want_init_flags = FUSE_POSIX_ACL, .access_reply = 0, .max_access_count = 0}));

enum class ExpectedGetAttrBehaviour {
  kNone,
  kOncePerFile,
  kOncePerAccess,
};

uint64_t ExpectedGetAttrsValue(ExpectedGetAttrBehaviour behaviour, uint64_t access_count) {
  switch (behaviour) {
    case ExpectedGetAttrBehaviour::kNone:
      return 0;
    case ExpectedGetAttrBehaviour::kOncePerFile:
      return std::min(access_count, static_cast<uint64_t>(1));
    case ExpectedGetAttrBehaviour::kOncePerAccess:
      return access_count;
  }
}

struct CacheAttributesTestCase {
  uint64_t lookup_attr_timeout;
  uint64_t getattr_attr_timeout;
  ExpectedGetAttrBehaviour expected_getattr_behaviour;
};

class FuseServerCacheAttributesTest
    : public FuseServerTest,
      public ::testing::WithParamInterface<CacheAttributesTestCase> {};

TEST_P(FuseServerCacheAttributesTest, CacheAttributes) {
  constexpr char kSomeFile1Name[] = "somefile1";
  constexpr char kSomeFile2Name[] = "somefile2";
  const CacheAttributesTestCase& test_case = GetParam();

  class CacheAttributesServer : public FuseServer {
   public:
    CacheAttributesServer(uint64_t lookup_attr_valid, uint64_t getattr_attr_valid)
        : FuseServer(FUSE_POSIX_ACL),
          lookup_attr_valid_(lookup_attr_valid),
          getattr_attr_valid_(getattr_attr_valid) {}

    uint64_t NonRootGetAttrCount() {
      return calls_to_non_root_getattr_.load(std::memory_order_relaxed);
    }

   protected:
    testing::AssertionResult HandleLookup(const std::shared_ptr<Node>& node,
                                          const struct fuse_in_header& in_header,
                                          const char* name) override {
      fuse_entry_out entry_out;
      if (!HandleLookupInner(node, in_header, name, entry_out)) {
        return testing::AssertionSuccess();
      }
      // Instruct the kernel to not immediately evict the |dcache| entry (|dentry|)
      // for this node by setting a really high entry value. This value is used to
      // determine when a FUSE-based |dentry| has gone stale. This test isn't focused
      // on the |dcache| or |dentry| so this is ok. For more details, see:
      //  - https://www.halolinux.us/kernel-reference/the-dentry-cache.html
      //  - https://www.kernel.org/doc/html/latest/filesystems/path-lookup.html
      //  - https://lwn.net/Articles/649115/
      //  -
      //  https://www.infradead.org/~mchehab/kernel_docs/filesystems/path-walking.html#dcache-name-lookup
      entry_out.entry_valid = std::numeric_limits<uint64_t>::max();
      entry_out.attr_valid = lookup_attr_valid_;
      return WriteStructResponse(in_header, entry_out);
    }

    testing::AssertionResult HandleGetAttr(const std::shared_ptr<Node>& node,
                                           const struct fuse_in_header& in_header,
                                           const struct fuse_getattr_in* getattr_in) override {
      if (in_header.nodeid != FUSE_ROOT_ID) {
        calls_to_non_root_getattr_.fetch_add(1, std::memory_order_relaxed);
      }
      fuse_attr_out attr_out = {
          .attr_valid = getattr_attr_valid_,
      };
      node->PopulateAttr(attr_out.attr);
      return WriteStructResponse(in_header, attr_out);
    }

   private:
    uint64_t lookup_attr_valid_;
    uint64_t getattr_attr_valid_;
    std::atomic_uint64_t calls_to_non_root_getattr_ = 0;
  };

  std::shared_ptr<CacheAttributesServer> server(
      new CacheAttributesServer(test_case.lookup_attr_timeout, test_case.getattr_attr_timeout));
  FileSystem& fs = server->fs();
  ASSERT_TRUE(fs.AddFileAtRoot(kSomeFile1Name));
  ASSERT_TRUE(fs.AddFileAtRoot(kSomeFile2Name));
  ASSERT_TRUE(Mount(server));
  server->WaitForInit();
  EXPECT_EQ(server->NonRootGetAttrCount(), 0u);

  InThreadWithoutCapDacOverride([&]() {
    auto check_getattr = [&](const char* file, uint64_t expected_getattrs) {
      const std::string filename = GetMountDir() + "/" + file;
      ASSERT_EQ(access(filename.c_str(), R_OK), 0) << strerror(errno);
      EXPECT_EQ(server->NonRootGetAttrCount(), expected_getattrs);
    };

    uint64_t count_after_file1;
    for (uint64_t i = 1; i <= 3; ++i) {
      count_after_file1 = ExpectedGetAttrsValue(test_case.expected_getattr_behaviour, i);
      ASSERT_NO_FATAL_FAILURE(check_getattr(kSomeFile1Name, count_after_file1));
    }

    for (uint64_t i = 1; i <= 5; ++i) {
      ASSERT_NO_FATAL_FAILURE(check_getattr(
          kSomeFile2Name,
          count_after_file1 + ExpectedGetAttrsValue(test_case.expected_getattr_behaviour, i)));
    }
  });
}

INSTANTIATE_TEST_SUITE_P(
    FuseServerCacheAttributesTest, FuseServerCacheAttributesTest,
    testing::Values(
        // When we don't cache the attributes, expect the kernel to refresh the
        // attributes each call.
        CacheAttributesTestCase{
            .lookup_attr_timeout = 0,
            .getattr_attr_timeout = 0,
            .expected_getattr_behaviour = ExpectedGetAttrBehaviour::kOncePerAccess},

        // When we respond to the lookup request with a cache timeout, it should
        // be respected.
        CacheAttributesTestCase{.lookup_attr_timeout = std::numeric_limits<uint64_t>::max(),
                                .getattr_attr_timeout = 0,
                                .expected_getattr_behaviour = ExpectedGetAttrBehaviour::kNone},
        // When don't provide a cache timeout with lookup, but do for getattr,
        // respect the cached attributes after the getattr request.
        CacheAttributesTestCase{
            .lookup_attr_timeout = 0,
            .getattr_attr_timeout = std::numeric_limits<uint64_t>::max(),
            .expected_getattr_behaviour = ExpectedGetAttrBehaviour::kOncePerFile}));

struct PathWalkRefreshDirEntryTestCase {
  bool modify_nodeid;
  bool modify_generation;
  uint64_t entry_valid_forever;
  uint64_t expected_extra_lookups;
};

class FusePathWalkRefreshDirEntryTest
    : public FuseServerTest,
      public testing::WithParamInterface<PathWalkRefreshDirEntryTestCase> {};

TEST_P(FusePathWalkRefreshDirEntryTest, PathWalkRefreshDirEntry) {
  const PathWalkRefreshDirEntryTestCase& test_case = GetParam();

  std::shared_ptr<CountingFuseServer> server(new CountingFuseServer(0, 0));
  FileSystem& fs = server->fs();
  std::shared_ptr<Directory> dir1 = fs.AddDirAtRoot("dir1");
  std::shared_ptr<Directory> dir2 = fs.AddDirAt(dir1, "dir2");
  std::shared_ptr<File> file = fs.AddFileAt(dir2, "file");
  if (test_case.entry_valid_forever) {
    dir1->SetEntryValidDuration(std::numeric_limits<uint64_t>::max());
    dir2->SetEntryValidDuration(std::numeric_limits<uint64_t>::max());
    file->SetEntryValidDuration(std::numeric_limits<uint64_t>::max());
  }
  ASSERT_TRUE(Mount(server));
  EXPECT_EQ(server->LookupCount(), 0u);

  constexpr uint64_t kNumberOfNodesInPath = 3;
  // TODO(https://fxbug.dev/331965426): Don't perform an extra set of lookups each
  // time we create a new DirEntry in starnix. Note that refreshing a DirEntry
  // does not result in extra lookups, only the initial lookup to populate a new
  // DirEntry does.
  const uint64_t extra_initial_lookups_per_node =
      (!test_case.entry_valid_forever && test_helper::IsStarnix()) ? 1 : 0;
  const uint64_t extra_subsequent_lookups_per_node =
      test_case.entry_valid_forever ? 0 : kNumberOfNodesInPath;
  const uint64_t lookup_offset = extra_initial_lookups_per_node * kNumberOfNodesInPath;
  const uint64_t expected_initial_lookup_count = kNumberOfNodesInPath + lookup_offset;
  const uint64_t expected_post_update_lookup_extra_offset = extra_initial_lookups_per_node;
  std::string filename = GetMountDir() + "/dir1/dir2/file";

  auto check_open = [&]() {
    fbl::unique_fd fd(open(filename.c_str(), O_RDWR));
    ASSERT_TRUE(fd.is_valid()) << strerror(errno);
  };

  ASSERT_NO_FATAL_FAILURE(check_open());
  EXPECT_EQ(server->LookupCount(), expected_initial_lookup_count);

  ASSERT_NO_FATAL_FAILURE(check_open());
  EXPECT_EQ(server->LookupCount(),
            expected_initial_lookup_count + extra_subsequent_lookups_per_node);

  // When the kernel attempts to refresh the entry and sees a node ID or generation
  // different from what the kernel has cached for the same name, the kernel will
  // discard the cached entry and perform a fresh lookup to create a new entry
  // for the node with a different ID/generation pair but with the same name. Note
  // that different ID/generation pairs are interpreted as a completely different
  // nodes, but no two nodes may have the same ID, even if nodes have the same name
  // in the same directory.
  if (test_case.modify_nodeid) {
    fs.UpdateNodeId(dir2);
  }
  if (test_case.modify_generation) {
    dir2->IncrementGeneration();
  }
  ASSERT_NO_FATAL_FAILURE(check_open());
  EXPECT_EQ(server->LookupCount(),
            expected_initial_lookup_count + (2 * extra_subsequent_lookups_per_node) +
                test_case.expected_extra_lookups * (1 + expected_post_update_lookup_extra_offset));
}

INSTANTIATE_TEST_SUITE_P(FusePathWalkRefreshDirEntryTest, FusePathWalkRefreshDirEntryTest,
                         testing::Values(
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = false,
                                 .modify_generation = false,
                                 .entry_valid_forever = false,
                                 .expected_extra_lookups = 0,
                             },
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = false,
                                 .modify_generation = true,
                                 .entry_valid_forever = false,
                                 .expected_extra_lookups = 1,
                             },
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = true,
                                 .modify_generation = false,
                                 .entry_valid_forever = false,
                                 .expected_extra_lookups = 1,
                             },
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = true,
                                 .modify_generation = true,
                                 .entry_valid_forever = false,
                                 .expected_extra_lookups = 1,
                             },

                             // Same as above but with entryies valid forever.
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = false,
                                 .modify_generation = false,
                                 .entry_valid_forever = true,
                                 .expected_extra_lookups = 0,
                             },
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = false,
                                 .modify_generation = true,
                                 .entry_valid_forever = true,
                                 .expected_extra_lookups = 0,
                             },
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = true,
                                 .modify_generation = false,
                                 .entry_valid_forever = true,
                                 .expected_extra_lookups = 0,
                             },
                             PathWalkRefreshDirEntryTestCase{
                                 .modify_nodeid = true,
                                 .modify_generation = true,
                                 .entry_valid_forever = true,
                                 .expected_extra_lookups = 0,
                             }));

struct DirPermissionCheckTestCase {
  uint32_t want_init_flags;
  uint32_t perms;
  bool expect_open;
};

class FuseDirPermissionCheck : public FuseServerTest,
                               public ::testing::WithParamInterface<DirPermissionCheckTestCase> {};

TEST_P(FuseDirPermissionCheck, DirPermissionCheck) {
  const DirPermissionCheckTestCase& test_case = GetParam();

  std::shared_ptr<FuseServer> server(new FuseServer(test_case.want_init_flags));
  FileSystem& fs = server->fs();
  std::shared_ptr<Directory> dir = fs.AddDirAtRoot("dir");
  ASSERT_TRUE(fs.AddFileAt(dir, "node"));
  ASSERT_TRUE(Mount(server));
  server->WaitForInit();

  std::string path = GetMountDir() + "/dir/node";
  dir->SetPermissions(test_case.perms);
  InThreadWithoutCapDacOverride([&]() {
    fbl::unique_fd fd(open(path.c_str(), O_RDONLY));
    EXPECT_EQ(fd.is_valid(), test_case.expect_open);
  });
}

INSTANTIATE_TEST_SUITE_P(FuseDirPermissionCheck, FuseDirPermissionCheck,
                         testing::Values(
                             DirPermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .perms = S_IRWXU | S_IRWXG | S_IRWXO,
                                 .expect_open = true,
                             },
                             DirPermissionCheckTestCase{
                                 .want_init_flags = 0,
                                 .perms = 0,
                                 .expect_open = true,
                             },
                             DirPermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .perms = S_IRWXU | S_IRWXG | S_IRWXO,
                                 .expect_open = true,
                             },
                             DirPermissionCheckTestCase{
                                 .want_init_flags = FUSE_POSIX_ACL,
                                 .perms = 0,
                                 .expect_open = false,
                             }));

TEST_F(FuseServerTest, ExecAccessDenied) {
  std::shared_ptr<FuseServer> server(new FuseServer(0));
  FileSystem& fs = server->fs();
  std::shared_ptr<Directory> dir = fs.AddDirAtRoot("dir");
  auto file = fs.AddFileAt(dir, "node");
  ASSERT_TRUE(file);
  file->SetPermissions(S_IRUSR | S_IWUSR | S_IRGRP | S_IWGRP | S_IROTH | S_IWOTH);
  ASSERT_TRUE(Mount(server));
  server->WaitForInit();

  std::string path = GetMountDir() + "/dir/node";
  InThreadWithoutCapDacOverride([&]() {
    int ret = execl(path.c_str(), path.c_str(), nullptr);
    EXPECT_EQ(ret, -1);
    EXPECT_EQ(errno, EACCES);
  });
}

struct MkPermissionCheckTestCase {
  uint32_t want_init_flags;
  uint32_t perms;
  int expected_errno;
};

class FuseMkPermissionCheck
    : public FuseServerTest,
      public ::testing::WithParamInterface<
          std::tuple<std::function<int(const std::string&)>, MkPermissionCheckTestCase>> {};

TEST_P(FuseMkPermissionCheck, MkPermissionCheck) {
  const std::tuple<std::function<int(const std::string&)>, MkPermissionCheckTestCase>& param =
      GetParam();
  const std::function<int(const std::string&)>& test_fn = std::get<0>(param);
  const MkPermissionCheckTestCase& test_case = std::get<1>(param);

  std::shared_ptr<FuseServer> server(new FuseServer(test_case.want_init_flags));
  FileSystem& fs = server->fs();
  std::shared_ptr<Directory> dir = fs.AddDirAtRoot("dir");
  ASSERT_TRUE(Mount(server));
  server->WaitForInit();

  std::string path = GetMountDir() + "/dir/node";
  dir->SetPermissions(test_case.perms);
  InThreadWithoutCapDacOverride([&]() {
    int ret = test_fn(path);
    if (test_case.expected_errno == 0) {
      EXPECT_EQ(ret, 0) << strerror(errno);
    } else {
      ASSERT_EQ(ret, -1);
      EXPECT_EQ(errno, test_case.expected_errno);
    }
  });
}

INSTANTIATE_TEST_SUITE_P(
    FuseMkPermissionCheck, FuseMkPermissionCheck,
    testing::Combine(
        testing::Values([](const std::string& path) { return mknod(path.c_str(), 0, 0); },
                        [](const std::string& path) { return mkdir(path.c_str(), 0); }),
        testing::Values(
            MkPermissionCheckTestCase{
                .want_init_flags = 0,
                .perms = S_IRWXU | S_IRWXG | S_IRWXO,
                .expected_errno = 0,
            },
            MkPermissionCheckTestCase{
                .want_init_flags = 0,
                .perms = 0,
                .expected_errno = 0,
            },
            MkPermissionCheckTestCase{
                .want_init_flags = FUSE_POSIX_ACL,
                .perms = S_IRWXU | S_IRWXG | S_IRWXO,
                .expected_errno = 0,
            },
            MkPermissionCheckTestCase{
                .want_init_flags = FUSE_POSIX_ACL,
                .perms = 0,
                .expected_errno = EACCES,
            })));

TEST_F(FuseServerTest, InvalidateMountDir) {
  std::shared_ptr<FuseServer> parent_server(new FuseServer());
  std::shared_ptr<Directory> mount_dir = parent_server->fs().AddDirAtRoot("mount_dir");
  ASSERT_TRUE(mount_dir);
  ASSERT_TRUE(Mount(parent_server));

  std::shared_ptr<FuseServer> child_server(new FuseServer());
  ASSERT_TRUE(child_server->fs().AddFileAtRoot("node"));

  const std::string child_mount_dir = GetMountDir() + "/mount_dir";
  ASSERT_TRUE(child_server->Mount(child_mount_dir));
  std::thread child_mount_thread([&] {
    while (child_server->ServeOnce()) {
    }
  });
  auto cleanup_child_mount = fit::defer([&]() {
    child_server->Stop();
    child_mount_thread.join();
    umount2(child_mount_dir.c_str(), MNT_DETACH);
  });
  const std::string node_path = child_mount_dir + "/node";
  ASSERT_NO_FATAL_FAILURE(TestOpenWithFlags(node_path, O_RDONLY));

  // The following open operation will trigger a revalidation on the child
  // mount directory which will fail because we update the generation. This
  // revalidation failure should trigger the implicit unmounting of the child
  // FUSE server.
  mount_dir->IncrementGeneration();
  ASSERT_EQ(open(node_path.c_str(), O_RDONLY), -1);
  EXPECT_EQ(errno, ENOENT);

  ASSERT_EQ(umount2(child_mount_dir.c_str(), MNT_DETACH), -1);
  EXPECT_EQ(errno, EINVAL);
}

TEST_F(FuseServerTest, ReadDir) {
  std::shared_ptr<FuseServer> parent_server(new FuseServer());
  std::shared_ptr<Directory> mount_dir = parent_server->fs().AddDirAtRoot("dir");
  ASSERT_TRUE(mount_dir);
  ASSERT_TRUE(Mount(parent_server));
  const std::string dir_path = GetMountDir() + "/dir";

  fbl::unique_fd fd(open(dir_path.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  char buf[4096];
  ASSERT_EQ(read(fd.get(), buf, 4096), -1);
  ASSERT_EQ(errno, EISDIR);
}

// Custom FUSE server mock that disables PASSTHROUGH and handles standard FUSE_READ.
class NonPassthroughReadServer : public FuseServer {
 public:
  NonPassthroughReadServer() : FuseServer() {}

  testing::AssertionResult HandleRead(std::shared_ptr<Node> node,
                                      const struct fuse_in_header& in_header,
                                      const struct fuse_read_in* read_in) override {
    // Respond with mock data filled with 'L' (matching our bounds check test)
    std::string response_data(read_in->size, 'L');
    return WriteDataResponse(in_header, response_data.data(), response_data.size());
  }
};

// Test Case: Verify standard non-passthrough FUSE read caps target sizes at the EOF boundary.
TEST_F(FuseServerTest, StandardReadEOFCapping) {
  auto server = std::make_shared<NonPassthroughReadServer>();
  ASSERT_TRUE(Mount(server));

  // 1. Create a virtual regular file inside the mock VFS using native FileSystem methods.
  std::shared_ptr<File> file = server->fs().AddFileAtRoot("witness");
  ASSERT_TRUE(file);
  file->SetSize(8589934592ULL);  // Set file size to 8 GB (Matches CTS largeFile)
  file->SetPermissions(0600);

  std::string file_path = GetMountDir() + "/witness";
  fbl::unique_fd fd(open(file_path.c_str(), O_RDONLY));
  ASSERT_TRUE(fd.is_valid());

  // 2. positional read (pread64) 128 bytes at: 8GB - 64 bytes
  char buffer[128];
  ssize_t read_bytes = pread64(fd.get(), buffer, 128, 8589934528ULL);  // 8GB - 64

  // 3. Expected Capping: Starnix must detect EOF boundary, cap read request size to 64 bytes
  //    BEFORE sending to userspace, and return only 64 bytes!
  ASSERT_EQ(read_bytes, 64) << strerror(errno);

  for (int i = 0; i < 64; i++) {
    EXPECT_EQ(buffer[i], 'L');
  }
}
