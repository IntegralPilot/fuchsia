# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.



function __fuchsia_product_bundles
    if command -v jq >/dev/null 2>&1
        set _jq jq
    else if command -v fx >/dev/null 2>&1
        set _jq "fx jq"
    else
        return
    end

    set -l build_dir "$FUCHSIA_DIR"/(__fuchsia_build_dir)
    eval $_jq -r ".[].name" "$build_dir"/product_bundles.json
end

# Add completions for `fx set-main-pb` which will suggest the product bundles that are
# available to use.
complete -c fx -n "__fish_seen_subcommand_from set-main-pb; and not __fish_seen_subcommand_from (__fuchsia_product_bundles)" \
    -a "(__fuchsia_product_bundles)"
