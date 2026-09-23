// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{ResponderExt, SendResultExt, WithName};

use test_case::test_case;

// The following type definitions emulate the definition of FIDL requests and responder types.
// In addition to testing the types defined in this crate, these tests demonstrate how those
// types would be used in the context of a FIDL request.
//
// As such, the `Request` type is superfluous but provides a meaningful example for the reader.
enum Request {
    Ax { payload: RequestAx, responder: RequestAxResponder },
    Bx { payload: RequestBx, responder: RequestBxResponder },
    Cx { payload: RequestCx, responder: RequestCxResponder },
    Dx { payload: RequestDx, responder: RequestDxResponder },
    Ex { responder: RequestExResponder },
    Fx { responder: RequestFxResponder },
}

#[derive(Debug)]
struct RequestAx {
    x: Option<()>,
}

#[derive(Debug)]
struct RequestAxResponder {}
impl RequestAxResponder {
    fn send(self) -> Result<(), fidl::Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct RequestBx {
    x: Option<String>,
    y: Option<u64>,
}

#[derive(Debug)]
struct RequestBxResponder {}
impl RequestBxResponder {
    fn send(self) -> Result<(), fidl::Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct RequestCx {
    x: Option<i32>,
}

#[derive(Debug)]
struct RequestCxResponder {}
impl RequestCxResponder {
    fn send(self, _result: Result<u64, u64>) -> Result<(), fidl::Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct RequestDx {
    x: Option<Option<()>>,
    y: Option<bool>,
}

#[derive(Debug)]
struct RequestDxResponder {}
impl RequestDxResponder {
    fn send(self, _result: Result<(), zx::Status>) -> Result<(), fidl::Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct RequestExResponder {}
impl RequestExResponder {
    fn send(self) -> Result<(), fidl::Error> {
        Err(fidl::Error::InvalidHeader)
    }
}

#[derive(Debug)]
struct RequestFxResponder {}
impl RequestFxResponder {
    fn send(self, _result: Result<(), zx::Status>) -> Result<(), fidl::Error> {
        Err(fidl::Error::InvalidHeader)
    }
}

impl ResponderExt for RequestAxResponder {
    type Response<'a> = ();
    const REQUEST_NAME: &'static str = stringify!(RequestAx);

    fn send<'a>(self, _: Self::Response<'a>) -> Result<(), fidl::Error> {
        Self::send(self)
    }
}

impl ResponderExt for RequestBxResponder {
    type Response<'a> = ();
    const REQUEST_NAME: &'static str = stringify!(RequestBx);

    fn send<'a>(self, _: Self::Response<'a>) -> Result<(), fidl::Error> {
        Self::send(self)
    }
}

impl ResponderExt for RequestCxResponder {
    type Response<'a> = Result<u64, u64>;
    const REQUEST_NAME: &'static str = stringify!(RequestCx);

    fn send<'a>(self, response: Self::Response<'a>) -> Result<(), fidl::Error> {
        Self::send(self, response)
    }
}

impl ResponderExt for RequestDxResponder {
    type Response<'a> = Result<(), zx::Status>;
    const REQUEST_NAME: &'static str = stringify!(RequestDx);

    fn send<'a>(self, response: Self::Response<'a>) -> Result<(), fidl::Error> {
        Self::send(self, response)
    }
}

impl ResponderExt for RequestExResponder {
    type Response<'a> = ();
    const REQUEST_NAME: &'static str = stringify!(RequestEx);

    fn send<'a>(self, _: Self::Response<'a>) -> Result<(), fidl::Error> {
        Self::send(self)
    }
}

impl ResponderExt for RequestFxResponder {
    type Response<'a> = Result<(), zx::Status>;
    const REQUEST_NAME: &'static str = stringify!(RequestFx);

    fn send<'a>(self, response: Self::Response<'a>) -> Result<(), fidl::Error> {
        Self::send(self, response)
    }
}

#[test]
fn responder_send_error_without_context_compiles() {
    if let Request::Ax { responder, .. } =
        (Request::Ax { payload: RequestAx { x: Some(()) }, responder: RequestAxResponder {} })
    {
        let _: Result<(), anyhow::Error> = ResponderExt::send(responder, ()).format_send_err();
    } else {
        panic!("Failed to match request");
    }
}

#[test]
fn responder_send_error_with_context_compiles() {
    if let Request::Cx { responder, .. } =
        (Request::Cx { payload: RequestCx { x: Some(5) }, responder: RequestCxResponder {} })
    {
        let _: Result<(), anyhow::Error> =
            ResponderExt::send(responder, Ok(10)).format_send_err_with_context("");
    }
}

#[test]
fn responder_send_error_without_type_fails_to_send() {
    if let Request::Ex { responder } = (Request::Ex { responder: RequestExResponder {} }) {
        let e = ResponderExt::send(responder, ())
            .format_send_err()
            .expect_err("Unexpected success upon send().");
        assert!(e.to_string().contains("Failed to respond: "));
    }
}

#[test]
fn responder_send_error_with_context_fails_to_send() {
    if let Request::Fx { responder } = (Request::Fx { responder: RequestFxResponder {} }) {
        let e = ResponderExt::send(responder, Err(zx::Status::INVALID_ARGS))
            .format_send_err_with_context("foo")
            .expect_err("Unexpected success upon send().");
        assert!(e.to_string().contains("Failed to respond (foo): "));
    }
}

macro_rules! assert_debug_fmt_contains {
            ($s:expr, $($substr:expr),+ $(,)?) => {
                let s = format!("{:?}", $s);
                $(
                    assert!(s.contains($substr), "String is \"{}\"", s);
                )+
            };
        }

#[test_case(Some(()))]
#[test_case(None)]
fn unpack_single_annotated_field(x: Option<()>) {
    if let Request::Ax { payload, responder } =
        (Request::Ax { payload: RequestAx { x }, responder: RequestAxResponder {} })
    {
        let result = responder.unpack_fields_or_respond(payload.x.with_name("x"));
        if x.is_some() {
            let (_x, _responder) = result.expect("Failed to unpack x");
        } else {
            assert_debug_fmt_contains!(
                result.expect_err("Took x when it wasn't present"),
                "Missing required field",
                "RequestAx",
                ": x",
            );
        }
    } else {
        panic!("Failed to match request");
    }
}

#[test_case(Some(format!("")), Some(754))]
#[test_case(Some(format!("")), None)]
#[test_case(None, Some(754))]
#[test_case(None, None)]
fn unpack_multiple_annotated_fields(x: Option<String>, y: Option<u64>) {
    if let Request::Bx { payload, responder } =
        (Request::Bx { payload: RequestBx { x: x.clone(), y }, responder: RequestBxResponder {} })
    {
        let result = responder
            .unpack_fields_or_respond((payload.x.with_name("x"), payload.y.with_name("y")));
        match (x, y) {
            (None, None) => {
                assert_debug_fmt_contains!(
                    result.expect_err("Took x when it wasn't present"),
                    "Missing required field",
                    "RequestBx",
                    ": x, y",
                );
            }
            (None, Some(_)) => {
                assert_debug_fmt_contains!(
                    result.expect_err("Took x when it wasn't present"),
                    "Missing required field",
                    "RequestBx",
                    ": x",
                );
            }
            (Some(_), None) => {
                assert_debug_fmt_contains!(
                    result.expect_err("Took y when it wasn't present"),
                    "Missing required field",
                    "RequestBx",
                    ": y",
                );
            }
            (Some(_), Some(_)) => {
                result.expect("Failed to unpack x and y");
            }
        }
    } else {
        panic!("Failed to match request");
    }
}

#[test_case(Some(2))]
#[test_case(None)]
fn unpack_single_annotated_field_with_error(x: Option<i32>) {
    if let Request::Cx { payload, responder } =
        (Request::Cx { payload: RequestCx { x }, responder: RequestCxResponder {} })
    {
        let mut closure_ran = false;
        let closure_ran_ref = &mut closure_ran;
        let result = responder.unpack_fields_or_else_send(payload.x.with_name("x"), || {
            *closure_ran_ref = true;
            Err(12)
        });
        if x.is_some() {
            let (x_ret, _responder) = result.expect("Failed to unpack x");
            assert!(!closure_ran);
            assert_eq!(x_ret, x.unwrap());
        } else {
            assert!(closure_ran);
            assert_debug_fmt_contains!(
                result.expect_err("Took x when it wasn't present"),
                "Missing required field",
                "RequestCx",
                ": x",
            );
        }
    }
}

#[test_case(Some(None), Some(true))]
#[test_case(Some(None), None)]
#[test_case(None, Some(true))]
#[test_case(None, None)]
fn unpack_multiple_annotated_fields_with_error(x: Option<Option<()>>, y: Option<bool>) {
    if let Request::Dx { payload, responder } =
        (Request::Dx { payload: RequestDx { x: x.clone(), y }, responder: RequestDxResponder {} })
    {
        let result = responder.unpack_fields_or_else_send(
            (payload.x.with_name("x"), payload.y.with_name("y")),
            || Err(zx::Status::INVALID_ARGS),
        );
        match (x, y) {
            (None, None) => {
                assert_debug_fmt_contains!(
                    result.expect_err("Took x and y when they weren't present"),
                    "Missing required field",
                    "RequestDx",
                    ": x, y",
                );
            }
            (None, Some(_)) => {
                assert_debug_fmt_contains!(
                    result.expect_err("Took x when it wasn't present"),
                    "Missing required field",
                    "RequestDx",
                    ": x",
                );
            }
            (Some(_), None) => {
                assert_debug_fmt_contains!(
                    result.expect_err("Took y when it wasn't present"),
                    "Missing required field",
                    "RequestDx",
                    ": y",
                );
            }
            (Some(_), Some(_)) => {
                let ((_x, y_ret), _responder) = result.expect("Failed to unpack x and y");
                assert_eq!(y_ret, y.unwrap());
            }
        }
    }
}

#[test_case(Some(None), Some(true))]
#[test_case(Some(None), None)]
#[test_case(None, Some(true))]
#[test_case(None, None)]
fn unpack_multiple_annotated_fields_over_separate_calls(x: Option<Option<()>>, y: Option<bool>) {
    if let Request::Dx { payload, responder } =
        (Request::Dx { payload: RequestDx { x: x.clone(), y }, responder: RequestDxResponder {} })
    {
        let mut closure_ran = false;
        let closure_ran_ref = &mut closure_ran;
        let result = responder.unpack_fields_or_else_send(payload.x.with_name("x"), || {
            *closure_ran_ref = true;
            Err(zx::Status::INVALID_ARGS)
        });
        if x.is_none() {
            assert_debug_fmt_contains!(
                result.expect_err("Took x when it wasn't present"),
                "Missing required field",
                "RequestDx",
                ": x",
            );
            assert!(closure_ran);
            return;
        }
        let (_x, responder): (Option<()>, RequestDxResponder) = result.expect("Failed to unpack x");
        assert!(!closure_ran);

        let closure_ran_ref = &mut closure_ran;
        let result = responder.unpack_fields_or_else_send(payload.y.with_name("y"), || {
            *closure_ran_ref = true;
            Err(zx::Status::INVALID_ARGS)
        });
        if y.is_none() {
            assert_debug_fmt_contains!(
                result.expect_err("Took y when it wasn't present"),
                "Missing required field",
                "RequestDx",
                ": y",
            );
            assert!(closure_ran);
            return;
        }
        let (y_ret, _responder) = result.expect("Failed to unpack y");
        assert!(!closure_ran);
        assert_eq!(y_ret, y.unwrap());
    }
}
