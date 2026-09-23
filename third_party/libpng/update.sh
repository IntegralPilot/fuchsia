#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Usage: ./update.sh <version_tag>
# Example: ./update.sh v1.6.58

set -e

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <version_tag>"
    echo "Example: $0 v1.6.58"
    exit 1
fi

VERSION_TAG=$1
# Strip leading 'v' for the version number if present
VERSION=${VERSION_TAG#v}

echo "Updating libpng to ${VERSION_TAG}..."

# 1. Ensure we are in the right directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

# 2. Get the commit hash of the tag
REVISION=$(git -C src rev-parse "${VERSION_TAG}")
if [ $? -ne 0 ]; then
    echo "Error: Could not find tag ${VERSION_TAG} in src directory."
    exit 1
fi

# 3. Merge upstream
echo "Merging ${VERSION_TAG} into src/..."
# We use || true because we expect conflicts due to local deletions
git -C src merge "${VERSION_TAG}" --no-commit --no-ff || true

# 4. Resolve "deleted by us" conflicts
echo "Resolving conflicts (keeping local deletions)..."
git -C src status --porcelain | grep '^DU' | cut -c 4- | xargs -r git -C src rm

# 5. Re-apply file stripping
echo "Stripping unnecessary files..."
(
    cd src
    rm -rf ci contrib projects scripts tests \
    ANNOUNCE CHANGES INSTALL TODO autogen.sh configure.ac CMakeLists.txt Makefile.am \
    aclocal.m4 compile config.guess config.sub configure depcomp install.sh ltmain.sh \
    missing test-driver libpng-manual.txt .github .editorconfig .editorconfig-checker.json \
    *.in *.3 *.5 *.png *.jpg *.dfa *.yaml .markdownlint.yml
)

# 6. Finalize merge
echo "Committing merge..."
git -C src add -A
git -C src commit -m "Merge libpng version ${VERSION}: tag '${REVISION}'

- Strip all unneeded files as per README.fuchsia.
- Resolve modify/delete conflicts by keeping deletions."

# 7. Update pnglibconf.h
echo "Updating pnglibconf.h..."
CURRENT_YEAR=$(date +%Y)
sed -i "s/libpng [0-9.]* CUSTOM/libpng ${VERSION} CUSTOM/g" pnglibconf.h
sed -i "s/libpng version [0-9.]*/libpng version ${VERSION}/g" pnglibconf.h
sed -i "s/Copyright (c) 2018-[0-9]*/Copyright (c) 2018-${CURRENT_YEAR}/g" pnglibconf.h

# 8. Update README.fuchsia metadata
echo "Updating README.fuchsia..."
sed -i "s/^Version: .*/Version: ${VERSION}/" README.fuchsia
sed -i "s/^Revision: .*/Revision: ${REVISION}/" README.fuchsia
sed -i "s/Updated to [0-9.]*/Updated to ${VERSION}/" README.fuchsia
sed -i "s/merge-base JIRI_HEAD v[0-9.]*/merge-base JIRI_HEAD ${VERSION_TAG}/g" README.fuchsia

# 9. Update Local Modifications section
echo "Regenerating Local Modifications list..."
# Find the line number of the git diff command in README.fuchsia
LINE=$(grep -n "git diff \$(git merge-base JIRI_HEAD" README.fuchsia | cut -d: -f1)
if [ -n "${LINE}" ]; then
    # Keep everything up to the command line
    head -n "${LINE}" README.fuchsia > README.fuchsia.new
    # Append the new diff
    git -C src diff "${VERSION_TAG}" HEAD --name-status | sed 's/^/  /' >> README.fuchsia.new
    mv README.fuchsia.new README.fuchsia
else
    echo "Warning: Could not find Local Modifications section in README.fuchsia to update."
fi

echo "------------------------------------------------------------"
echo "Update to ${VERSION_TAG} completed successfully."
echo "Revision: ${REVISION}"
echo "------------------------------------------------------------"
echo "Next steps:"
echo "1. Verify the build: fx build //third_party/libpng:libpng"
echo "2. Run relevant tests (e.g., scenic unittests)."
echo "3. Review changes in README.fuchsia and pnglibconf.h."
