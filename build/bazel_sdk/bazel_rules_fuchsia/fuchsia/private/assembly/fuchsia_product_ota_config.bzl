# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for creating OTA configurations for products that use the Omaha Client."""

load(
    "@fuchsia_rules_common//assembly:providers.bzl",
    "FuchsiaOmahaOtaConfigInfo",
)

def ota_realm(name, app_id, tuf, channels, check_interval = None):
    """Defines an OTA Realm.

    Args:
      name: the realm name.
      app_id: The omaha app_id of the realm.
      tuf: The TUF repo configuration for the realm (created by `tuf_repo()`).
      channels: A list of the channels in the realm.
      check_interval: The rate to poll for updates (optional)

    Returns:
      A struct defining the realm for use with `fuchsia_product_ota_config()`
    """
    if not name:
        fail("realm name cannot be None")
    if not app_id:
        fail("%s: realm must have an app id" % name)
    if not channels:
        fail("%s: realm must have ota channels" % name)

    return struct(
        name = name,
        app_id = app_id,
        tuf = tuf,
        channels = channels,
        check_interval = check_interval,
    )

def tuf_repo(hostnames, root, mirror_url):
    """Defines the TUF repo for a realm.

    Args:
      hostnames: The hostname(s) the TUF repo can be found at.
      root:  The root key for the repo.
      mirror_url:  The mirror that contains the blobs for the repo.

    Returns:
      A struct defining the TUF repo for use with `realm()`
    """
    if not hostnames:
        fail("'hostnames' cannot be None")
    if not root:
        fail("'root' cannot be None")
    if not mirror_url:
        fail("'mirror_url' cannot be None")

    return struct(
        hostnames = hostnames,
        root = root,
        mirror_url = mirror_url,
    )

def tuf_repo_root(version, type, value, signing_threshold):
    """Defines the root key of a TUF repo.

    Args:
      version: The key's version.
      type: The key's type.
      value: The value of the public key.
      signing_threshold: The signing threshold for the key.

    Returns:
      A struct defining the root key of the repo, for use with `tuf_repo()`.
    """
    if not version:
        fail("'version' cannot be None")
    if not type:
        fail("'version' cannot be None")
    if not value:
        fail("'value' cannot be None")
    if not signing_threshold:
        fail("'signing_threshold' cannot be None")

    return struct(
        version = version,
        key = struct(
            type = type,
            value = value,
        ),
        signing_threshold = signing_threshold,
    )

def _fuchsia_product_ota_config_impl(ctx):
    return [
        FuchsiaOmahaOtaConfigInfo(
            channels = ctx.attr.channels,
            tuf_repositories = ctx.attr.tuf_repositories,
        ),
    ]

_fuchsia_product_ota_config = rule(
    doc = """Define the OTA configs for a product to use.""",
    implementation = _fuchsia_product_ota_config_impl,
    provides = [FuchsiaOmahaOtaConfigInfo],
    attrs = {
        "channels": attr.string(
            doc = "Raw json of the channel configuration.",
        ),
        "tuf_repositories": attr.string_dict(
            doc = "Dict of raw json of the tuf repo configurations, by hostname.",
        ),
    },
)

def fuchsia_product_ota_config(
        realms,
        default_channel = None,
        **kwarg):
    """Produce the configuration files needed by a product which is using the omaha-client for updates.

    Produce the omaha-client channel configuration and TUf repo configuration
    from the product name and set of realms.

    Args:
     realms: list of realms created by realm().
     default_channel: A default channel, or None.
     **kwarg: The usual.
    """

    all_channels = []

    for realm in realms:
        for channel in realm.channels:
            if channel in all_channels:
                fail("channel %s is specified more than once. All channels under an OTA config must be unique." % channel)
            all_channels.append(channel)

    # If a default channel is defined, then validate that it's a channel from one
    # of the realms.
    if default_channel:
        if not default_channel in all_channels:
            fail("%s is not the name of an Omaha channel in any realm for this product" % default_channel)

    # Create the channel info for each channel, which joins the channel name
    # with the app_id of the realm that it's from.
    channel_infos = []
    for realm in realms:
        for channel in realm.channels:
            channel_info = {
                "name": channel,
                "repo": channel,
                "appid": realm.app_id,
            }
            if realm.check_interval:
                channel_info["check_interval_secs"] = realm.check_interval
            channel_infos.append(channel_info)

    # Create the actual channel configuration structure that's passed to assembly
    channel_config = {
        "version": "1",
        "content": {
            "channels": channel_infos,
        },
    }

    # Set the default channel, if there is one
    if default_channel:
        channel_config["content"]["default_channel"] = default_channel

    # Encode the channel configuration as raw json, so it can be passed through
    # the provider interface.
    channel_content = json.encode_indent(channel_config, indent = "  ")

    # Create the tuf configuration file contents, as a dict of raw json for each
    # repo, keyed by the hostname of the repo.
    tuf_repositories = {}
    for realm in realms:
        tuf = realm.tuf
        for hostname in tuf.hostnames:
            if hostname in tuf_repositories:
                fail("duplicate hostname found in realms: %s" % hostname)

            tuf_repositories[hostname] = json.encode_indent(struct(
                version = "1",
                content = [struct(
                    repo_url = "fuchsia-pkg://%s" % hostname,
                    root_version = tuf.root.version,
                    root_threshold = tuf.root.signing_threshold,
                    root_keys = [tuf.root.key],
                    mirrors = [struct(
                        mirror_url = "https://%s" % hostname,
                        subscribe = False,
                        blob_mirror_url = tuf.mirror_url,
                    )],
                )],
            ), indent = "  ")

    _fuchsia_product_ota_config(
        channels = channel_content,
        tuf_repositories = tuf_repositories,
        **kwarg
    )
