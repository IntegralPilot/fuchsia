// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "args.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <stdlib.h>

#include <cstring>

namespace {
int ParseCommonArgs(int argc, char** argv, const char** error, std::string* interface) {
  while (argc > 1) {
    if (!strncmp(argv[1], "--interface", 11)) {
      if (argc < 3) {
        *error = "netsvc: missing argument to --interface";
        return -1;
      }
      *interface = argv[2];
      argv++;
      argc--;
    }
    argv++;
    argc--;
  }
  return 0;
}

uint32_t NamegenParse(const std::string& str) {
  if (str == "0") {
    return 0;
  }
  return 1;
}

}  // namespace

int ParseArgs(int argc, char** argv, const device_name_provider_config::Config& config,
              const char** error, DeviceNameProviderArgs* out) {
  // Reset the args.
  *out = DeviceNameProviderArgs();

  out->interface = config.primary_interface();
  out->nodename = config.nodename();
  out->namegen = NamegenParse(config.namegen());

  int err = ParseCommonArgs(argc, argv, error, &out->interface);
  if (err) {
    return err;
  }

  out->devdir = kDefaultDevdir;

  while (argc > 1) {
    if (!strcmp(argv[1], "--nodename")) {
      if (argc < 3) {
        *error = "netsvc: missing argument to --nodename";
        return -1;
      }
      out->nodename = argv[2];
      argv++;
      argc--;
    }
    if (!strcmp(argv[1], "--devdir")) {
      if (argc < 3) {
        *error = "netsvc: missing argument to --devdir";
        return -1;
      }
      out->devdir = argv[2];
      argv++;
      argc--;
    }
    if (!strcmp(argv[1], "--namegen")) {
      if (argc < 3) {
        *error = "netsvc: missing argument to --namegen";
        return -1;
      }
      out->namegen = NamegenParse(std::string{argv[2]});
      argv++;
      argc--;
    }
    argv++;
    argc--;
  }
  return 0;
}
