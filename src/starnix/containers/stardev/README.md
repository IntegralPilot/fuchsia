# stardev

The `stardev` container is intended for self-hosted development of Fuchsia.

## Status

Currently, only very basic software development is possible in `stardev`.

## Getting started

### Create the image

These commands use the `Dockerfile` in this directory to create the disk image for the container,
which is exported into the `//local` directory.

```sh
$ docker build -t stardev src/starnix/containers/stardev
$ docker save stardev:latest -o local/stardev.tar
```

### Build Fuchsia

Configure the Fuchsia build to include the `stardev` container:

```sh
$ fx set workbench_eng.x64 \
    --release \
    --with //src/starnix/containers/stardev \
    --args 'stardev_path = "//local/stardev.tar"' \
    --args 'starnix_enable_console_tool = true'
$ fx build
```

The `--release` flag is necessary to get reasonable performance for self-hosted development workloads.

### Run the container

After building and running Fuchsia, you can run `stardev` in the Starnix playground:

```sh
$ src/starnix/containers/stardev/start.sh
```

Later, if you want to stop the container, you can run the following command:

```sh
$ src/starnix/containers/stardev/stop.sh
```

### Get a shell

Once you have the container running, you can connect to the container via the console:

```sh
$ src/starnix/containers/stardev/shell.sh
```

### Installing software

Inside the container, you can install Debian packages using `apt-get` as normal:

```sh
# apt-get update
# apt-get install -y vim git clang
```

Currently, these packages get installed to file system that is backed by memory, which means they
are removed when the container is restarted. If you want to avoid installing these packages every
time you run the container, you can run `apt-get` from your `Dockerfile`, which will store the
packages in the persistent image.

## Hello, world

We can build and run a simple C program. First, create a directory and a `main.c` file:

```sh
# mkdir /tmp/hello_world
# cd /tmp/hello_world
# vim main.c
```

For example, you can use the following program:

```c
#include <stdio.h>

int main(int argc, char** argv) {
  printf("hello, starnix\n");
  return 0;
}
```

Next, build and run the program:

```sh
# clang main.c -Wall -Werror -o hello_world
# ./hello_world
hello, starnix
```
