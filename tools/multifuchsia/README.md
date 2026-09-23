# `multifuchsia` Tool for managing btrfs snapshots of fuchsia checkouts.

A `multifuchsia` setup consists of a root directory (of your choice) called
`$MULTIFUCHSIA_ROOT`. In it are a set of fuchsia checkouts:

*   `clean/` which is the original source of the snapshots. You can enable a
    nightly job which runs `jiri update` and `fx build` and makes a snapshot in
    `snapshots/build.success`
*   `snapshots/build.success` the results of the most recent successful clean
    build. You can make a new one with `./multifuchsia sync_and_build` or
    `systemctl --user start fuchsia_update_and_build.service`.
*   `snapshots/update.success` (optional) The most recent build used to OTA a
    fuchsia device with `./multifuchsia update`. If enabled, it's used as the
    source of individual checkouts (otherwise they just use
    `snapshots/build.success`)
*   `checkouts/$name` individual checkouts made with `./multifuchsia checkout
    $name`. You can have as many of these as you want, and they come in 2
    flavors: plain snapshots, and worktrees.

A second directory called `$MOUNTPOINT` is where each of the fuchsia checkouts
will be mounted during the nightly builds and whenever you use `fcd`,
`./multifuchsia mount`, and `./multifuchsia enter`. This ensures that any
absolute paths which sneak into built artifacts will still work even when you
copy the build artifacts around with snapshots. Ideally this would be a nice
path that's easy to open with your editor of choice, for example
`~/src/fuchsia`. But it could also be a subdirectory of `$MULTIFUCHSIA_ROOT`
like `$MULTIFUCHSIA_ROOT/mount`, if you want.

## Caveats

*   You might frequently see the warning `WARNING: cannot read default subvolume
    id: Operation not permitted`. Unless it is followed by an **error**, it is
    not a cause for concern. This is rooted in a [subtlely of permissions when
    traversing btrfs subvolumes](https://lore.kernel.org/linux-btrfs/YYniaHjEO9ssCiGq@angband.pl/T/).
    It can be safely ignored for now.

## A tour of `multifuchsia`

### `./multifuchsia sync_and_build`

working with `$MULTIFUCHSIA_ROOT/clean` in an isolated environment, performs a
`jiri update` and an `fx build`, then updates `snapshots/build.success` with the
result (if successful). Usually run as a nightly job by
`fuchsia_update_and_build.service` triggered by
`fuchsia_update_and_build.timer`.

Generally recommend to run via `systemctl start --user
fuchsia_update_and_build.service` since that ensures that two don't run
simultaneously, but there isn't a huge risk of that happening if you're running
it during the day since the timer defaults to running at midnight.

### `./multifuchsia update`

updates the snapshot at `snapshots/update.success` and performs an `fx ota` to
update your running fuchsia device to the latest build.

Note: if you don't want to use `./multifuchsia update` (perhaps because you
don't have a fuchsia device on your desk, then you can just have
`snapshots/update.success` be a symlink to `snapshots/build.success`, which
makes it so that new builds are immediately used by new checkouts.

Note2: `./multifuchsia update` currently always publishes packages to a local
repository serving "fuchsia.com". It does not yet support arbitrary URLs for
packages. It also does not support serving multiple repositories.

### `./multifuchsia checkout $name`

creates a new checkout in `checkouts/$name` from `snapshots/update.success`. It
can optionally turn fuchsia.git into a worktree of the fuchsia.git in `clean` if
you run it as `./multifuchsia checkout $name worktree`, or change the default
checkout mode in `multifuchsia.rc`.

Note: worktrees are incompatible with several tools used by fuchsia. In a
worktree checkout, you can't use `jiri` at all (although you can still get
updates via `./multifuchsia rebase`), you have to turn off version stamping by
setting the `kernel_version_string` gn argument to a placeholder value, and `fx
bazel` doesn't work. Probably all these limitations could be lifted with focused
effort, but ordinary snapshot checkouts might be a bit less brittle, for now.

benefits of worktree checkouts:

*   branches in a worktree are saved in the clean checkout, so deleting a
    checkout doesn't cause loss of data.
*   git packfiles are shared with the clean checkout, so if the clean checkout
    gets repacked, it will still share space with the worktree checkout.

### `./multifuchsia enter $checkout`

creates an isolated shell environment where the `$checkout` directory is
bind-mounted on your chosen `$MOUNTPOINT`.
Other processes on your machine will not see
the bind mount, only those run inside the shell started by `./multifuchsia
enter` will see it. When finished, use `exit` or Ctrl+D (EOF) to leave. Since
the bind mount is isolated, you can have different shells entered into different
checkouts both running a build, and they won't interfere with each other
(although each build will consume memory and CPU, of course)

### `./multifuchsia mount $name`

(requires `sudo`)

bind-mounts `checkouts/$name` on your chose `$MOUNTPOINT` without any isolation.
There can only be one checkout mounted at a time this way, but it is visible to
all processes on the system (with the exception of stuff in a `./multifuchsia
enter` environment, which will see their own mount). This is handy for vscode
remote sessions, or for running tools which don't work well inside the isolated
environment.

Note1: it also creates the checkout if it doesn't already exist, but this might
not be the desired / expected behavior. Maybe that should only be for `mfcd`?

Note2: `./multifuchsia mount` fails if it can't unmount what is already mounted
on the mountpoint. The most common problem is a shell which is `cd`ed into the
mountpoint. Some other common causes include: `ffx` daemons, `fx shell`
connections, and the vscode remote server. `lsof $MOUNTPOINT` and `fuser
-m $MOUNTPOINT` can find most causes, but for some reason not all. Anyway, it's
not usually too hard to look through the running processes and find one which is
using it, then kill it.

### Digression about `$MOUNTPOINT`

The `multifuchsia` workflow always runs builds from the single configured
`$MOUNTPOINT` (configured in `multifuchsia.rc`). This is to ensure the build
artifacts generated in the clean checkout can be safely used with the
snapshotted checkouts. It's conceivable that you could get this workflow to work
without the bind-mounting, but you'd have to be confident that no tool anywhere
in the build references the absolute path of the checkout. There's actually some
enforcement of this in the fuchsia build today, but it's not complete, and also
allows opt-outs, so it's not something we can rely on for correctness.
Bind-mounting ensures that even if an absolute path does make it into build
artifacts, those artifacts can still be used in the checkouts, since the
bind-mounting will ensure that the absolute paths stay the same.

### `./multifuchsia cleanup`

a tool for listing the contents of checkouts, with the ultimate goal of letting
you delete the ones that are no longer important. Because multifuchsia makes it
incredibly cheap to make checkouts, it's really easy to end up with a lot of
them. And over time, the clean checkout will change more and more from
individual checkouts, causing them to use more and more space (since the files
will no longer be de-duplicated between them)

It lists the commit in the repository which aren't present on `JIRI_HEAD`, along
with whether the CL has been submitted (by searching for the change ID in
clean's `origin/main`). It also includes whether the working directory is dirty
(as measured by `git status`).

For example, here's one of my checkouts, with a submitted CL, a not-yet
submitted CL, and a dirty working directory:

```
checkouts/bt:
  [dirty] working directory
  [not submitted] a89597cb99ae [media][codec] remove unused allow for non-hermetic package
  [submitted] fdd3ead4a56e [bt-hfp-audio-gateway-tests] switch to subpackage for codec_factory
```

It does this for each checkout, then at the end provides a list of the checkouts
which have a clean working directory and no unsubmitted CLs, along with a
copy-pastable command for deleting them. (It does not delete anything on it's
own)

Note: the dirty working directory detection is built on `git status` which means
it ignores `.gitignore`d files. While this is most likely what you want (Having
ever completed a build in a checkout does not imply it should be kept forever.),
it is worth keeping it in mind, in case you have files in `out/` or another
ignored directory that you might want to keep.

### `./multifuchsia pack`

also about reducing space usage, but for checkouts which do have unsubmitted
changes in them. It simply commits any uncommitted changes into a wip commit,
and then stores the result in a git branch.

### `./multifuchsia rebase`

rebases a checkout on a new snapshot. What this means is: doing a `git rebase`
onto the commit used by the latest snapshot, then swapping out the current
checkout for a fresh one, and then checking out the freshly-rebased commit
there. In practice what this means is you get a brand new checkout but with all
the same local changes in it. So an `fx build` will only need to build the stuff
affected by your local changes. This is also good for reducing disk usage since
it'll maximize the de-duplication with the clean checkout.

It supports both snapshot checkouts and worktree checkouts, and also has some
spacial handling for `git-branchless` workflows.

Warning: unlike `./multifuchsia cleanup`, `./multifuchsia rebase` does not
handle changes in repositories other than fuchsia.git, nor does it have
guardrails for that, so changes there will be lost if you rebase a checkout with
such changes. Sorry about that, it's something that should be fixed.

### `./multifuchsia status`

Tells you about the latest state of your nightly build, including how old your
snapshots are and the output of `systemctl status` on the job.

Example:

```
$ ./multifuchsia status
snapshots/build.success/integration: 8e17bc7669 31 minutes ago [roll] Roll fuchsia [superproject] Roll third_party/pigweed third_party/fuchsia: Include LICENSE in copy.bara.sky
snapshots/update.success/integration: c88c3b39e7 24 hours ago [roll] Roll fuchsia [hwstress] Update OWNERS
× fuchsia_update_and_build.service - Run a fuchsia update and build
     Loaded: loaded (/usr/local/google/home/computerdruid/.config/systemd/user/fuchsia_update_and_build.service; linked; preset: enabled)
    Drop-In: /usr/local/google/home/computerdruid/.config/systemd/user/fuchsia_update_and_build.service.d
             └─hourly.conf, workdir.conf
     Active: failed (Result: exit-code) since Thu 2023-06-08 15:06:11 PDT; 11min ago
TriggeredBy: ● fuchsia_update_and_build.timer
    Process: 2220247 ExecStart=/usr/local/google/home/computerdruid/extra_data/fuchsia/external/tools/multifuchsia/systemd/../multifuchsia sync_and_build_here (code=exited, status=2)
   Main PID: 2220247 (code=exited, status=2)
        CPU: 1h 8min 16.578s
Jun 08 15:06:11 earthshaker.mtv.corp.google.com multifuchsia[2220251]: ./multifuchsia: line 571: syntax error near unexpected token `)'
Jun 08 15:06:11 earthshaker.mtv.corp.google.com systemd[5762]: fuchsia_update_and_build.service: Main process exited, code=exited, status=2/INVALIDARGUMENT
Jun 08 15:06:11 earthshaker.mtv.corp.google.com systemd[5762]: fuchsia_update_and_build.service: Failed with result 'exit-code'.
Jun 08 15:06:11 earthshaker.mtv.corp.google.com systemd[5762]: Failed to start fuchsia_update_and_build.service - Run a fuchsia update and build.
Jun 08 15:06:11 earthshaker.mtv.corp.google.com systemd[5762]: fuchsia_update_and_build.service: Consumed 1h 8min 16.578s CPU time.
```

Although it's a bit more colorful in person.

### bash/zsh aliases

`mfcd $name`: does `./multifuchsia mount $name` and then `cd $MOUNTPOINT`. Even
if we change mount to not auto-create new checkouts, `mfcd` maybe should still
auto-create checkouts. There's also tab-completion for the checkout names, and
you can run it from anywhere. Also, `mfcd` with no arguments takes you to
`$MULTIFUCHSIA_ROOT`, which is handy.

`mfenter $name`: enters the `checkouts/$name` with `./multifuchsia enter` and
can similarly be run from anywhere. This one has a much less strong case for
being a bash alias, since it doesn't have to `cd` it can't really do anything
that `./multifuchsia enter` can't. So if we make all the commands accept paths
OR names, I'll probably just delete this

Both of these aliases feature tab completion for checkout names.

Please consider contributing aliases for your shell of choice.

## Setup Instructions

1.  Have a btrfs filesystem with plenty of space mounted somewhere. Make sure to
    mount it with the mount option `user_subvol_rm_allowed`, so that the nightly
    update is allowed to delete the snapshots it makes. You also might want to
    enable compression on the volume. For example:

    ```
    $ mount -o compress=zstd,user_subvol_rm_allowed /path/to/btrfsBlockDevice /mnt/btrfs
    ```

2.  Designate some directory in it as `$MULTIFUCHSIA_ROOT` (which doesn't itself
    need to be a subvolume)

    Setting this as a shell variable with simplify setup (makes it easier to
    copy/paste the snippets below. For example:

    ```
    $ export MULTIFUCHSIA_ROOT=/mnt/btrfs/multifuchsia
    ```

3.  Following the directions at
    <https://fuchsia.dev/fuchsia-src/get-started/get_fuchsia_source>, download
    fuchsia into `$MULTIFUCHSIA_ROOT/clean`, making sure to make it a btrfs
    subvolume.

    * {Steps}

        1.  Create the subvolume and change into it:

            ```posix-terminal
            btrfs subvolume create "$MULTIFUCHSIA_ROOT/clean"

            cd "$MULTIFUCHSIA_ROOT/clean"
            ```

        2.  Download and decode the script:

            ```posix-terminal
            curl -s "https://fuchsia.googlesource.com/fuchsia/+/HEAD/scripts/bootstrap?format=TEXT" | base64 --decode > bootstrap.sh
            ```

        3.  (Recommended) Verify the contents of the `bootstrap.sh` script.

        4.  Run the script:

            ```posix-terminal
            bash bootstrap.sh
            ```

        5.  Move the files to the root of the subvolume:

            ```posix-terminal
            mv fuchsia to-delete && mv to-delete/* to-delete/.* ./ && rmdir to-delete
            ```

    * {One-liner}

        If you prefer to run it in a single command, you can use the following:

        ```posix-terminal
        btrfs subvolume create "$MULTIFUCHSIA_ROOT/clean"

        cd "$MULTIFUCHSIA_ROOT/clean"

        curl -s "https://fuchsia.googlesource.com/fuchsia/+/HEAD/scripts/bootstrap?format=TEXT" | base64 --decode > bootstrap.sh && bash bootstrap.sh

        mv fuchsia to-delete && mv to-delete/* to-delete/.* ./ && rmdir to-delete
        ```

4.  Symlink the multifuchsia script to your root work dir:

    ```posix-terminal
    ln -s "$MULTIFUCHSIA_ROOT/clean/tools/multifuchsia/multifuchsia" "$MULTIFUCHSIA_ROOT/"
    ```

    Important: When running `multifuchsia`, make sure to run it via this
    symlink, which enables it to figure out which `$MULTIFUCHSIA_ROOT` to use.

5.  Copy `multifuchsia.rc.example` to `$MULTIFUCHSIA_ROOT/multifuchsia.rc` and
    edit it to your preference:

    ```posix-terminal
    cp \
        "$MULTIFUCHSIA_ROOT/clean/tools/multifuchsia/multifuchsia.rc.example" \
        "$MULTIFUCHSIA_ROOT/multifuchsia.rc"
    ```

6.  Set up the fuchsia build in clean by doing:

    ```posix-terminal
    cd $MULTIFUCHSIA_ROOT

    ./multifuchsia enter ./clean

    ./scripts/fx set core.x64 --release

    exit
    ```

    With your choice of fx set arguments. Although consider setting `--args
    'kernel_version_string="dev"'` if you want to be able to use worktree
    checkouts.

8.  Make parent directories for snapshots and checkouts:

    ```posix-terminal
    mkdir -p "$MULTIFUCHSIA_ROOT/snapshots" "$MULTIFUCHSIA_ROOT/checkouts"
    ```

9.  (Optional) Try your first build without using the systemd unit with:

    ```posix-terminal
    "$MULTIFUCHSIA_ROOT/multifuchsia" sync_and_build
    ```

10. Make symlinks for systemd units:

    ```posix-terminal
    mkdir -p ~/.config/systemd/user/

    ln \
          -s "$MULTIFUCHSIA_ROOT/clean/tools/multifuchsia/systemd/"* \
          ~/.config/systemd/user/
    ```

11. Create systemd drop-in in
    `~/.config/systemd/user/fuchsia_update_and_build.service.d/workdir.conf`

    ```posix-terminal
    mkdir -p ~/.config/systemd/user/fuchsia_update_and_build.service.d

    cat <<EOF > ~/.config/systemd/user/fuchsia_update_and_build.service.d/workdir.conf
    [Service]
    WorkingDirectory=$MULTIFUCHSIA_ROOT
    EOF
    ```

12. Try to run the unit:

    ```posix-terminal
    systemctl --user daemon-reload

    systemctl --user start fuchsia_update_and_build.service

    systemctl --user status fuchsia_update_and_build.service
    ```

13. If it all seems to be working, enable the timer to run it nightly:

    ```posix-terminal
    systemctl --user enable --now fuchsia_update_and_build.timer
    ```

14. Enable your timers to run when you are logged out:

    ```posix-terminal
    loginctl enable-linger $(whoami)
    ```

15. (Optional) If you use bash or zsh, enable the `mfcd` and `mfenter` aliases:

    bash:

    ```posix-terminal
    echo 'eval "$("'"$MULTIFUCHSIA_ROOT"'/multifuchsia" env bash)"' >> ~/.bashrc
    ```

    zsh:

    ```posix-terminal
    echo 'eval "$("'"$MULTIFUCHSIA_ROOT"'/multifuchsia" env zsh)"' >> ~/.zshrc
    ```

    If you use a different shell, please consider contributing patch with support for
    your preferred shell.

Extra notes:

*   If you want to have multiple multifuchsia roots, you can use the systemd
    instance feature to have multiple, like
    `fuchsia_update_and_build@internal.service` and
    `fuchsia_update_and_build@internal.timer`.
*   If you want to skip the `./multifuchsia update` part of the workflow, create
    a symlink from `snapshots/update.success` to `build.success`.
*   With multifuchsia you will have multiple `$FUCHSIA_DIR` directories, at
    least one per checkout. Hence adding `$FUCHSIA_DIR/.jiri_root/bin/` to your
    `PATH` in your shell resource config file will lead to undesired side
    effects. As [recommended in the Fuchsia developer docs](https://fuchsia.dev/fuchsia-src/development/build/fx?hl=en#setting-up-fx)
    for `fx`, leveraging tools like `direnv` or `dotenv` is highly recommended.

<!-- Reference links -->
