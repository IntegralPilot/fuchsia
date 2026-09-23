// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/netsvc/args.h"

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
}  // namespace

int ParseArgs(int argc, char** argv, const netsvc_config::Config& config, const char** error,
              NetsvcArgs* out) {
  // Reset the args.
  *out = NetsvcArgs();

  // First parse from config args, then use use cmdline args as overrides.
  out->interface = config.primary_interface();
  out->disable = config.disable();
  out->netboot = config.netboot();
  out->advertise = config.advertise();
  out->all_features = config.all_features();

  int err = ParseCommonArgs(argc, argv, error, &out->interface);
  if (err) {
    return err;
  }
  while (argc > 1) {
    const struct {
      std::string_view name;
      bool* flag;
      bool value = true;
    } flags[] = {
        {
            "--netboot",
            &out->netboot,
        },
        {
            "--nodename",
            &out->print_nodename_and_exit,
        },
        {
            "--advertise",
            &out->advertise,
        },
        {
            "--all-features",
            &out->all_features,
        },
        {
            "--log-packets",
            &out->log_packets,
        },
        {
            "--enable",
            &out->disable,
            false,
        },
    };
    for (const auto& f : flags) {
      if (f.name == argv[1]) {
        *(f.flag) = f.value;
        break;
      }
    }
    argv++;
    argc--;
  }
  return 0;
}
