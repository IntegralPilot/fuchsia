Fxfs can run in Linux using FUSE, where FUSE is an interface for running
custom filesystems in userspace without needing to change Linux kernel code.

Slides: http://go/fxfs-linux-slides
Design doc: http://go/fxfs-linux-doc

This README outlines the steps to run Fxfs in a Linux FUSE mount.

**Step 1: build fxfs binary tool**

```shell
# Configure the build.
$ fx set core.x64 --with //src/storage/fxfs/tools
# Build the fxfs binary tool.
$ fx build
```

**Step 2: run fxfs to initialize the mount**

```shell
Terminal 1:
# Create a Linux directory for mounting.
# Only needed if mount_dir does not exist.
$ mkdir ~/mount_dir
# Option 1: Run the FUSE mount by creating a new device.
$ fx fxfs create_file_fuse ~/mount_dir ~/device
# Option 2: Run the FUSE mount by opening an existing device.
$ fx fxfs open_file_fuse ~/mount_dir ~/device

Terminal 2:
$ cd ~/mount_dir
# Do anything inside mount_dir!
```

**Step 3: gracefully shutdown fxfs**

```shell
Terminal 1:
# Shutdown fxfs (this should also unmount the directory)
$ Ctrl + C
```

**KNOWN ISSUES**
1. Performance is not as good as native filesystems like ext4 (https://fxbug.dev/42072632)
2. Extended attributes are not supported yet (https://fxbug.dev/42072627)
