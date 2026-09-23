// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;
use std::os::raw::c_int;

/// Methods from the [OpenThread "CLI" Module](https://openthread.io/reference/group/api-cli).
pub trait Cli {
    /// Functional equivalent of [`otsys::otCliInputLine`](crate::otsys::otCliInputLine).
    fn cli_input_line(&self, line: &CStr);

    /// Functional equivalent of [`otsys::otCliInit`](crate::otsys::otCliInit).
    fn cli_init<'a, F>(&self, output_callback: F)
    where
        F: FnMut(&CStr) + 'a;
}

impl<T: Cli + ot::Boxable> Cli for ot::Box<T> {
    fn cli_input_line(&self, line: &CStr) {
        self.as_ref().cli_input_line(line);
    }

    fn cli_init<'a, F>(&self, output_callback: F)
    where
        F: FnMut(&CStr) + 'a,
    {
        self.as_ref().cli_init(output_callback);
    }
}

impl Cli for Instance {
    fn cli_input_line(&self, line: &CStr) {
        unsafe { otCliInputLine(line.as_ptr() as *mut c_char) }
    }

    fn cli_init<'a, F>(&self, f: F)
    where
        F: FnMut(&CStr) + 'a,
    {
        unsafe extern "C" fn _ot_cli_output_callback(
            context: *mut ::std::os::raw::c_void,
            line: *const c_char,
            _: *mut otsys::__va_list_tag,
        ) -> c_int {
            let line = unsafe { CStr::from_ptr(line) };

            trace!("_ot_cli_output_callback: {:?}", line);

            // Reconstitute a reference to our instance
            let instance =
                unsafe { Instance::ref_from_ot_ptr(context as *mut otInstance) }.unwrap();

            // Get a reference to our instance backing.
            let backing = instance.borrow_backing();

            // Remove our line sender from the cell in the backing.
            // We will replace it once we are done.
            let mut sender_option = backing.cli_output_fn.replace(None);

            if let Some(sender) = sender_option.as_mut() {
                sender(line);
            } else {
                info!("_ot_cli_output_callback: Nothing to handle CLI output");
            }

            // Put the sender back into the backing so that we can
            // use it again later.
            if let Some(new_sender) = backing.cli_output_fn.replace(sender_option) {
                // In this case someone called the init function while
                // they were in the callback. We want to restore that
                // callback.
                backing.cli_output_fn.set(Some(new_sender));
            }

            line.to_bytes().len().try_into().unwrap()
        }

        // Put our sender closure in a box.
        // This will go into the backing.
        let fn_box = Box::new(f) as Box<dyn FnMut(&CStr) + 'a>;

        // Prepare a function pointer for our callback closure.
        let cb: otCliOutputCallback = Some(_ot_cli_output_callback);

        // Go ahead and get our context pointer ready for the callback.
        // In this case our context pointer is the `otInstance` pointer.
        let fn_ptr = self.as_ot_ptr() as *mut ::std::os::raw::c_void;

        unsafe {
            // Make sure our object eventually gets cleaned up.
            // Here we must also transmute our closure to have a 'static
            // lifetime.
            // SAFETY: We need to do this because the borrow checker
            //         cannot infer the proper lifetime for the
            //         singleton instance backing, but this is guaranteed
            //         by the API.
            let prev_callback =
                self.borrow_backing().cli_output_fn.replace(std::mem::transmute::<
                    Option<Box<dyn FnMut(&'_ CStr) + 'a>>,
                    Option<Box<dyn FnMut(&'_ CStr) + 'static>>,
                >(Some(fn_box)));

            // Check to see if there was a previous callback registered.
            // We only want to call `otCliInit()` if there was no
            // previously registered callback.
            if prev_callback.is_none() {
                // SAFETY: This must only be called once.
                otCliInit(self.as_ot_ptr(), cb, fn_ptr);
            }
        }
    }
}
