# syscalls test data

## Overview of Test Files

This directory contains test data used by the `dm-verity` tests in
`device_mapper_test.rs`. `dm-verity` guarantees the integrity of block devices
using a cryptographic Merkle hash tree. The tests simulate various
configurations of data and hash tree combinations.

### Data files:
- **`device_mapper_ext4.img`**: A basic EXT4 filesystem image containing a single file
  (`hello_world.txt`). This represents the raw data block device used in the
  device mapper verity tests.
- **`hashtree_sha256.txt`** (and `hashtree_sha512.txt`): Contains *only* the
  Merkle hash tree blocks generated from `device_mapper_ext4.img`. The `veritysetup`
  tool prepends a 4KB configuration header to its output, but because `dm-verity`
  expects the pure tree without the header, the hashtree files have that first
  4KB block stripped away.
- **`root_hash_sha256.txt`** (and `root_hash_sha512.txt`): The top-level
  cryptographic hash of the Merkle tree. `dm-verity` requires this to verify
  the rest of the tree.

### Combined (Shared Loop Device) files:
Some configurations append the hash tree to the very end of the data device. We
test this "shared device" setup using:
- **`valid_image_with_hashtree_sha256.txt`**: A concatenation of `device_mapper_ext4.img`
  followed directly by `hashtree_sha256.txt`.
- **`valid_image_with_corrupted_hashtree_sha256.txt`**: Similar to above, but
  the appended with a corrupted hashtree (the hashtree does not match the data
  blocks in the image). Reading it will result in an I/O error (`EIO`)
  because it fails to validate against the corrupt hash tree.

### Mismatched Tree files:
- **`corrupted_hashtree_sha256.txt`** (and `..._sha512.txt`): A mathematically
  valid hash tree, but generated from a corrupted version of the image.
- **`corrupted_root_hash_sha256.txt`** (and `..._sha512.txt`): The true root
  hash of the tampered tree. Passing this matching root hash simulates an
  artificial testing scenario where `dm-verity` succeeds load-time validation.
  This allows the test to isolate and verify read-time validation failures,
  because the data blocks in `device_mapper_ext4.img` will fail to match the leaf
  hashes of this tampered tree.

## Generating Test Files

### ext4 image

Created without the 64bit feature with:

* `truncate -s 1M device_mapper_ext4.img`
* `mkfs.ext4 device_mapper_ext4.img -O ^64bit`
* `sudo mkdir /mnt/tmp`
* `sudo mount -oloop device_mapper_ext4.img /mnt/tmp`
* `sudo cp hello_world.txt /mnt/tmp/`
* `sudo umount /mnt/tmp`
* `e2fsck -f device_mapper_ext4.img`
* `resize2fs -M device_mapper_ext4.img` (counting number of reported blocks)
* `truncate -o --size NN device_mapper_ext4.img` (where NN=number of blocks above)

### Generating Hash Trees

The DM_TABLE_LOAD ioctl for dm-verity requires the user to pass in a hashtree
file that contains a merkle tree generated from the contents of the ext4 image.

`veritysetup` can accept either block devices or standard file paths. The
instructions for both are shown below. For reference, the SHA-256 files were
originally generated using loop devices.

#### Using loop devices
1. Set up two loop devices. One backing the ext4 image and one backing the
   hashtree.
* `sudo losetup -f src/starnix/tests/syscalls/rust/data/device_mapper_ext4.img`
* `sudo dd if=/dev/zero bs=4k conv=notrunc oflag=append count=2 of=/tmp/hashtree`
*(NOTE: count here is the number of blocks that we need to store the merkle tree.
Can be adjusted for larger ext4 images.)*
* `sudo losetup -f /tmp/hashtree`

2. Figure out which loop devices are associated with which files (`sudo losetup -a`).
e.g.
```
/dev/loop19: (/usr/local/google/home/nikitajindal/fuchsia/src/starnix/tests/syscalls/rust/data/device_mapper_ext4.img)

/dev/loop20: (/tmp/hashtree)
```

3. Use `veritysetup format` to generate the merkle tree:
* `sudo veritysetup format /dev/loop19 /dev/loop20 --salt ffffffffffffffff`
*(The printed root hash is manually copied into `data/root_hash_sha256.txt`)*

4. Copy the contents, removing the first 4KB veritysetup header block.
* `dd if=/tmp/hashtree of=src/starnix/tests/syscalls/rust/data/hashtree_sha256.txt bs=1 skip=4096`

#### Using direct files
1. Generate the hash tree directly from the file (and save the root hash):
   * `veritysetup format src/starnix/tests/syscalls/rust/data/device_mapper_ext4.img /tmp/hashtree_sha512 --hash=sha512 --salt=ffffffffffffffff`
2. Remove the header block and save the hash tree:
   * `dd if=/tmp/hashtree_sha512 of=src/starnix/tests/syscalls/rust/data/hashtree_sha512.txt bs=1 skip=4096`

### Generating Corrupted Hash Trees

To test verification failure paths, we use a valid hash tree that was generated
from a *corrupted* image. If we mount the original `device_mapper_ext4.img` but pass
this corrupted tree, verification will fail.

1. Create a corrupted image by flipping the first byte of the simple image:
   * `python3 -c 'with open("src/starnix/tests/syscalls/rust/data/device_mapper_ext4.img", "rb") as f: d = bytearray(f.read()); d[0] ^= 0xFF; open("/tmp/corrupt_ext4.img", "wb").write(d)'`

2. Generate the corrupted hash tree directly from the corrupted file (and save
   the root hash):
   * SHA-256: `veritysetup format /tmp/corrupt_ext4.img /tmp/corrupt_hashtree_sha256 --salt=ffffffffffffffff`
   * SHA-512: `veritysetup format /tmp/corrupt_ext4.img /tmp/corrupt_hashtree_sha512 --hash=sha512 --salt=ffffffffffffffff`

3. Remove the header block and save the corrupted hash tree:
   * SHA-256: `dd if=/tmp/corrupt_hashtree_sha256 of=src/starnix/tests/syscalls/rust/data/corrupted_hashtree_sha256.txt bs=1 skip=4096`
   * SHA-512: `dd if=/tmp/corrupt_hashtree_sha512 of=src/starnix/tests/syscalls/rust/data/corrupted_hashtree_sha512.txt bs=1 skip=4096`

4. For the shared loop device test (SHA-256 only), append the corrupted tree
   to the simple image:
   * `cat src/starnix/tests/syscalls/rust/data/device_mapper_ext4.img src/starnix/tests/syscalls/rust/data/corrupted_hashtree_sha256.txt > src/starnix/tests/syscalls/rust/data/valid_image_with_corrupted_hashtree_sha256.txt`
