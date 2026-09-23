// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <sys/wait.h>
#include <unistd.h>

#include <iostream>
#include <regex>
#include <vector>

#include <gtest/gtest.h>
#include <sanitizer/common_interface_defs.h>

namespace {

[[clang::noinline]]
void baz() {
  __sanitizer_print_stack_trace();
}

[[clang::noinline]]
void bar() {
  baz();
}

[[clang::noinline]]
void foo() {
  bar();
}

TEST(SanitizerSymbolizerMarkup, Test) {
  int pipefd[2];

  if (pipe(pipefd) == -1) {
    perror("pipe failed");
    exit(1);
  }

  pid_t pid = fork();
  if (pid == -1) {
    perror("fork failed");
    exit(1);
  }

  if (pid == 0) {
    // Child process

    // Close unused read end of the pipe
    close(pipefd[0]);

    // Redirect stdout and stderr to the write end of the pipe
    dup2(pipefd[1], STDOUT_FILENO);
    dup2(pipefd[1], STDERR_FILENO);
    close(pipefd[1]);

    foo();

    exit(0);
  }

  // Parent process

  close(pipefd[1]);

  // Create a buffer to read from the pipe
  std::vector<char> buffer(1024);

  std::string markup_stack_trace;
  ssize_t nbytes;
  while ((nbytes = read(pipefd[0], buffer.data(), buffer.size())) > 0) {
    markup_stack_trace.append(buffer.data(), nbytes);
  }

  std::regex pattern(
      R"(\{\{\{reset\}\}\}\n)"                                   // Match the initial reset
      R"(\{\{\{module:\d+:[^:]+:elf:[^:]+\}\}\}\n)"              // Match one of the modules
      R"(\{\{\{mmap:[^:]+:[^:]+:[^:]+:\d+:[^:]+:[^:]+\}\}\}\n)"  // Match at least one mmap
      R"((.*\n)*?)"                    // Omit some of the lines until we see a bt line
      R"(\{\{\{bt:\d+:[^:]+\}\}\}\n)"  // Match at least one of the bt lines
      R"((.*\n)*)"                     // Match the rest of the input
  );

  std::cout << markup_stack_trace;

  // Wait for the child process to finish
  int status;
  ASSERT_EQ(wait(&status), pid);
  ASSERT_TRUE(WIFEXITED(status));
  ASSERT_EQ(WEXITSTATUS(status), 0);
  // Close the read end of the pipe in the parent
  close(pipefd[0]);

  ASSERT_TRUE(std::regex_match(markup_stack_trace, pattern));
}
}  // namespace
