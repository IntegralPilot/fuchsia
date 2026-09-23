// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_TEST_PROCESSARGS_H_
#define LIB_LD_TESTING_TEST_PROCESSARGS_H_

#include <lib/zx/channel.h>
#include <lib/zx/handle.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <concepts>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <ranges>
#include <string>
#include <vector>

#include <fbl/unique_fd.h>

namespace ld::testing {

// This is a "builder"-style object that collects information to be packed into
// the processargs message(s).
class TestProcessArgs {
 public:
  TestProcessArgs& AddHandle(uint32_t info, zx::handle handle);

  // This duplicates the handle.
  TestProcessArgs& AddDuplicateHandle(uint32_t info, zx::unowned_handle ref);

  // This is just a shorthand for using PA_HND(PA_FD, fd).
  TestProcessArgs& AddFd(int fd, zx::handle handle);

  // These work with any zx::foo type.
  template <typename T>
  TestProcessArgs& AddHandle(uint32_t info, T&& handle) {
    return AddHandle(info, zx::handle{handle.release()});
  }

  template <typename T>
  TestProcessArgs& AddDuplicateHandle(uint32_t info, zx::unowned<T> ref) {
    return AddDuplicateHandle(info, zx::unowned_handle{ref->get()});
  }

  template <typename T>
  TestProcessArgs& AddFd(int fd, T&& handle) {
    return AddFd(fd, zx::handle{handle.release()});
  }

  TestProcessArgs& AddFd(int test_fd, fbl::unique_fd local_fd);

  // This doesn't consume the fd, but immediately clones it.  It's about
  // equivalent to `AddFd(test_fd, fbl::unique_fd{dup(local_fd)})`.
  TestProcessArgs& AddClonedFd(int test_fd, int local_fd);

  // This returns the index of the entry the next AddName call will append.
  size_t next_name() const { return names_.size(); }

  // Append one name to the name table.
  TestProcessArgs& AddName(std::string_view name) {
    names_.emplace_back(name);
    return *this;
  }

  // This adds a name table entry along with a handle of the given PA_NS_* type
  // using the new entry's index.
  TestProcessArgs& AddName(std::string_view name, uint32_t info, zx::channel handle);

  // This replaces the entire name table.
  TestProcessArgs& SetNames(std::ranges::input_range auto names)
    requires std::constructible_from<std::vector<std::string>,  //
                                     decltype(names.begin()), decltype(names.end())>
  {
    names_ = std::vector<std::string>{names.begin(), names.end()};
    return *this;
  }

  TestProcessArgs& SetArgs(std::ranges::input_range auto args)
    requires std::constructible_from<std::vector<std::string>,  //
                                     decltype(args.begin()), decltype(args.end())>
  {
    args_ = std::vector<std::string>{args.begin(), args.end()};
    return *this;
  }

  // If the argument would be given to SetArgs for the main (libc) message, then
  // this yields an argument to SetArgs for the dynamic linker's message.
  static constexpr auto InterpArgs(std::ranges::input_range auto args)
    requires std::constructible_from<std::vector<std::string>,  //
                                     decltype(args.begin()), decltype(args.end())>
  {
    // The startup dynamic linker can use argv[0] in messages.
    return std::views::take(args, 1);
  }

  TestProcessArgs& SetEnv(std::ranges::input_range auto env)
    requires std::constructible_from<std::vector<std::string>,  //
                                     decltype(env.begin()), decltype(env.end())>
  {
    env_ = std::vector<std::string>{env.begin(), env.end()};
    return *this;
  }

  // If the argument would be given to SetEnv for the main (libc) message, then
  // this yields an argument to SetEnv for the dynamic linker's message.
  static constexpr auto InterpEnv(std::ranges::input_range auto env)
    requires std::constructible_from<std::vector<std::string>,  //
                                     decltype(env.begin()), decltype(env.end())>
  {
    using namespace std::literals;
    constexpr auto interp_cares = [](std::string_view str) {
      return str.starts_with("LD_DEBUG="sv);
    };
    return std::views::filter(env, interp_cares);
  }

  // This calls AddProcess and AddThread.
  TestProcessArgs& AddInProcessTestHandles();

  // These add the handles for the process and initial thread.
  TestProcessArgs& AddProcess(zx::unowned_process process);
  TestProcessArgs& AddThread(zx::unowned_thread thread);

  // Add a fuchsia.ldsvc.Loader channel handle.
  TestProcessArgs& AddLdsvc(zx::channel ldsvc);

  // Add the VMAR to use for allocation and loading.
  TestProcessArgs& AddAllocationVmar(zx::vmar vmar);
  TestProcessArgs& AddAllocationVmar(zx::unowned_vmar vmar);

  // Add the VMAR for the dynamic linker's load image.
  TestProcessArgs& AddSelfVmar(zx::vmar vmar);
  TestProcessArgs& AddSelfVmar(zx::unowned_vmar vmar);

  // Add the VMO for the executable.
  TestProcessArgs& AddExecutableVmo(zx::vmo vmo);

  // Add the VMO for the initial stack.
  TestProcessArgs& AddStackVmo(zx::vmo vmo);

  // This packs up a message and sends it on the given channel.  When this
  // returns without raising a gtest fatal failure, the state of the object is
  // cleared out so it it could be used for another call.
  void PackBootstrap(zx::unowned_channel bootstrap_sender);

  // This creates a new channel and returns the receiver end to be passed to
  // the new process.  Before returning, it sends a bootstrap message on the
  // sender side of the channel and clears the builder state.  The sender side
  // of the channel is retained in this object.  If this is called again when a
  // channel already exists, it sends a second message on that same channel; in
  // that case it always returns a null handle.  If anything went wrong, this
  // raises a gtest fatal failure.
  zx::channel PackBootstrap();

  // Access the sender channel if one was created by MakeBootstrap() or
  // PackBootstrap().  Once no more PackBootstrap() calls will be made, this
  // can be moved-from or reset to close or take ownership of the channel end.
  zx::channel& bootstrap_sender() { return bootstrap_sender_; }

  // This returns the stack size to use for the initial thread,
  // if the bootstrap protocol wants to control that.
  std::optional<size_t> GetStackSize();

  // This just creates a new channel and installs the other end as
  // bootstrap_sender(), without packing anything into it.
  zx::channel MakeBootstrap();

  // This returns true if the object has no pending data or handles added since
  // construction or the last PackBootstrap() call.
  bool empty() const;

 private:
  void PackBootstrap(zx::unowned_channel bootstrap_sender, size_t* count_for_stack_size);

  std::vector<zx::handle> handles_;
  std::vector<uint32_t> handle_info_;
  std::vector<std::string> args_, env_, names_;
  zx::channel bootstrap_sender_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TESTING_TEST_PROCESSARGS_H_
