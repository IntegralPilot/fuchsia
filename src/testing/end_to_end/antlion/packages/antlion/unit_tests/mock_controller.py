#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This is a mock third-party controller module used for unit testing antlion.

import logging

MOBLY_CONTROLLER_CONFIG_NAME = "MagicDevice"


def create(configs):
    objs = []
    for c in configs:
        if isinstance(c, dict):
            c.pop("serial")
        objs.append(MagicDevice(c))
    return objs


def destroy(objs):
    print("Destroying magic")


def get_info(objs):
    infos = []
    for obj in objs:
        infos.append(obj.who_am_i())
    return infos


class MagicDevice(object):
    def __init__(self, config):
        self.magic = config

    def get_magic(self):
        logging.info("My magic is %s.", self.magic)
        return self.magic

    def who_am_i(self):
        return {"MyMagic": self.magic}
