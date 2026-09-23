// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_config::EnvironmentContext;
const CONFIG_KEY_DEFAULT_REPOSITORY: &str = "repository.default";
const CONFIG_KEY_SERVER_LISTEN: &str = "repository.server.listen";

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryConfigError {
    #[error(
        "Server listening address is unspecified. You can fix this with:\n\
             $ ffx config set repository.server.listen '[::]:8083'\n\
             $ ffx repository server start\n\
             Or alternatively specify at runtime \n\
             $ ffx repository server start --address <addr>"
    )]
    AddressUnspecified,

    #[error(
        "ffx config detects repository.server.listen to be {0} \
             Another process may be using that address. \
             Try shutting it down \n\
             $ ffx repository server stop --all\n\
             Or alternatively specify a different address on the command line\n\
             $ ffx repository server start --address <addr>"
    )]
    AddressInUse(std::net::SocketAddr),

    #[error("Parsing {0}: {1}")]
    ParseAddressError(String, #[source] std::net::AddrParseError),

    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),
}

/// Default name used for package repositories in ffx. It is expected that there is no need to
/// change this constant. But in case this is changed, ensure that it is consistent with the ffx
/// developer documentation, see
/// https://cs.opensource.google/search?q=devhost&sq=&ss=fuchsia%2Ffuchsia:src%2Fdeveloper%2Fffx%2F
// LINT.IfChange
pub const DEFAULT_REPO_NAME: &str = "devhost";
// LINT.ThenChange(/src/developer/ffx/plugins/repository/add-from-pm/src/args.rs)

// Try to figure out why the server is not running.
pub fn determine_why_repository_server_is_not_running(
    context: &EnvironmentContext,
) -> RepositoryConfigError {
    macro_rules! check {
        ($e:expr) => {
            match $e {
                Ok(value) => value,
                Err(err) => {
                    return err;
                }
            }
        };
    }

    match check!(repository_listen_addr(context)) {
        Some(addr) => {
            return RepositoryConfigError::AddressInUse(addr);
        }
        None => {
            return RepositoryConfigError::AddressUnspecified;
        }
    }
}

/// Return the repository server address from ffx config.
pub fn repository_listen_addr(
    context: &EnvironmentContext,
) -> std::result::Result<Option<std::net::SocketAddr>, RepositoryConfigError> {
    if let Some(address) = context.get::<Option<String>, _>(CONFIG_KEY_SERVER_LISTEN)? {
        if address.is_empty() {
            Ok(None)
        } else {
            Ok(Some(address.parse::<std::net::SocketAddr>().map_err(|e| {
                RepositoryConfigError::ParseAddressError(CONFIG_KEY_SERVER_LISTEN.to_string(), e)
            })?))
        }
    } else {
        Ok(None)
    }
}

/// Return the default repository from the configuration if set.
pub fn get_default_repository(
    context: &EnvironmentContext,
) -> Result<Option<String>, RepositoryConfigError> {
    Ok(context.get(CONFIG_KEY_DEFAULT_REPOSITORY)?)
}
