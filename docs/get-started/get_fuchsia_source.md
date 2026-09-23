# Download the Fuchsia source code

This guide provides instructions on how to download the
Fuchsia source code and set up the Fuchsia development
environment on your machine.

The steps are:

1. [Install prerequisite packages](#install-prerequisite-packages).
2. [Perform a preflight check](#perform-a-preflight-check).
3. [Download the Fuchsia source code](#download-the-fuchsia-source-code).
4. [Set up environment variables](#set-up-environment-variables).
5. [Configure firewall rules (Optional)](#configure-firewall-rules).


## 1. Install prerequisite packages {#install-prerequisite-packages}

Fuchsia requires `curl`, `file`, `unzip`, and `git` to be up to date.
The version of `git` needs to be 2.31 or higher.

To install or update these packages, run the following command:

```posix-terminal
sudo apt install curl file git unzip
```

(**Optional**) If you want to run the [Fuchsia emulator (FEMU)][femu],
consider completing the following additional steps before proceeding
to the next section:

* [Enable KVM acceleration](#enable-kvm-acceleration)
* [Configure emulator networking](#configure-emulator-networking)

## 2. Perform a preflight check {#perform-a-preflight-check}

Fuchsia provides a preflight check tool
([`ffx platform preflight`][ffx-platform-preflight])
that examines your machine and informs you of any issues that may
affect building Fuchsia from source on the machine.

Note: The preflight tool works for the `x64` architecture only.

To perform a preflight check, run the following command:

```posix-terminal
curl -sO https://storage.googleapis.com/fuchsia-ffx/ffx-linux-x64 && chmod +x ffx-linux-x64 && ./ffx-linux-x64 platform preflight
```

## 3. Download the Fuchsia source code {#download-the-fuchsia-source-code}

The Fuchsia [bootstrap script][bootstrap-script] creates a directory
named `fuchsia` and downloads the Fuchsia source code.

Downloading the Fuchsia source code requires about 2 GB of space
on your machine. Depending on your build configuration, you need
another 80 to 90 GB of space later when you build Fuchsia. Additionally,
the download process uses a substantial amount of memory. It is advisible
to close non-crucial processes during this time.

To download the Fuchsia source, do the following:

1.  Select a directory for downloading the Fuchsia source code, for example:

    Note: You can set up Fuchsia in any directory. This guide selects
    the `$HOME` directory as an example.

    ```posix-terminal
    cd $HOME
    ```

1.  Run the bootstrap script to create the `fuchsia` directory and download the source code:

    Note: Depending on your network speed, downloading the Fuchsia source code
    (about 2 GB) can take a while.

    * {Steps}

        1.  Download and decode the script:

            ```posix-terminal
            curl -s "https://fuchsia.googlesource.com/fuchsia/+/HEAD/scripts/bootstrap?format=TEXT" | base64 --decode > bootstrap.sh
            ```

        2.  (Recommended) Verify the contents of the `bootstrap.sh` script.

        3.  Run the script:

            ```posix-terminal
            bash bootstrap.sh
            ```

            Note: The script is configured to automatically delete the
            temporary `bootstrap.sh` file upon completion or exit.

            Note: If you have a slower internet connection and run into
            timeouts, edit the `bootstrap.sh` script directly to increase the
            values of `-fetch-packages-timeout` and `-hook-timeout`. These
            timeouts are in minutes, and indicate a 2 hour timeout for
            package downloads. (The `-hook-timeout` value is multiplied
            by 5 when downloading CIPD packages.)

    * {One-liner}

        If you prefer to run it in a single command, you can use the following:

        ```posix-terminal
        curl -s "https://fuchsia.googlesource.com/fuchsia/+/HEAD/scripts/bootstrap?format=TEXT" | base64 --decode > bootstrap.sh && bash bootstrap.sh
        ```

    If you see the `Invalid authentication credentials` error during the
    bootstrapping process, see [Authentication error](#authentication-error) for
    help.

## 4. Set up environment variables {#set-up-environment-variables}

Note: If you don't wish to update your shell profile, see
[Work on Fuchsia without updating your PATH](#work-on-fuchsia-without-updating-your-path)
in Appendix instead.

To configure the recommended Fuchsia environment variables in your shell profile,
do the following:

1.  Use a text editor to open your `~/.bash_profile` file (in the example below,
    we use the [Nano][nano]{:.external} text editor):

    Note: This guide uses a `bash` terminal as an example. If you're
    using `zsh`, replace `~/.bash_profile` with `~/.zprofile` in the
    following steps.

    ```posix-terminal
    nano ~/.bash_profile
    ```

1.  Add the following lines to your `~/.bash_profile` file:

    Note: If your Fuchsia checkout is not located in the `~/fuchsia`
    directory, replace `~/fuchsia` with your Fuchsia directory.

    ```sh
    export PATH=~/fuchsia/.jiri_root/bin:$PATH
    source ~/fuchsia/scripts/fx-env.sh
    ```

    *   The `.jiri_root/bin` directory in the Fuchsia source contains the
        [`jiri`](https://fuchsia.googlesource.com/jiri) and
        [`fx`](/docs/development/build/fx.md) tools that are essential to
        Fuchsia workflows. Fuchsia uses the `jiri` tool to manage repositories in
        the Fuchsia project, and the `fx` tool helps configure, build, run, and
        debug Fuchsia. The Fuchsia toolchain requires that `jiri` is available in
        your `PATH`.

    *   The [`fx-env.sh`](https://cs.opensource.google/fuchsia/fuchsia/+/main:/scripts/fx-env.sh) script enables a number of
        useful shell functions in your terminal. For instance, it creates the
        `FUCHSIA_DIR` environment variable and provides the `fd` command for
        navigating directories with auto-completion. (For more information, see
        comments in `fx-env.sh`.)

1.  Save the file and exit the text editor.

1.  Update environment variables:

    ```posix-terminal
    source ~/.bash_profile
    ```

To verify your environment setup, do the following:

1. Go to your `fuchsia` directory, for example:

   ```posix-terminal
   cd ~/fuchsia
   ```

2. Verify that you can run the following commands without errors:

   ```posix-terminal
   jiri help
   ```

   ```posix-terminal
   fx help
   ```

## 5. Configure firewall rules (Optional) {#configure-firewall-rules}

Note: This step is **not required** for building or running Fuchsia. But it is
recommended to ensure that Fuchsia's emulator instances run smoothly on Linux.

To allow Fuchsia-specific traffic on the host machine, run the following command:

```posix-terminal
fx setup-ufw
```

This script requires `sudo` (which asks for your password) to set the appropriate
firewall rules. For more information on this script, see [`setup-ufw`][setup-ufw].

## Next steps

To build your first Fuchsia system image, see
[Configure and build Fuchsia](/docs/get-started/build_fuchsia.md).

## Appendix

### Authentication error {#authentication-error}

If you see the `Invalid authentication credentials` error during the bootstrap
process, your `~/.gitcookies` file may contain cookies from some repositories in
`googlesource.com` that the bootstrap script wants to check out anonymously.

To resolve this error, do one of the following:

*   Follow the onscreen directions to get passwords for the specified
    repositories.
*   Delete the offending cookies from the `.gitcookies` file.

### Work on Fuchsia without updating your PATH {#work-on-fuchsia-without-updating-your-path}

The following sections provide alternative approaches to the
[Set up environment variables](#set-up-environment-variables) section:

*   [Copy the tool to your binary directory](#copy-the-tool-to-your-binary-directory)
*   [Add a symlink to your binary directory](#add-a-symlink-to-your-binary-directory)

#### Copy the tool to your binary directory {#copy-the-tool-to-your-binary-directory}

If you don't wish to update your environment variables, but you want `jiri` to
work in any directory, copy the `jiri` tool to your `~/bin` directory, for
example:

Note: If your Fuchsia source code is not located in the `~/fuchsia` directory,
replace `~/fuchsia` with your Fuchsia directory.

```posix-terminal
cp ~/fuchsia/.jiri_root/bin/jiri ~/bin
```

However, you must have write access to the `~/bin` directory without `sudo`. If
you don't, `jiri` cannot keep itself up-to-date.

#### Add a symlink to your binary directory {#add-a-symlink-to-your-binary-directory}

Similarly, if you want to use the `fx` tool without updating your environment
variables, provide the `fx` tool's symlink in your `~/bin` directory, for
example:

Note: If your Fuchsia source code is not located in the `~/fuchsia` directory,
replace `~/fuchsia` with your Fuchsia directory.

```posix-terminal
ln -s ~/fuchsia/scripts/fx ~/bin
```

Alternatively, run the `fx` tool directly using its path, for example:

```posix-terminal
./scripts/fx help
```

In either case, you need `jiri` in your `PATH`.

### Enable KVM acceleration {#enable-kvm-acceleration}

(**Linux only**) Most Linux machines support VM acceleration through KVM,
which greatly improves the performance and usability of the emulator.

If KVM is available on your machine, add yourself to the `kvm` group:

```posix-terminal
sudo usermod -a -G kvm ${USER}
```

For this change to take effect, you may need to log out and log back in.

### Configure emulator networking {#configure-emulator-networking}

(**Linux only**) To allow the [Fuchsia emulator (FEMU)][femu] to
access the network, you can configure an IPv6 network using
[TUN/TAP][tuntap]{: .external}.

To enable networking using [tap networking][tap-networking]{: .external},
do the following:

1. Set up `tuntap`:

   ```posix-terminal
   sudo ip tuntap add dev qemu mode tap user $USER
   ```

1. Enable the network for `qemu`:

   ```posix-terminal
   sudo ip link set qemu up
   ```

<!-- Reference links -->

[bootstrap-script]: https://cs.opensource.google/fuchsia/fuchsia/+/main:/scripts/bootstrap
[ffx-platform-preflight]: https://fuchsia.dev/reference/tools/sdk/ffx#preflight
[nano]: https://www.nano-editor.org/docs.php
[setup-ufw]: https://fuchsia.dev/reference/tools/fx/cmd/setup-ufw
[femu]: set_up_femu.md
[tuntap]: https://en.wikipedia.org/wiki/TUN/TAP
[tap-networking]: https://wiki.qemu.org/Documentation/Networking#Tap

