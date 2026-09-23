# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

function __fuchsia_test_completions
    # figure out what our command line looks like so we can alter our query
    set -l cmd (commandline -opc)
    set -l last_token $cmd[(count $cmd)]

    # We need jq to parse our tests.json file. If we don't have it return
    if command -v jq >/dev/null 2>&1
        set _jq jq
    else if command -v fx >/dev/null 2>&1
        set _jq "fx jq"
    else
        return
    end

    set -l build_dir "$FUCHSIA_DIR"/(__fuchsia_build_dir)
    set -l tests_file "$build_dir"/tests.json
    if test -e $tests_file
        switch $last_token
            case --package -p
                # Just search for package names
                set -l query '.[].test.package_url | select(. != null)'
                # Use awk to split the url and remove the "#meta"
                eval $_jq -r '$query' $tests_file | awk -F/ '{print substr($4, 1, length($4)-5)}'
                return
            case --component -c
                # Just search for component names
                set -l query '.[].test.package_url | select(. != null)'
                # Use awk to split the url and remove the ".cm"
                eval $_jq -r '$query' $tests_file | awk -F/ '{print substr($5, 1, length($5)-3)}'
                return
            case --exact
                # Only return exact names
                set -l query '.[].test.name | select(. != null)'
                eval $_jq -r '$query' $tests_file
                return
        end

        # If we have no flags look for both the name and the package_url
        set -l query '.[].test | .name,.package_url,.label | select(. != null)'
        eval $_jq -r '$query' $tests_file
    end
end

# Add completions for `fx test`
complete -c fx -n "__fish_seen_subcommand_from test" -a "(__fuchsia_test_completions)"
