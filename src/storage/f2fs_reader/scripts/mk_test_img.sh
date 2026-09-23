#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Produces a small test image for use with f2fs_reader.
#
# Requires:
#  * root.
#  * fscryptctl (https://github.com/google/fscryptctl/)
#
# Produces ../testdata/f2fs.img.zst
#
# Usage:
#   # sudo ${PWD}/mk_test_img.sh
#
set -e
PATH=${PATH}:/usr/local/bin/

# Prerequisites.
apt-get install f2fs-tools zstd fsverity
modprobe f2fs
rm -f /tmp/f2fs.img /tmp/f2fs_snapshot.img ../testdata/f2fs.img.zst

MOUNT_PATH=/tmp/f2fs_mnt

cleanup() {
  exec 3<&- 2>/dev/null || true
  exec 4<&- 2>/dev/null || true
  exec 5<&- 2>/dev/null || true
  if mountpoint -q "${MOUNT_PATH}"; then
    umount "${MOUNT_PATH}" 2>/dev/null || true
  fi
  rm -f /tmp/f2fs.img /tmp/f2fs_snapshot.img
}
trap cleanup EXIT

# Build empty image.
dd if=/dev/zero bs=4096 count=65536 of=/tmp/f2fs.img
mkfs.f2fs -f -O encrypt,verity -l testimage /tmp/f2fs.img

# Mount and populate.
mkdir -p ${MOUNT_PATH}
mount -o loop -t f2fs /tmp/f2fs.img ${MOUNT_PATH}

REGULAR_PATH=${MOUNT_PATH}/a/b/c
mkdir -p ${REGULAR_PATH}
echo "inline_data" > ${REGULAR_PATH}/inlined
dd if=/dev/zero bs=4096 count=8 of=${REGULAR_PATH}/regular
echo -n "01234567" >> ${REGULAR_PATH}/regular

ln -s regular ${REGULAR_PATH}/symlink
ln ${REGULAR_PATH}/regular ${REGULAR_PATH}/hardlink
touch ${REGULAR_PATH}/chowned
chown 999:999 ${REGULAR_PATH}/chowned

# Large directory (2,000 entries)
mkdir ${MOUNT_PATH}/large_dir
for i in $(seq 0 2000); do
  touch ${MOUNT_PATH}/large_dir/${i}
done

# Large directory with deleted files.
mkdir ${MOUNT_PATH}/large_dir2
for i in $(seq 0 2000); do
  touch ${MOUNT_PATH}/large_dir2/${i}
done
for i in $(seq 0 1999); do
  rm ${MOUNT_PATH}/large_dir2/${i}
done

# Sparse files across nids.
# from i_addrs
echo -n "foo" > ${MOUNT_PATH}/sparse.dat
# from nids[0] n = 923
dd conv=notrunc if=/dev/zero bs=4096 count=1 seek=923 of=${MOUNT_PATH}/sparse.dat
# from nids[1] n += 1018 = 1941
dd conv=notrunc if=/dev/zero bs=4096 count=1 seek=1941 of=${MOUNT_PATH}/sparse.dat
# from nids[2] n += 1018 = 2959
dd conv=notrunc if=/dev/zero bs=4096 count=1 seek=2959 of=${MOUNT_PATH}/sparse.dat
# from nids[3] n += 1018^2 = 1039283
dd conv=notrunc if=/dev/zero bs=4096 count=1 seek=1039283 of=${MOUNT_PATH}/sparse.dat
# from nids[4] n += 1018^2 * 100 = 104671683
dd conv=notrunc if=/dev/zero bs=4096 count=1 seek=104671683 of=${MOUNT_PATH}/sparse.dat
echo -n "bar" >> ${MOUNT_PATH}/sparse.dat

# xattr
attr -s a -V "value" ${MOUNT_PATH}/sparse.dat
attr -s b -V "value" ${MOUNT_PATH}/sparse.dat
attr -s c -V "value" ${MOUNT_PATH}/sparse.dat
attr -r b ${MOUNT_PATH}/sparse.dat

# xattr padding tests
# 1 byte value -> 3 bytes padding
attr -s padding_test_1 -V "v" ${MOUNT_PATH}/sparse.dat
# 2 byte value -> 2 bytes padding
attr -s padding_test_2 -V "va" ${MOUNT_PATH}/sparse.dat
# 3 byte value -> 1 byte padding
attr -s padding_test_3 -V "val" ${MOUNT_PATH}/sparse.dat
# 4 byte value -> 0 bytes padding
attr -s padding_test_4 -V "valu" ${MOUNT_PATH}/sparse.dat
# 5 byte value -> 3 bytes padding
attr -s padding_test_5 -V "value" ${MOUNT_PATH}/sparse.dat

VERITY_PATH=${MOUNT_PATH}/verity
mkdir -p ${VERITY_PATH}
#Enable verity on a file that is normally inlined, but will not be after setting verity.
echo "inline_data" > ${VERITY_PATH}/inlined
fsverity enable ${VERITY_PATH}/inlined
#Enable verity on an otherwise normal file.
dd if=/dev/zero bs=4096 count=8 of=${VERITY_PATH}/regular
echo -n "01234567" >> ${VERITY_PATH}/regular
fsverity enable ${VERITY_PATH}/regular

# Enable verity on a large file. The digest will include all the zeroed areas making for a large
# merkle tree, to ensure that we can support however they handle layers. Not using the above
# sparse file because it is huge and would create many MB of merkle tree.
echo -n "foo" > ${VERITY_PATH}/merkle_layers.dat
dd conv=notrunc if=/dev/zero bs=4096 count=1 seek=129 of=${VERITY_PATH}/merkle_layers.dat
echo -n "bar" >> ${VERITY_PATH}/merkle_layers.dat
fsverity enable ${VERITY_PATH}/merkle_layers.dat

# fscrypt
#
# We will use a hard-coded 512-bit key of all zeros for this test.
KEY_IDENTIFIER=$(dd if=/dev/zero bs=1 count=64 status=none | fscryptctl add_key ${MOUNT_PATH})

mkdir ${MOUNT_PATH}/fscrypt
fscryptctl set_policy --padding=16 --iv-ino-lblk-32 ${KEY_IDENTIFIER} ${MOUNT_PATH}/fscrypt

# Track inode number to get the encrypted names out later.
declare -A INODES

mkdir -p ${MOUNT_PATH}/fscrypt/a/b
INODES["$(stat -c "%i" ${MOUNT_PATH}/fscrypt/a)"]="a"
INODES["$(stat -c "%i" ${MOUNT_PATH}/fscrypt/a/b)"]="b"
# Nb: encrypted files should never be inlined.
# The following data is more than 16 bytes to ensure that we validate the xts tweak during decoding.
echo -n "test45678abcdef_12345678" > ${MOUNT_PATH}/fscrypt/a/b/inlined
dd if=/dev/zero bs=4096 count=1 of=${MOUNT_PATH}/fscrypt/a/b/regular
echo "asdf" >> ${MOUNT_PATH}/fscrypt/a/b/regular
#Enable verity on a "regular" encrypted file.
fsverity enable ${MOUNT_PATH}/fscrypt/a/b/regular
ln -s "inlined" ${MOUNT_PATH}/fscrypt/a/b/symlink
INODES["$(stat -c "%i" ${MOUNT_PATH}/fscrypt/a/b/symlink)"]="symlink"

# Test filenames of different lengths to ensure we use a compatible proxy
# filename scheme.
LONG_NAME_16=xxxxxxxxyyyyyyyy
LONG_NAME_32=${LONG_NAME_16}${LONG_NAME_16}
LONG_NAME_64=${LONG_NAME_32}${LONG_NAME_32}
LONG_NAME_128=${LONG_NAME_64}${LONG_NAME_64}
LONG_NAME_192=${LONG_NAME_128}${LONG_NAME_64}
touch ${MOUNT_PATH}/fscrypt/1
touch ${MOUNT_PATH}/fscrypt/12
touch ${MOUNT_PATH}/fscrypt/123
touch ${MOUNT_PATH}/fscrypt/1234
touch ${MOUNT_PATH}/fscrypt/12345
touch ${MOUNT_PATH}/fscrypt/${LONG_NAME_192}

# A large amount of data that we need to copy (not fscrypt, needs encrypting)
echo "large zero..."
dd if=/dev/zero bs=4096 count=4096 of=${MOUNT_PATH}/large_zero
# A fscrypt file (no copy - already encrypted)
echo "large zero in fscrypt..."
dd if=/dev/zero bs=4096 count=4096 of=${MOUNT_PATH}/fscrypt/large_zero

# fscrypt nested directories
echo "deep nesting fscrypt..."
$(
	cd ${MOUNT_PATH}/fscrypt/
	for i in $(seq 0 400); do
		mkdir d
		cd d
	done
	touch f
)

# Args: "decrypted_name" "file_path1" "file_path2" ...
lookup_inode() {
	local target="${1}"
	shift 1
	for f in $@
	do
		if [[ ${INODES["$(stat -c "%i" $f)"]} == "${target}" ]]
		then
			echo $(basename $f)
			return 0
		fi
	done
	"Inode for '${target}' not found" >&2
	return 1
}

# Remove the key and get the encrypted names back from the inodes.
fscryptctl remove_key ${KEY_IDENTIFIER} ${MOUNT_PATH}
str_a=$(lookup_inode "a" ${MOUNT_PATH}/fscrypt/*)
echo "let str_a = \"${str_a}\";"

str_b=$(lookup_inode "b" ${MOUNT_PATH}/fscrypt/${str_a}/*)
echo "let str_b = \"${str_b}\";"

str_symlink=$(lookup_inode "symlink" ${MOUNT_PATH}/fscrypt/${str_a}/${str_b}/*)
echo "let str_symlink = \"${str_symlink}\";"
echo -n "let bytes_symlink_content = "
echo "b\"$(readlink ${MOUNT_PATH}/fscrypt/${str_a}/${str_b}/${str_symlink})\";"

echo "Expected:"
for f in ${MOUNT_PATH}/fscrypt/*
do
	echo "\"$(basename $f)\"",
done

# Create orphan files (files unlinked while held open by a process)
touch ${MOUNT_PATH}/orphan1 ${MOUNT_PATH}/orphan2 ${MOUNT_PATH}/orphan3
exec 3< ${MOUNT_PATH}/orphan1
exec 4< ${MOUNT_PATH}/orphan2
exec 5< ${MOUNT_PATH}/orphan3
rm -f ${MOUNT_PATH}/orphan1 ${MOUNT_PATH}/orphan2 ${MOUNT_PATH}/orphan3
sync

# Snapshot image while files are still open as orphans (clean unmount would purge them).
cp /tmp/f2fs.img /tmp/f2fs_snapshot.img

zstd -f /tmp/f2fs_snapshot.img -o ../testdata/f2fs.img.zst

echo "Done!"

