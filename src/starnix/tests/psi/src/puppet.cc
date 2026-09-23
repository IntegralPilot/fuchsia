// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <err.h>
#include <fcntl.h>
#include <poll.h>
#include <stdlib.h>
#include <sys/epoll.h>
#include <sys/resource.h>
#include <unistd.h>

#include <iostream>
#include <string>
#include <vector>

#include "src/lib/files/file.h"
#include "src/lib/files/file_descriptor.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/string_printf.h"

// Control socket between the puppet program running within Starnix (that is, us) and its Rust
// controller (puppet.rs).
//
// The protocol consists of semicolon-terminated messages, each consisting of
// hex-encoded parts separated by commas. For instance:
//  "48454c4c4f,574f524c44;" <=> [ "HELLO", "WORLD" ]
class ControlSocket {
 public:
  explicit ControlSocket(int fd) {
    socket_ = fdopen(fd, "r+b");
    if (socket_ == nullptr)
      err(EXIT_FAILURE, "Failed to wrap socket in a FILE*");
  }

  ~ControlSocket() { fclose(socket_); }

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(ControlSocket);

  std::vector<std::string> ReadMessage() {
    std::vector<std::string> decoded_parts;
    std::string current_part;
    bool more_parts_ahead = true;
    while (more_parts_ahead) {
      int val;
      // Try to read a two-characters hexadecimal number.
      if (fscanf(socket_, "%2x", &val) == 1) {
        current_part.push_back(static_cast<char>(val));
      } else {
        int c = fgetc(socket_);
        if (c == ';') {  // End of message?
          more_parts_ahead = false;
        } else if (c != ',') {  // Or simply the end of the current part?
          errx(EXIT_FAILURE, "Unexpected character (%d) in control socket stream", c);
        }

        // Regardless of whether we got a semicolon or a comma, we have reached the end of the
        // current part. Add it to the results and start a new one.
        std::string tmp;
        current_part.swap(tmp);
        decoded_parts.push_back(std::move(tmp));
      }
    }
    return decoded_parts;
  }

  void WriteMessage(const std::vector<std::string>& parts) {
    std::vector<std::string> encoded_parts;
    for (size_t i = 0; i < parts.size(); ++i) {
      if (i != 0) {
        fputc(',', socket_);
      }
      for (char c : parts[i]) {
        fprintf(socket_, "%02x", c);
      }
    }
    fputc(';', socket_);
    fflush(socket_);
  }

 private:
  FILE* socket_;
};

int main(int argc, const char** argv) {
  std::cout << "starting starnix puppet...\n";
  ControlSocket ctl_socket(3);

  ctl_socket.WriteMessage({"READY"});

  while (true) {
    std::vector<std::string> command = ctl_socket.ReadMessage();
    if (command.empty()) {
      break;
    }

    std::cout << "executing command:" << fxl::JoinStrings(command, " ") << "\n";

    if (command[0] == "CHECK_EXISTS" && command.size() == 2) {
      int r = access(command[1].c_str(), F_OK);
      ctl_socket.WriteMessage({r == 0 ? "YES" : "NO"});
    } else if (command[0] == "OPEN" && command.size() == 2) {
      int fd = open(command[1].c_str(), O_RDWR);
      ctl_socket.WriteMessage({fxl::StringPrintf("%d", fd)});
    } else if (command[0] == "CLOSE" && command.size() == 2) {
      int fd = fxl::StringToNumber<int>(command[1]);
      close(fd);
    } else if (command[0] == "READ_TO_END" && command.size() == 2) {
      int fd = fxl::StringToNumber<int>(command[1]);
      std::string buf;
      files::ReadFileDescriptorToString(fd, &buf);
      ctl_socket.WriteMessage({std::move(buf)});
    } else if (command[0] == "WRITE_ALL" && command.size() == 3) {
      int fd = fxl::StringToNumber<int>(command[1]);
      fxl::WriteFileDescriptor(fd, command[2].data(), static_cast<ssize_t>(command[2].length()));
      ctl_socket.WriteMessage({});
    } else if (command[0] == "POLL_POLLPRI" && command.size() == 3) {
      int fd = fxl::StringToNumber<int>(command[1]);
      int timeout_ms = fxl::StringToNumber<int>(command[2]);
      pollfd pfd{.fd = fd, .events = POLLPRI, .revents = 0};
      switch (poll(&pfd, 1, timeout_ms)) {
        case 0:
          ctl_socket.WriteMessage({"TIMEOUT"});
          break;
        case 1:
          if (!(pfd.revents & POLLPRI)) {
            errx(EXIT_FAILURE, "poll returned no POLLPRI event");
          }
          ctl_socket.WriteMessage({"EVENT"});
          break;
        default:
          err(EXIT_FAILURE, "poll failed");
          break;
      }
    } else if (command[0] == "EPOLL_CREATE_AND_ADD_EPOLLPRI" && command.size() == 2) {
      int fd = fxl::StringToNumber<int>(command[1]);

      int epollfd = epoll_create1(0);
      if (epollfd < 0) {
        err(EXIT_FAILURE, "epoll_create1 failed");
      }

      epoll_event event;
      event.events = EPOLLPRI;
      event.data.u32 = 1234;
      if (epoll_ctl(epollfd, EPOLL_CTL_ADD, fd, &event) != 0) {
        err(EXIT_FAILURE, "epoll_ctl(EPOLL_CTL_ADD) failed");
      }
      ctl_socket.WriteMessage({fxl::StringPrintf("%d", epollfd)});
    } else if (command[0] == "EPOLL_WAIT_EPOLLPRI" && command.size() == 3) {
      int epollfd = fxl::StringToNumber<int>(command[1]);
      int timeout_ms = fxl::StringToNumber<int>(command[2]);
      epoll_event event;
      switch (epoll_wait(epollfd, &event, 1, timeout_ms)) {
        case 0:
          ctl_socket.WriteMessage({"TIMEOUT"});
          break;
        case 1:
          if (!(event.events & EPOLLPRI)) {
            errx(EXIT_FAILURE, "epoll returned no EPOLLPRI event");
          }
          ctl_socket.WriteMessage({"EVENT"});
          break;
        default:
          err(EXIT_FAILURE, "epoll_wait failed");
          break;
      }
    } else if (command[0] == "EXIT" && command.size() == 1) {
      break;
    } else {
      errx(EXIT_FAILURE, "Unrecognized command");
    }
  }

  std::cout << "stopping starnix puppet...\n";
  return 0;
}
