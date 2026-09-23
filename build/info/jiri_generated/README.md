This directory contains files that reflect the commit hash and timestamp of
//integration/.git/HEAD.

- integration_commit_hash.txt: git hash of HEAD in integration.git.
- integration_commit_stamp.txt: commit time of HEAD in integration.git, as a UTC
  UNIX timestamp.
- integration_daily_commit_hash.txt: Like integration_commit_hash.txt, but for
  the most recent commit before midnight (UTC) of the day that HEAD was
  committed. If you've just run `jiri update`, this is probably yesterday's last
  CL.
- integration_daily_commit_stamp.txt: commit time of the commit described in
  integration_daily_commit_hash.txt, as a UTC UNIX timestamp.

A Jiri hook, in //integration/fuchsia/stem, is used to invoke
`//build/info/create_jiri_hook_files.sh` which will overwrite them, on each
`jiri sync` operation, with up-to-date values.

The files are listed in this directory's `.gitignore` to ensure that their
updates are properly ignored by git commands.

For context, see https://fxbug.dev/335391299
