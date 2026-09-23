#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This code is utilized by infra's run_script.py to automatically build and upload these
# containers to CIPD as per b/427767342.
set -e

# If specified, infra uses this to upload the packages created here.
CIPD_YAML_MANIFEST=""
ARCHITECTURES="amd64 arm64"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cipd-yaml-manifest)
      CIPD_YAML_MANIFEST="$2"
      echo "Creating cipd YAML manifest at: ${CIPD_YAML_MANIFEST}"
      shift
      shift
      ;;
    --architectures)
      ARCHITECTURES="$2"
      shift
      shift
      ;;
    *)
      echo "Unrecognized argument: $1"
      exit 1
      ;;
  esac
done

# Move to the directory of the script to find Dockerfile
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)

FUCHSIA_ROOT=$(git rev-parse --show-toplevel)
if [ -z "${FUCHSIA_ROOT}" ]; then
    echo "Could not find fuchsia root directory. Are you in a git repository?"
    exit 1
fi

# Dockerfile is expected to be at src/starnix/containers/alpine/Dockerfile
DOCKERFILE_DIR="${FUCHSIA_ROOT}/src/starnix/containers/alpine"

# Register qemu handlers for multi-platform builds.
docker run --rm --privileged multiarch/qemu-user-static --reset -p yes

OUT_DIR="${FUCHSIA_ROOT}/out/alpine_cipd"
rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

echo "Building and saving docker images for alpine..."
for arch in ${ARCHITECTURES}; do
  echo "Building for ${arch}..."
  DOCKER_DEFAULT_PLATFORM="linux/${arch}" docker build -t "alpine-${arch}" "${DOCKERFILE_DIR}"

  echo "Saving docker image for ${arch}..."
  arch_dir="${OUT_DIR}/${arch}"
  mkdir -p "${arch_dir}"
  docker save -o "${arch_dir}/alpine.tar" "alpine-${arch}:latest"

  echo "Cleaning up docker images for ${arch}..."
  docker rmi "alpine-${arch}:latest"
  docker image prune -f
done

GIT_REV=$(git -C "${FUCHSIA_ROOT}" rev-parse HEAD)
GIT_REPO=$(git -C "${FUCHSIA_ROOT}" config --get remote.origin.url)

CIPD_CLIENT="cipd"


echo "Creating README.fuchsia..."
cat <<EOF > "${OUT_DIR}/README.fuchsia"
Name: alpine
Upstream Git: https://hub.docker.com/_/alpine
Version: ha256:c5b1261d6d3e43071626931fc004f70149baeba2c8ec672bd4f27761f8e1ad6b
Security Critical: no
License: MIT
License File: LICENSE
Description:
A minimal Docker image based on Alpine Linux.
EOF

echo "Creating LICENSE..."
cat <<EOF > "${OUT_DIR}/LICENSE"
MIT License

Copyright (c) 2019 Natanael Copa

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
EOF

function create_cipd_yaml() {
  local arch=$1
  local cipd_package_name="fuchsia/starnix/alpine-image-${arch}"
  local cipd_yaml_file="${OUT_DIR}/cipd-${arch}.yaml"

  echo "Creating ${arch} package..."

  tee <<EOF > "${cipd_yaml_file}"
package: ${cipd_package_name}
install_mode: copy
data:
  - file: ${arch}/alpine.tar
  - file: README.fuchsia
  - file: LICENSE
EOF
}

for arch in ${ARCHITECTURES}; do
  create_cipd_yaml "${arch}" "${OUT_DIR}/${arch}"
done

if [[ -n "$CIPD_YAML_MANIFEST" ]]; then
  # The output file is a JSON file that contains a list of YAML files.
  # It is consumed by the run_script recipe.
  echo "[" > "${CIPD_YAML_MANIFEST}"
  first=true
  for arch in ${ARCHITECTURES}; do
    if [ "$first" = true ]; then
      first=false
    else
      echo "," >> "${CIPD_YAML_MANIFEST}"
    fi
    tee -a <<EOF >> "${CIPD_YAML_MANIFEST}"
    {
        "path": "out/alpine_cipd/cipd-${arch}.yaml",
        "tags": {
            "git_repository": "${GIT_REPO}",
            "git_revision": "${GIT_REV}"
        }
    }
EOF
  done
  echo "]" >> "${CIPD_YAML_MANIFEST}"
fi
