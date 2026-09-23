# syscalls test data

## ext4 image

Created without the 64bit feature with:

* `truncate -s 1M mount_ext4.img`
* `mkfs.ext4 mount_ext4.img -O ^64bit`
* `sudo mkdir /mnt/tmp`
* `sudo mount -oloop mount_ext4.img /mnt/tmp`
* `sudo cp hello_world.txt /mnt/tmp/`
* `sudo umount /mnt/tmp`
* `e2fsck -f mount_ext4.img`
* `resize2fs -M mount_ext4.img` (counting number of reported blocks)
* `truncate -o --size NN mount_ext4.img` (where NN=number of blocks above)
