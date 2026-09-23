#!/bin/sh

# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

if [ $# -ne 1 ]
then
  echo "Usage: do-update.sh /path/to/chromium/base/numerics"
  exit 65
fi

pushd $1
REV=`git rev-parse HEAD`
popd

echo "Updating to Chromium revision $REV"

# Copy in pristine version.
cp $1/* .

# Update revision.
sed -i -e "s/Revision:.*/Revision: $REV/" README.fuchsia

# Remove Chromium-specific file.
rm -f DEPS

# Replace header guards and some macros.
sed -i -e 's/BASE_NUMERICS_/SAFEMATH_/g' *.h
sed -i -e 's/BASE_/SAFEMATH_/g' *.h

# Update include paths.
sed -i -e 's/#include "base\/numerics\/\(.*\)"/#include <safemath\/\1>/g' *.h *.cc

# Update to local namespace.
sed -i -e 's/namespace base/namespace safemath/g' *.h *.cc
sed -i -e 's/numerics_internal/internal/g' *.h *.cc
sed -i -e 's/base::/safemath::/g' *.h *.cc

# Update .md documentation.
sed -i -e 's/base\/numerics/safemath/g' *.md

# Reformat due to different line lengths, but don't reformat to local style to
# keep diff small.
../../../../prebuilt/third_party/clang/linux-x64/bin/clang-format -i -style=Chromium *.h *.cc

rm -rf include/safemath
mkdir -p include/safemath
mv *.h include/safemath/

mkdir -p tests
rm -f tests/*.cc
mv *unittest*.cc tests
