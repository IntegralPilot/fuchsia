#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
import colorama
from colorama import Fore, Style

colorama.init()


class ColoramaTest(unittest.TestCase):
    def test_color_output(self):
        print(
            Fore.GREEN
            + Style.BRIGHT
            + "This text is green"
            + Style.NORMAL
            + " (except in infra)"
            + Style.RESET_ALL
        )
        print("This text is normal")


if __name__ == "__main__":
    unittest.main()
