// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use debug::{dprintf, ltracef};
use init::{LkInitFlags, LkInitLevel, LkInitStruct};

const LOCAL_TRACE: u32 = 0;

fn get_init_structs() -> &'static [LkInitStruct] {
    unsafe extern "C" {
        #[link_name = "__start_lk_init"]
        static START_LK_INIT: LkInitStruct;
        #[link_name = "__stop_lk_init"]
        static STOP_LK_INIT: LkInitStruct;
    }
    // SAFETY: These are defined by the linker script to bound the lk_init section.
    unsafe {
        let start = &START_LK_INIT as *const LkInitStruct;
        let stop = &STOP_LK_INIT as *const LkInitStruct;
        let count = (stop as usize - start as usize) / core::mem::size_of::<LkInitStruct>();
        core::slice::from_raw_parts(start, count)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lk_init_level(
    required_flag: LkInitFlags,
    start_level: LkInitLevel,
    stop_level: LkInitLevel,
) {
    ltracef!(
        "flags {:#x}, start_level {:#x}, stop_level {:#x}\n",
        required_flag as u32,
        start_level.0,
        stop_level.0
    );
    assert!(start_level.0 > 0);
    let mut last_called_level = LkInitLevel(start_level.0 - 1);
    let mut last: Option<*const LkInitStruct> = None;

    let init_structs = get_init_structs();

    loop {
        // Search for the lowest uncalled hook to call.
        ltracef!(
            "last {:p}, last_called_level {:#x}\n",
            last.unwrap_or_default(),
            last_called_level.0
        );

        let mut found: Option<&LkInitStruct> = None;
        let mut seen_last = false;

        for ptr in init_structs {
            ltracef!(
                "looking at {:p} {:?} level {:#x}, flags {:#x}, seen_last {}\n",
                ptr,
                unsafe { core::ffi::CStr::from_ptr(ptr.name) },
                ptr.level.0,
                ptr.flags as u32,
                seen_last
            );
            let ptr_addr = ptr as *const LkInitStruct;
            if Some(ptr_addr) == last {
                seen_last = true;
            }

            // Reject the easy ones.
            if ((ptr.flags as u32) & (required_flag as u32)) == 0 {
                continue;
            }
            if ptr.level > stop_level {
                continue;
            }
            if ptr.level < last_called_level {
                continue;
            }
            if let Some(found_ref) = found
                && found_ref.level <= ptr.level
            {
                continue;
            }

            // Keep the lowest one we haven't called yet.
            if ptr.level >= start_level && ptr.level > last_called_level {
                found = Some(ptr);
                continue;
            }

            // If we're at the same level as the last one we called and we've
            // already passed over it this time around, we can mark this one
            // and early terminate the loop.
            if ptr.level == last_called_level && Some(ptr_addr) != last && seen_last {
                found = Some(ptr);
                break;
            }
        }

        let Some(found_ref) = found else {
            break;
        };

        dprintf!(
            INFO,
            "INIT: cpu {}, calling hook {:?} {:?} at level {:#x}, flags {:#x}\n",
            crate::arch_rs::curr_cpu_num(),
            found_ref.hook,
            unsafe { core::ffi::CStr::from_ptr(found_ref.name) },
            found_ref.level.0,
            found_ref.flags as u32
        );
        (found_ref.hook)(found_ref.level);

        last_called_level = found_ref.level;
        last = Some(found_ref as *const LkInitStruct);
    }
}
