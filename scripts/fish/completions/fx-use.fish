# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

function __fuchsia_build_dirs
    if test -n "$FUCHSIA_DIR"
        for d in (find $FUCHSIA_DIR/out -maxdepth 1 -type d)
            if test -e $d/args.gn
                echo out/(basename $d)
            end
        end
    end
end

# Add completions for `fx use` which will suggest the out directories that are
# available to use.
complete -c fx -n "__fish_seen_subcommand_from use; and not __fish_seen_subcommand_from (__fuchsia_build_dirs)" \
    -a "(__fuchsia_build_dirs)"
