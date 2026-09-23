// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::problems::ProblemPattern;
use super::problems::StringPattern::*;
use super::test::*;

#[test]
fn protocol_openness() {
    assert!(compare_fidl_protocol(
        "Foo",
        "
    @available(replaced=2)
    @discoverable
    closed protocol Foo {
        strict OneWay();
        strict TwoWay() -> ();
        strict -> Event();
    };

    @available(added=2)
    @discoverable
    open protocol Foo {
        strict OneWay();
        strict TwoWay() -> ();
        strict -> Event();
    };
    "
    )
    .is_compatible());

    assert!(compare_fidl_protocol(
        "Foo",
        "
    @available(replaced=2)
    @discoverable
    ajar protocol Foo {
        strict OneWay();
        strict TwoWay() -> ();
        strict -> Event();
    };

    @available(added=2)
    @discoverable
    open protocol Foo {
        strict OneWay();
        strict TwoWay() -> ();
        strict -> Event();
    };
    "
    )
    .is_compatible());

    assert!(compare_fidl_protocol(
        "Foo",
        "
    @available(replaced=2)
    @discoverable
    closed protocol Foo {
        strict OneWay();
        strict TwoWay() -> ();
        strict -> Event();
    };

    @available(added=2)
    @discoverable
    ajar protocol Foo {
        strict OneWay();
        strict TwoWay() -> ();
        strict -> Event();
    };
    "
    )
    .is_compatible());
}

mod method {
    use super::*;

    /// Helper to generate protocols with changing methods.
    fn protocol(discoverable: &str, change: &str, openness: &str, flexibility: &str) -> String {
        let discoverable = if discoverable == "" {
            "@discoverable".to_string()
        } else {
            format!("@discoverable({discoverable})")
        };

        "
        DISCOVERABLE
        OPENNESS protocol Foo {
            strict Bar() -> ();
            @available(CHANGE)
            FLEXIBILITY Baz() -> ();
        };"
        .replace("DISCOVERABLE", &discoverable)
        .replace("OPENNESS", openness)
        .replace("CHANGE", change)
        .replace("FLEXIBILITY", flexibility)
    }

    #[test]
    fn server_platform_add_remove_method() {
        // With server=platform it's safe to add and remove methods because the
        // platform will support the superset of all stable and unstable levels'
        // methods.
        for change in vec!["added=2", "added=HEAD", "removed=2", "removed=HEAD"] {
            for (openness, flexibility) in
                vec![("closed", "strict"), ("open", "strict"), ("open", "flexible")]
            {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol("server=\"platform\"", change, openness, flexibility)
                )
                .is_compatible());
            }
        }
    }

    #[test]
    fn server_external_add_method() {
        let changes = vec!["added=2", "added=HEAD"];
        let discoverable = "server=\"external\"";
        // For strict methods this should fail.
        for change in &changes {
            for (openness, flexibility) in vec![("closed", "strict"), ("open", "strict")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .has_problems(vec![
                    ProblemPattern::error().message(Begins("Server(@1) missing method"))
                ]));
            }
        }
        // For flexible methods this should succeed.
        for change in changes {
            for (openness, flexibility) in vec![("open", "flexible")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .is_compatible());
            }
        }
    }

    #[test]
    fn remove_method() {
        // It's always safe to remove methods because the platform will support the
        // superset of all stable and unstable levels' methods.

        let discoverables = ["", "server=\"platform\"", "server=\"external\""];

        for discoverable in discoverables {
            for change in vec!["removed=2", "removed=HEAD"] {
                for (openness, flexibility) in
                    vec![("closed", "strict"), ("open", "strict"), ("open", "flexible")]
                {
                    assert!(compare_fidl_library(
                        ["1", "1,2,NEXT,HEAD"],
                        protocol(discoverable, change, openness, flexibility)
                    )
                    .is_compatible());
                }
            }
        }
    }

    #[test]
    fn server_anywhere_add_method() {
        let changes = vec!["added=2", "added=HEAD"];
        let discoverable = "name=\"fuchsia.example.Foo\"";

        // For strict methods this should fail.
        for change in &changes {
            for (openness, flexibility) in vec![("closed", "strict"), ("open", "strict")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .has_problems(vec![
                    ProblemPattern::error().message(Begins("Server(@1) missing method"))
                ]));
            }
        }
        // For flexible methods this should succeed.
        for change in changes {
            for (openness, flexibility) in vec![("open", "flexible")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .is_compatible());
            }
        }
    }
}

mod event {
    use super::*;

    /// Helper to generate protocols with changing events.
    fn protocol(discoverable: &str, change: &str, openness: &str, flexibility: &str) -> String {
        let discoverable = if discoverable == "" {
            "@discoverable".to_string()
        } else {
            format!("@discoverable({discoverable})")
        };

        "
        DISCOVERABLE
        OPENNESS protocol Foo {
            strict Bar() -> ();
            @available(CHANGE)
            FLEXIBILITY -> Baz();
        };"
        .replace("DISCOVERABLE", &discoverable)
        .replace("OPENNESS", openness)
        .replace("CHANGE", change)
        .replace("FLEXIBILITY", flexibility)
    }

    #[test]
    fn server_external_add_remove_event() {
        // With server=external it's safe to add and remove protocols because the
        // platform will support the superset of all stable and unstable levels'
        // protocols.
        for change in vec!["added=2", "added=HEAD", "removed=2", "removed=HEAD"] {
            for (openness, flexibility) in
                vec![("closed", "strict"), ("open", "strict"), ("open", "flexible")]
            {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol("server=\"external\"", change, openness, flexibility)
                )
                .is_compatible());
            }
        }
    }

    #[test]
    fn server_platform_add_event() {
        let changes = vec!["added=2", "added=HEAD"];
        let discoverable = "server=\"platform\"";
        // For strict events this should fail.
        for change in &changes {
            for (openness, flexibility) in vec![("closed", "strict"), ("open", "strict")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .has_problems(vec![
                    ProblemPattern::error().message(Begins("Client(@1) missing event"))
                ]));
            }
        }
        // For flexible events this should succeed.
        for change in changes {
            for (openness, flexibility) in vec![("open", "flexible")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .is_compatible());
            }
        }
    }

    #[test]
    fn remove_event() {
        // It's safe to remove events because the platform will support the
        // superset of all stable and unstable levels' events.

        let discoverables = ["", "server=\"platform\"", "server=\"external\""];

        for discoverable in discoverables {
            for change in vec!["removed=2", "removed=HEAD"] {
                for (openness, flexibility) in
                    vec![("closed", "strict"), ("open", "strict"), ("open", "flexible")]
                {
                    assert!(compare_fidl_library(
                        ["1", "1,2,NEXT,HEAD"],
                        protocol(discoverable, change, openness, flexibility)
                    )
                    .is_compatible());
                }
            }
        }
    }

    #[test]
    fn client_anywhere_add_event() {
        let changes = vec!["added=2", "added=HEAD"];
        let discoverable = "name=\"fuchsia.example.Foo\"";

        // For strict clients this should fail.
        for change in &changes {
            for (openness, flexibility) in vec![("closed", "strict"), ("open", "strict")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .has_problems(vec![
                    ProblemPattern::error().message(Begins("Client(@1) missing event"))
                ]));
            }
        }
        // For flexible clients this should succeed.
        for change in changes {
            for (openness, flexibility) in vec![("open", "flexible")] {
                assert!(compare_fidl_library(
                    ["1", "1,2,NEXT,HEAD"],
                    protocol(discoverable, change, openness, flexibility)
                )
                .is_compatible());
            }
        }
    }
}

mod protocol {
    use super::*;

    #[test]
    fn add_protocol() {
        assert!(compare_fidl_library(
            ["1", "1,2,NEXT,HEAD"],
            r#"
            @discoverable
            @available(added=2)
            protocol Example {};
            "#
        )
        .is_compatible());
    }
}

#[test]
fn tear_off() {
    let error_message = "Server(@1) missing method fuchsia.compat.test/TearOff.Added";
    assert!(compare_fidl_library(
        ["1", "1,2,NEXT,HEAD"],
        r#"
            // Server @1 will be incompatible with clients @PLATFORM
            closed protocol TearOff {
                @available(added=2)
                strict Added();
            };

            @discoverable(server="external")
            protocol ServerExternal {
                // Ok - TearOff server in ServerExternal client.
                ClientEndInRequest(resource struct { t client_end:TearOff; });
                // Error - TearOff server in ServerExternal server.
                ClientEndInResponse() -> (resource struct { t client_end:TearOff; });
                // Error - TearOff server in ServerExternal server.
                ServerEndInRequest(resource struct { t server_end:TearOff; });
                // Ok - TearOff server in ServerExternal client.
                ServerEndInResponse() -> (resource struct { t server_end:TearOff; });
            };

            @discoverable(server="platform")
            protocol ServerPlatform {
                // Error - TearOff server in ServerPlatform server.
                ClientEndInRequest(resource struct { t client_end:TearOff; });
                // Ok - TearOff server in ServerPlatform client.
                ClientEndInResponse() -> (resource struct { t client_end:TearOff; });
                // Ok - TearOff server in ServerPlatform client.
                ServerEndInRequest(resource struct { t server_end:TearOff; });
                // Error - TearOff server in ServerPlatform server.
                ServerEndInResponse() -> (resource struct { t server_end:TearOff; });
            };
            "#
    )
    .has_problems(vec![
        // ServerExternal.ClientEndInResponse
        ProblemPattern::error()
            .path(Contains("ServerExternal.ClientEndInResponse"))
            .message(error_message),
        ProblemPattern::error()
            .path(Contains("ServerExternal.ClientEndInResponse"))
            .message(Contains("Incompatible response")),
        // ServerExternal.ServerEndInRequest
        ProblemPattern::error()
            .path(Contains("ServerExternal.ServerEndInRequest"))
            .message(error_message),
        ProblemPattern::error()
            .path(Contains("ServerExternal.ServerEndInRequest"))
            .message(Contains("Incompatible request")),
        // ServerPlatform.ClientEndInRequest
        ProblemPattern::error()
            .path(Contains("ServerPlatform.ClientEndInRequest"))
            .message(error_message),
        ProblemPattern::error()
            .path(Contains("ServerPlatform.ClientEndInRequest"))
            .message(Contains("Incompatible request")),
        // ServerPlatform.ServerEndInResponse
        ProblemPattern::error()
            .path(Contains("ServerPlatform.ServerEndInResponse"))
            .message(error_message),
        ProblemPattern::error()
            .path(Contains("ServerPlatform.ServerEndInResponse"))
            .message(Contains("Incompatible response")),
    ]));
}

#[test]
fn discoverable_contradiction() {
    assert!(compare_fidl_library(
        ["1", "1,2,NEXT,HEAD"],
        r#"
        @discoverable(client="external", server="platform")
        protocol ExternalClient {};

        @discoverable(client="platform", server="external")
        protocol ExternalServer {
            TakeServerEnd(resource struct{endpoint server_end:ExternalClient;}); // Bad
            TakeClientEnd(resource struct{endpoint client_end:ExternalClient;});
            ReturnServerEnd() -> (resource struct{endpoint server_end:ExternalClient;});
            ReturnClientEnd() -> (resource struct{endpoint client_end:ExternalClient;}); // Bad
        };
        "#
    )
    .has_problems(vec![
        ProblemPattern::warning()
            .path(Contains("ExternalServer.TakeServerEnd"))
            .message(Contains("ExternalClient(@1) used as a external server")),
        ProblemPattern::warning()
            .path(Contains("ExternalServer.ReturnClientEnd"))
            .message(Contains("ExternalClient(@1) used as a external server")),
        ProblemPattern::warning()
            .path(Contains("ExternalServer.TakeServerEnd"))
            .message(Contains("ExternalClient(@1,2,NEXT,HEAD) used as a platform client")),
        ProblemPattern::warning()
            .path(Contains("ExternalServer.ReturnClientEnd"))
            .message(Contains("ExternalClient(@1,2,NEXT,HEAD) used as a platform client")),
    ]));
}
