// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
#include <map>
#include <string>
#include <vector>

#include <linux/fuse.h>

#define FUSE_DIRENT_ALIGN(x) (((x) + sizeof(uint64_t) - 1) & ~(sizeof(uint64_t) - 1))

namespace {

// The FUSE implementation will write to this fd to signal readiness.
constexpr int kFuseReadyFd = 3;

void StatToFuseAttr(const struct stat* st, struct fuse_attr* attr) {
  attr->ino = st->st_ino;
  attr->mode = st->st_mode;
  attr->nlink = static_cast<uint32_t>(st->st_nlink);
  attr->uid = st->st_uid;
  attr->gid = st->st_gid;
  attr->rdev = static_cast<uint32_t>(st->st_rdev);
  attr->size = st->st_size;
  attr->blksize = static_cast<uint32_t>(st->st_blksize);
  attr->blocks = st->st_blocks;
  attr->atime = st->st_atime;
  attr->mtime = st->st_mtime;
  attr->ctime = st->st_ctime;
}

struct FileNode {
  struct stat attr = {};
  std::string content;
};

std::map<std::string, FileNode> g_filesystem;

void HandleLookup(int fd, fuse_in_header* in_header) {
  fuse_out_header out_header;
  out_header.unique = in_header->unique;

  char* filename = reinterpret_cast<char*>(in_header + 1);
  std::string path(filename);

  auto it = g_filesystem.find(path);
  if (it == g_filesystem.end()) {
    out_header.len = sizeof(fuse_out_header);
    out_header.error = -ENOENT;
    write(fd, &out_header, sizeof(out_header));
    return;
  }

  fuse_entry_out out_payload;
  memset(&out_payload, 0, sizeof(out_payload));
  StatToFuseAttr(&it->second.attr, &out_payload.attr);
  out_payload.nodeid = it->second.attr.st_ino;
  out_payload.generation = 0;
  out_payload.entry_valid = 10;
  out_payload.attr_valid = 10;
  StatToFuseAttr(&it->second.attr, &out_payload.attr);

  out_header.len = sizeof(fuse_out_header) + sizeof(fuse_entry_out);
  out_header.error = 0;

  std::vector<char> response(out_header.len);
  memcpy(response.data(), &out_header, sizeof(out_header));
  memcpy(response.data() + sizeof(out_header), &out_payload, sizeof(out_payload));

  write(fd, response.data(), response.size());
}

void HandleGetattr(int fd, fuse_in_header* in_header) {
  fuse_out_header out_header;
  out_header.unique = in_header->unique;

  const FileNode* node = nullptr;
  for (auto const& [key, val] : g_filesystem) {
    if (val.attr.st_ino == in_header->nodeid) {
      node = &val;
      break;
    }
  }

  if (!node) {
    out_header.len = sizeof(fuse_out_header);
    out_header.error = -ENOENT;
    write(fd, &out_header, sizeof(out_header));
    return;
  }

  fuse_attr_out out_payload;
  memset(&out_payload, 0, sizeof(out_payload));
  out_payload.attr_valid = 10;
  StatToFuseAttr(&node->attr, &out_payload.attr);

  out_header.len = sizeof(fuse_out_header) + sizeof(fuse_attr_out);
  out_header.error = 0;

  std::vector<char> response(out_header.len);
  memcpy(response.data(), &out_header, sizeof(out_header));
  memcpy(response.data() + sizeof(out_header), &out_payload, sizeof(out_payload));

  write(fd, response.data(), response.size());
}

void HandleMknod(int fd, fuse_in_header* in_header) {
  fuse_out_header out_header = {};
  out_header.unique = in_header->unique;

  fuse_mknod_in* in_payload = reinterpret_cast<fuse_mknod_in*>(in_header + 1);
  char* filename = reinterpret_cast<char*>(in_payload + 1);
  std::string path(filename);

  if (g_filesystem.contains(path)) {
    out_header.len = sizeof(fuse_out_header);
    out_header.error = -EEXIST;
    write(fd, &out_header, sizeof(out_header));
    return;
  }

  FileNode new_node;
  new_node.attr.st_ino = g_filesystem.size() + 1;  // 0 is not valid.
  new_node.attr.st_mode = in_payload->mode;
  new_node.attr.st_nlink = 1;
  g_filesystem[path] = new_node;

  fuse_entry_out out_payload;
  memset(&out_payload, 0, sizeof(out_payload));
  StatToFuseAttr(&new_node.attr, &out_payload.attr);
  out_payload.nodeid = new_node.attr.st_ino;
  out_payload.generation = 0;
  out_payload.entry_valid = 10;
  out_payload.attr_valid = 10;
  StatToFuseAttr(&new_node.attr, &out_payload.attr);

  out_header.len = sizeof(fuse_out_header) + sizeof(fuse_entry_out);
  out_header.error = 0;

  std::vector<char> response(out_header.len);
  memcpy(response.data(), &out_header, sizeof(out_header));
  memcpy(response.data() + sizeof(out_header), &out_payload, sizeof(out_payload));

  write(fd, response.data(), response.size());
}

void HandleInit(int fd, fuse_in_header* in_header) {
  fuse_out_header out_header;
  out_header.unique = in_header->unique;

  fuse_init_out out_payload;
  memset(&out_payload, 0, sizeof(out_payload));
  out_payload.major = FUSE_KERNEL_VERSION;
  out_payload.minor = FUSE_KERNEL_MINOR_VERSION;

  out_header.len = sizeof(fuse_out_header) + sizeof(fuse_init_out);
  out_header.error = 0;

  std::vector<char> response(out_header.len);
  memcpy(response.data(), &out_header, sizeof(out_header));
  memcpy(response.data() + sizeof(out_header), &out_payload, sizeof(out_payload));

  write(fd, response.data(), response.size());
}

void HandleFlush(int fd, fuse_in_header* in_header) {
  fuse_out_header out_header;
  out_header.unique = in_header->unique;
  out_header.len = sizeof(fuse_out_header);
  out_header.error = 0;
  write(fd, &out_header, sizeof(out_header));
}

void HandleNotImplemented(int fd, fuse_in_header* in_header) {
  // Return ENOSYS to let FUSE know the operation is not implemented.
  fuse_out_header out_header;
  out_header.unique = in_header->unique;
  out_header.len = sizeof(fuse_out_header);
  out_header.error = -ENOSYS;
  write(fd, &out_header, sizeof(out_header));
}

void HandleOpen(int fd, fuse_in_header* in_header) {
  fuse_out_header out_header;
  out_header.unique = in_header->unique;

  fuse_open_out out_payload;
  out_payload.fh = 1;  // Dummy file handle
  out_payload.open_flags = FOPEN_DIRECT_IO;

  out_header.len = sizeof(fuse_out_header) + sizeof(fuse_open_out);
  out_header.error = 0;

  std::vector<char> response(out_header.len);
  memcpy(response.data(), &out_header, sizeof(out_header));
  memcpy(response.data() + sizeof(out_header), &out_payload, sizeof(out_payload));

  write(fd, response.data(), response.size());
}

}  // namespace

int main(int argc, char* argv[]) {
  if (argc < 3) {
    fprintf(stderr, "Usage: %s <fuse_dev> <mountpoint>\n", argv[0]);
    return 1;
  }

  std::string fuse_dev = argv[1];
  std::string mountpoint = argv[2];

  int fd = open(fuse_dev.c_str(), O_RDWR);
  if (fd == -1) {
    fprintf(stderr, "Failed to open %s: %s\n", fuse_dev.c_str(), strerror(errno));
    return 1;
  }

  std::string options = "fd=" + std::to_string(fd) + ",rootmode=40000,user_id=0,group_id=0";

  if (mount("memfs", mountpoint.c_str(), "fuse", 0, options.c_str()) == -1) {
    fprintf(stderr, "Failed to mount filesystem: %s\n", strerror(errno));
    return 1;
  }

  fprintf(stderr, "Filesystem mounted at %s\n", mountpoint.c_str());
  constexpr char kReadyMessage[] = "ready";
  write(kFuseReadyFd, kReadyMessage, sizeof(kReadyMessage));

  // Add root directory
  g_filesystem["/"].attr.st_ino = 1;
  g_filesystem["/"].attr.st_mode = S_IFDIR | 0755;
  g_filesystem["/"].attr.st_nlink = 2;

  std::vector<char> buffer(FUSE_MIN_READ_BUFFER);
  while (true) {
    ssize_t bytes_read = read(fd, buffer.data(), buffer.size());
    if (bytes_read == -1) {
      if (errno == ENODEV) {
        fprintf(stderr,
                "Read from fuse device returned ENODEV - we probably have been unmounted.\n");
        return 0;
      }
      fprintf(stderr, "Failed to read from %s: %s\n", fuse_dev.c_str(), strerror(errno));
      return 1;
    }

    fuse_in_header* in_header = reinterpret_cast<fuse_in_header*>(buffer.data());
    switch (in_header->opcode) {
      case FUSE_LOOKUP:
        HandleLookup(fd, in_header);
        break;
      case FUSE_GETATTR:
        HandleGetattr(fd, in_header);
        break;
      case FUSE_MKNOD:
        HandleMknod(fd, in_header);
        break;
      case FUSE_INIT:
        HandleInit(fd, in_header);
        break;
      case FUSE_FLUSH:
        HandleFlush(fd, in_header);
        break;
      case FUSE_OPEN:
        HandleOpen(fd, in_header);
        break;
      case FUSE_RELEASE:
      case FUSE_CREATE:
        HandleNotImplemented(fd, in_header);
        break;
      default: {
        fprintf(stderr, "Unknown FUSE opcode: %d\n", in_header->opcode);
        HandleNotImplemented(fd, in_header);
        break;
      }
    }
  }

  umount(mountpoint.c_str());
  close(fd);

  return 0;
}
