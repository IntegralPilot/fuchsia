// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log::LogClient;
use anyhow::Error;

const LOGO_TEXT: &str = "
\r
                                      ff    ff  ff\r
                                   ff  fffffffffff ff\r
                                  f ffffffffffffffff f\r
                                ff ffffffffffffffffffff\r
                                f fffffffff        ffff\r
                               f ffffffff            ff\r
                               f fffffff              f\r
                              ff fffffff\r
                              f  ffffff\r
                               fffffffff             f\r
                        fffffff                    fff\r
                    ffffffffffffffffffffffff   ffffff\r
                 ffffffffffffffffffffffffffffffffff\r
                ffffff   fffffff         ffffff\r
               fffff  fff      ffffffffff\r
              fffff ff         fffffff  f\r
              ffff fff         fffffff ff\r
             fffff ff         ffffffff f\r
              ffff fff       ffffffff  f\r
              ffff ffffffffffffffffff f\r
               ffff fffffffffffffff  f\r
                fffff  ffffffffff  ff\r
                  fffff         fff\r
                    fffffffffffff\r
";

pub struct Logo;
impl Logo {
    pub fn start<T: LogClient>(client: &T, id: u32) -> Result<(), Error>
where {
        let client = client.clone();
        let terminal =
            client.create_terminal(id, "logo".to_string()).expect("failed to create terminal");
        let term = terminal.clone_term();

        let mut parser = term.borrow_mut();
        parser.process(LOGO_TEXT.as_bytes());
        client.request_update(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::ColorScheme;
    use crate::terminal::Terminal;
    use fuchsia_async as fasync;

    #[derive(Default, Clone)]
    struct TestLogClient;

    impl LogClient for TestLogClient {
        fn create_terminal(&self, _id: u32, title: String) -> Result<Terminal, Error> {
            Ok(Terminal::new(title, ColorScheme::default(), 1024, None))
        }
        fn request_update(&self, _id: u32) {}
    }

    #[fasync::run_singlethreaded(test)]
    async fn can_start_logo() -> Result<(), Error> {
        let client = TestLogClient::default();
        let _ = Logo::start(&client, 0)?;
        Ok(())
    }
}
