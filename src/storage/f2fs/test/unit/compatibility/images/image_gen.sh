#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

IMAGE_SIZE=100M
MOUNT_DIR=image_mnt

setup() {
  local image_name=${1}
  local is_inline=${2:-false}

  truncate --size ${IMAGE_SIZE} ${image_name}
  mkfs.f2fs -f ${image_name}
  if ${is_inline}; then
    mount -t f2fs -o inline_data,inline_dentry ${image_name} ${MOUNT_DIR}
  else
    mount -t f2fs ${image_name} ${MOUNT_DIR}
  fi
  pushd ${MOUNT_DIR}
}

clean() {
  local image_name=$1

  popd
  umount ${MOUNT_DIR}
  zstd ${image_name}
  rm ${image_name}
}

directory_test() {
  IMAGE="directory_test.img"
  setup ${IMAGE}

  mkdir -p depth
  current="depth"
  for i in $(seq 0 59); do
    current="$current/$i"
    mkdir -p "$current"
  done

  mkdir -p width
  for i in $(seq 0 59); do
    mkdir -p "width/$i"
  done

  for i in $(seq 20 39); do
    mv "width/$i" "width/$((i + 100))"
  done

  for i in $(seq 40 59); do
    rm -rf "width/$i"
  done

  clean ${IMAGE}
}

write_pattern() {
    local outfile=${1}
    local blockcount=${2}
    blocksize=4096

    : > "$outfile"

    for i in $(seq 0 $(($blockcount-1))); do
        val=$(($i % 256))
        byte="$(printf "\\$(printf '%03o' $val)")"
        perl -e "print pack('C', $val) x $blocksize" >> "$outfile"
    done
}

file_test() {
  IMAGE="file_test.img"
  setup ${IMAGE}

  FILE=file_write
  touch ${FILE}
  write_pattern ${FILE} 16

  FILE=file_truncate
  touch ${FILE}
  truncate --size 16384 ${FILE}

  FILE=file_truncate_shrink
  touch ${FILE}
  write_pattern ${FILE} 16
  truncate --size 16384 ${FILE}

  FILE=file_exceed
  touch ${FILE}
  write_pattern ${FILE} 2
  truncate --size 7168 ${FILE}

  FILE=file_rename
  touch ${FILE}
  mv ${FILE} renamed_file

  FILE=file_fallocate
  fallocate -l 65536 ${FILE}

  FILE=file_fallocate_hole
  touch ${FILE}
  write_pattern ${FILE} 16
  fallocate --punch-hole --keep-size -o 8192 -l 8192 ${FILE}

  for i in $(seq 0 512); do # 512 decimal = 777 octal
    mode=`printf "%03o" $i`
    FILE=filemode_${mode}
    touch ${FILE}
    chmod $mode ${FILE}
  done

  clean ${IMAGE}
}

inline_test() {
  IMAGE="inline_test.img"
  setup ${IMAGE} true

  FILE=inline_file
  echo "hello" > ${FILE}

  FILE=inline_file_empty
  touch ${FILE}

  DIR=inline_dir
  mkdir ${DIR}
  touch ${DIR}/a
  touch ${DIR}/b
  touch ${DIR}/c

  clean ${IMAGE}
}

mkdir -p ${MOUNT_DIR}
directory_test
file_test
inline_test
rmdir ${MOUNT_DIR}
