// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::Parameter::Parameter;
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::ReturnOrigin::ReturnOrigin;
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::ParameterSet::ParameterSet;
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::OpResult::OpResult;
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::Value::Value;
use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::Buffer::Buffer;
    use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::Direction::Direction;
use binder;
use fidl;
use std::ffi::CString;

// in_params_to_fidl converts a parameter set specified in AIDL to a FIDL representation for use as
// input to a TA operation.
pub fn in_params_to_fidl(params: &ParameterSet) -> fidl_fuchsia_tee::ParameterSet {
    let mut fidl_params = Vec::with_capacity(4);
    for i in 0..4 {
        fidl_params.push(param_to_fidl(&params.params[i]));
    }
    fidl_params
}

// op_results_to_aidl converts the results of an operation to the AIDL representation including
// any necessary parameters. Parameters with a direction of 'input' are omitted in the results.
pub fn op_result_to_aidl(fidl_op_result: fidl_fuchsia_tee::OpResult) -> OpResult {
    OpResult {
        returnCode: fidl_op_result.return_code.unwrap() as i64,
        returnOrigin: return_origin_to_aidl(fidl_op_result.return_origin.unwrap()),
        params: out_params_to_aidl(&fidl_op_result.parameter_set),
    }
}

fn out_params_to_aidl(out_fidl_params: &Option<fidl_fuchsia_tee::ParameterSet>) -> ParameterSet {
    let mut out_params = ParameterSet::default();
    if let Some(out_fidl_params) = out_fidl_params {
        for i in 0..out_fidl_params.len() {
            if parameter_is_output_or_inout(&out_fidl_params[i]) {
                out_params.params[i] = param_to_aidl(&out_fidl_params[i]);
            }
        }
    }
    out_params
}

pub fn error_to_binder_status(error: fidl::Error) -> binder::Status {
    // TODO: Investigate if there are other error types or statuses we should return here.
    // Also, in production mode consider if we want to provide any detail at over binder protocols
    // as this status crosses a trust domain.
    binder::Status::new_exception(
        binder::ExceptionCode::TRANSACTION_FAILED,
        Some(&CString::new(error.to_string()).unwrap()),
    )
}

fn return_origin_to_aidl(fidl_origin: fidl_fuchsia_tee::ReturnOrigin) -> ReturnOrigin {
    match fidl_origin {
        fidl_fuchsia_tee::ReturnOrigin::Communication => ReturnOrigin::COMMUNICATION,
        fidl_fuchsia_tee::ReturnOrigin::TrustedOs => ReturnOrigin::TRUSTED_OS,
        fidl_fuchsia_tee::ReturnOrigin::TrustedApplication => ReturnOrigin::TRUSTED_APPLICATION,
    }
}

fn direction_to_fidl(direction: Direction) -> fidl_fuchsia_tee::Direction {
    match direction {
        Direction::INPUT => fidl_fuchsia_tee::Direction::Input,
        Direction::INOUT => fidl_fuchsia_tee::Direction::Inout,
        Direction::OUTPUT => fidl_fuchsia_tee::Direction::Output,
        _ => panic!("unknown direction"),
    }
}

fn buffer_to_fidl(buffer: &Buffer) -> fidl_fuchsia_tee::Buffer {
    let vmo = if buffer.contents.is_empty() {
        None
    } else {
        let vmo = zx::Vmo::create(buffer.contents.len() as u64).unwrap();
        let _ = vmo.write(&buffer.contents, 0);
        Some(vmo)
    };
    fidl_fuchsia_tee::Buffer {
        direction: Some(direction_to_fidl(buffer.direction)),
        vmo,
        offset: Some(0),
        size: Some(buffer.contents.len() as u64),
        ..Default::default()
    }
}

fn value_to_fidl(value: &Value) -> fidl_fuchsia_tee::Value {
    fidl_fuchsia_tee::Value {
        direction: Some(direction_to_fidl(value.direction)),
        a: Some(value.a as u64),
        b: Some(value.b as u64),
        c: Some(value.c as u64),
        ..Default::default()
    }
}

fn param_to_fidl(param: &Parameter) -> fidl_fuchsia_tee::Parameter {
    use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::Parameter::Parameter::*;
    match param {
        Empty(_) => fidl_fuchsia_tee::Parameter::None(fidl_fuchsia_tee::None_),
        Buffer(buffer) => fidl_fuchsia_tee::Parameter::Buffer(buffer_to_fidl(buffer)),
        Value(value) => fidl_fuchsia_tee::Parameter::Value(value_to_fidl(value)),
    }
}

fn direction_to_aidl(direction: fidl_fuchsia_tee::Direction) -> Direction {
    match direction {
        fidl_fuchsia_tee::Direction::Input => Direction::INPUT,
        fidl_fuchsia_tee::Direction::Output => Direction::OUTPUT,
        fidl_fuchsia_tee::Direction::Inout => Direction::INOUT,
    }
}

fn direction_is_output_or_inout(direction: Option<fidl_fuchsia_tee::Direction>) -> bool {
    match direction {
        Some(fidl_fuchsia_tee::Direction::Inout) | Some(fidl_fuchsia_tee::Direction::Output) => {
            true
        }
        _ => false,
    }
}

fn parameter_is_output_or_inout(p: &fidl_fuchsia_tee::Parameter) -> bool {
    match p {
        fidl_fuchsia_tee::Parameter::None(_) => false,
        fidl_fuchsia_tee::Parameter::Buffer(buffer) => {
            direction_is_output_or_inout(buffer.direction)
        }
        fidl_fuchsia_tee::Parameter::Value(value) => direction_is_output_or_inout(value.direction),
        _ => false,
    }
}

fn buffer_to_aidl(buffer: &fidl_fuchsia_tee::Buffer) -> Buffer {
    let contents = match &buffer.vmo {
        Some(vmo) => vmo.read_to_vec::<u8>(buffer.offset.unwrap(), buffer.size.unwrap()).unwrap(),
        None => vec![],
    };
    Buffer { direction: direction_to_aidl(buffer.direction.unwrap()), contents }
}

fn value_to_aidl(value: &fidl_fuchsia_tee::Value) -> Value {
    Value {
        direction: direction_to_aidl(value.direction.unwrap()),
        a: value.a.unwrap_or(0) as i64,
        b: value.b.unwrap_or(0) as i64,
        c: value.c.unwrap_or(0) as i64,
    }
}

fn param_to_aidl(param: &fidl_fuchsia_tee::Parameter) -> Parameter {
    use android_system_microfuchsia_trusted_app::aidl::android::system::microfuchsia::trusted_app::Parameter::Parameter::*;
    match param {
        fidl_fuchsia_tee::Parameter::None(_) => Empty(false),
        fidl_fuchsia_tee::Parameter::Buffer(buffer) => Buffer(buffer_to_aidl(buffer)),
        fidl_fuchsia_tee::Parameter::Value(value) => Value(value_to_aidl(value)),
        _ => panic!("unknown parameter variant"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Buffer, Direction, Parameter, ReturnOrigin};

    use test_case::test_case;

    #[test_case(fidl_fuchsia_tee::ReturnOrigin::Communication, ReturnOrigin::COMMUNICATION)]
    #[test_case(fidl_fuchsia_tee::ReturnOrigin::TrustedOs, ReturnOrigin::TRUSTED_OS)]
    #[test_case(
        fidl_fuchsia_tee::ReturnOrigin::TrustedApplication,
        ReturnOrigin::TRUSTED_APPLICATION
    )]
    #[fuchsia::test]
    fn return_origin_to_aidl(fidl: fidl_fuchsia_tee::ReturnOrigin, aidl: ReturnOrigin) {
        assert_eq!(aidl, super::return_origin_to_aidl(fidl));
    }

    #[test_case(Direction::INPUT, fidl_fuchsia_tee::Direction::Input)]
    #[test_case(Direction::OUTPUT, fidl_fuchsia_tee::Direction::Output)]
    #[test_case(Direction::INOUT, fidl_fuchsia_tee::Direction::Inout)]
    #[fuchsia::test]
    fn direction_to_fidl(aidl: Direction, fidl: fidl_fuchsia_tee::Direction) {
        assert_eq!(fidl, super::direction_to_fidl(aidl));
    }

    #[fuchsia::test]
    fn empty_buffer_to_fidl() {
        let buffer = Buffer { direction: Direction::INPUT, contents: vec![] };
        let fidl_buffer = super::buffer_to_fidl(&buffer);
        assert_eq!(fidl_buffer.direction, Some(fidl_fuchsia_tee::Direction::Input));
        assert_eq!(fidl_buffer.vmo, None);
        assert_eq!(fidl_buffer.offset, Some(0));
        assert_eq!(fidl_buffer.size, Some(0));
    }

    #[fuchsia::test]
    fn input_buffer_to_fidl() {
        let buffer = Buffer { direction: Direction::INPUT, contents: vec![1, 2, 3, 4] };
        let fidl_buffer = super::buffer_to_fidl(&buffer);
        assert_eq!(fidl_buffer.direction, Some(fidl_fuchsia_tee::Direction::Input));
        assert_eq!(fidl_buffer.offset, Some(0));
        assert_eq!(fidl_buffer.size, Some(4));
        assert!(fidl_buffer.vmo.is_some());
        if let Some(vmo) = fidl_buffer.vmo {
            assert_eq!(vmo.get_content_size(), Ok(4));
            let read_data = vmo.read_to_vec::<u8>(0, 4).unwrap();
            assert_eq!(&read_data, &[1, 2, 3, 4]);
        }
    }

    #[fuchsia::test]
    fn short_out_params_to_aidl() {
        // A FIDL response may have fewer than 4 entries, test that we fill out the AIDL value
        // correctly.
        let out_fidl_params =
            Some(vec![fidl_fuchsia_tee::Parameter::Value(fidl_fuchsia_tee::Value {
                direction: Some(fidl_fuchsia_tee::Direction::Output),
                a: Some(1),
                b: None,
                c: None,
                ..Default::default()
            })]);

        let aidl_params = super::out_params_to_aidl(&out_fidl_params);

        // The first parameter should match the FIDL value.
        match &aidl_params.params[0] {
            Parameter::Value(value) => {
                assert_eq!(value.direction, Direction::OUTPUT);
                assert_eq!(value.a, 1);
                assert_eq!(value.b, 0);
                assert_eq!(value.c, 0);
            }
            _ => {
                assert!(false, "Expected params[0] to be a value type.");
            }
        }
        // The rest should be empty.

        match aidl_params.params[1] {
            Parameter::Empty(_) => {}
            _ => {
                assert!(false, "params[1] should be empty");
            }
        };
        match aidl_params.params[2] {
            Parameter::Empty(_) => {}
            _ => {
                assert!(false, "params[1] should be empty");
            }
        };
        match aidl_params.params[3] {
            Parameter::Empty(_) => {}
            _ => {
                assert!(false, "params[1] should be empty");
            }
        };
    }

    #[fuchsia::test]
    fn parameter_direction_to_aidl_conversions() {
        let fidl_op_result = fidl_fuchsia_tee::OpResult {
            return_code: Some(0),
            return_origin: Some(fidl_fuchsia_tee::ReturnOrigin::TrustedApplication),
            parameter_set: Some(vec![
                fidl_fuchsia_tee::Parameter::Buffer(fidl_fuchsia_tee::Buffer {
                    direction: Some(fidl_fuchsia_tee::Direction::Input),
                    ..Default::default()
                }),
                fidl_fuchsia_tee::Parameter::Buffer(fidl_fuchsia_tee::Buffer {
                    direction: Some(fidl_fuchsia_tee::Direction::Output),
                    ..Default::default()
                }),
                fidl_fuchsia_tee::Parameter::Buffer(fidl_fuchsia_tee::Buffer {
                    direction: Some(fidl_fuchsia_tee::Direction::Inout),
                    ..Default::default()
                }),
                fidl_fuchsia_tee::Parameter::Value(fidl_fuchsia_tee::Value {
                    direction: Some(fidl_fuchsia_tee::Direction::Output),
                    a: Some(1),
                    b: Some(2),
                    c: Some(3),
                    ..Default::default()
                }),
            ]),
            ..Default::default()
        };

        let op_result = super::op_result_to_aidl(fidl_op_result);

        // params[0] has direction INPUT so it should be empty on the way back out.
        match op_result.params.params[0] {
            Parameter::Empty(_) => {}
            _ => {
                assert!(false, "params[0] should be empty");
            }
        };

        // params[1] has direction OUTPUT so we should see a buffer on the way back out.
        match &op_result.params.params[1] {
            Parameter::Buffer(buffer) => {
                assert_eq!(buffer.direction, Direction::OUTPUT);
            }
            _ => {
                assert!(false, "params[1] should be a buffer");
            }
        }

        // params[2] has direction INOUT so we should see a buffer on the way back out.
        match &op_result.params.params[2] {
            Parameter::Buffer(buffer) => {
                assert_eq!(buffer.direction, Direction::INOUT);
            }
            _ => {
                assert!(false, "params[2] should be a buffer");
            }
        }

        // params[3] has direction OUTPUT so we should see a (populated) value struct on the way back out.
        match &op_result.params.params[3] {
            Parameter::Value(value) => {
                assert_eq!(value.direction, Direction::OUTPUT);
                assert_eq!(value.a, 1);
                assert_eq!(value.b, 2);
                assert_eq!(value.c, 3);
            }
            _ => {
                assert!(false, "params[3] should be a value");
            }
        }
    }
}
