// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_TESTUTIL_H_
#define SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_TESTUTIL_H_

#include <string>

#include <gmock/gmock.h>

#include "api_abstraction.h"
#include "sockscripter.h"

class TestApi : public ApiAbstraction {
 public:
  MOCK_METHOD(int, socket, (int domain, int type, int protocol), (override));

  MOCK_METHOD(int, close, (int fd), (override));

  MOCK_METHOD(int, setsockopt,
              (int fd, int level, int optname, const void* optval, socklen_t optlen), (override));

  MOCK_METHOD(int, getsockopt, (int fd, int level, int optname, void* optval, socklen_t* optlen),
              (override));

  MOCK_METHOD(int, bind, (int fd, const struct sockaddr* addr, socklen_t len), (override));

  MOCK_METHOD(int, shutdown, (int fd, int how), (override));

  MOCK_METHOD(int, connect, (int fd, const struct sockaddr* addr, socklen_t len), (override));

  MOCK_METHOD(int, accept, (int fd, struct sockaddr* addr, socklen_t* len), (override));

  MOCK_METHOD(int, listen, (int fd, int backlog), (override));

  MOCK_METHOD(ssize_t, send, (int fd, const void* buf, size_t len, int flags), (override));

  MOCK_METHOD(ssize_t, sendto,
              (int fd, const void* buf, size_t buflen, int flags, const struct sockaddr* addr,
               socklen_t addrlen),
              (override));

  MOCK_METHOD(ssize_t, recv, (int fd, void* buf, size_t len, int flags), (override));

  MOCK_METHOD(ssize_t, recvfrom,
              (int fd, void* buf, size_t buflen, int flags, struct sockaddr* addr,
               socklen_t* addrlen),
              (override));

  MOCK_METHOD(ssize_t, recvmsg, (int fd, struct msghdr* msg, int flags), (override));

  MOCK_METHOD(int, getsockname, (int fd, struct sockaddr* addr, socklen_t* len), (override));

  MOCK_METHOD(int, getpeername, (int fd, struct sockaddr* addr, socklen_t* len), (override));

  MOCK_METHOD(unsigned int, if_nametoindex, (const char* ifname), (override));

  int RunCommandLine(const std::string& line) {
    SockScripter scripter(this);

    std::unique_ptr<char[]> parsing(new char[line.length() + 1]);
    strcpy(parsing.get(), line.c_str());
    auto* p = parsing.get();
    char* start = nullptr;
    char program[] = "sockscripter";
    std::vector<char*> args;
    args.push_back(program);

    while (*p) {
      if (!start) {
        start = p;
      }
      if (*p == ' ') {
        if (strlen(start)) {
          args.push_back(start);
          start = nullptr;
        }
        *p = '\0';
      }
      p++;
    }
    if (start && strlen(start)) {
      args.push_back(start);
    }
    return scripter.Execute(static_cast<int>(args.size()), args.data());
  }
};

#endif  // SRC_CONNECTIVITY_NETWORK_TOOLS_SOCKSCRIPTER_TESTUTIL_H_
