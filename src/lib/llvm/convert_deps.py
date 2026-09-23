# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Usage is in BUILD.gn.
while True:
    try:
        line = input()
    except:
        break
    # line is "libLLVMTextAPI.a: libLLVMSupport.a libLLVMTargetParser.a"
    target, rest = line.split(":", 1)
    deps = [c[3:-2] for c in rest.split() if c]
    print(f'llvm_library("{target[3:-2]}")' + " {")
    print("  deps = [")
    for d in deps:
        print(f'    ":{d}",')
    print("  ]")
    print("}\n")
