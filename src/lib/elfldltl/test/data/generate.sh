#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

compile() {
    clang -s -shared -nostdlib -o $1-$2$3e.so --target=$4-linux -E${3^^} -D${1^^} -Wl,-zmax-page-size=4096 -x assembler-with-cpp - <<'EOF'
// Note, the value of these symbols are 1 for first and 2 for second. It doesn't
// matter that a and b are effictively aliases in second because the test
// compares their symbol table symbols and not just their value. The value is
// used in the test along with Symbol address to ensure we resolved to the correct
// one.

#ifdef GNU_UNIQUE

.global a
.type a, %gnu_unique_object
.set a, 1

#elif defined(FIRST)

.global a
.set a, 1

.weak c
.set c, 1

.weak weak_both
.set weak_both, 1

#elif defined(SECOND)

.global a
.set a, 2

.global b
.set b, 2

.global c
.set c, 2

.weak weak_both
.set weak_both, 2

#else
#error "Bad defines"
#endif
EOF
}

for f in first second gnu_unique
do
    for arch_width in aarch64,64 arm,32
    do
        IFS=",";
        set -- $arch_width;
        for e in b l
        do
            compile $f $2 $e $1
        done
    done
done
