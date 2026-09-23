// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::{Counter, define_kcounter};
use crate::kernel::deadline::TimerSlack;
use zx_status::Status;
use zx_types::{
    ZX_JOB_POL_ABSOLUTE, ZX_POL_ACTION_ALLOW, ZX_POL_ACTION_DENY, ZX_POL_ACTION_DENY_EXCEPTION,
    ZX_POL_ACTION_KILL, ZX_POL_ACTION_MAX, ZX_POL_AMBIENT_MARK_VMO_EXEC, ZX_POL_BAD_HANDLE,
    ZX_POL_MAX, ZX_POL_NEW_ANY, ZX_POL_NEW_CHANNEL, ZX_POL_NEW_EVENT, ZX_POL_NEW_EVENTPAIR,
    ZX_POL_NEW_FIFO, ZX_POL_NEW_IOB, ZX_POL_NEW_PAGER, ZX_POL_NEW_PORT, ZX_POL_NEW_PROCESS,
    ZX_POL_NEW_PROFILE, ZX_POL_NEW_SAMPLER, ZX_POL_NEW_SOCKET, ZX_POL_NEW_TIMER, ZX_POL_NEW_VMO,
    ZX_POL_OVERRIDE_ALLOW, ZX_POL_OVERRIDE_DENY, ZX_POL_VMAR_WX, ZX_POL_WRONG_OBJECT,
    zx_policy_basic_v2_t,
};

/// Index of the last policy.
const JOB_POLICY_LAST_POLICY: usize = (ZX_POL_MAX - 1) as usize;
/// Number of bits per policy (an additional bit for policy override).
const JOB_POLICY_BITS_PER_POLICY: usize = 4;
/// Number of bits per storage bucket.
const JOB_POLICY_BITS_PER_BUCKET: usize = u64::BITS as usize;
/// Number of 64-bit storage buckets needed for job policies.
const JOB_POLICY_JOB_POLICY_BUCKETS: usize =
    (JOB_POLICY_LAST_POLICY * JOB_POLICY_BITS_PER_POLICY).div_ceil(JOB_POLICY_BITS_PER_BUCKET);

// Enforce the invariant of 4 bits per policy.
zr::static_assert!(JOB_POLICY_BITS_PER_POLICY == 4);
zr::static_assert!(JOB_POLICY_JOB_POLICY_BUCKETS == 2);
zr::static_assert!(ZX_POL_ACTION_ALLOW == 0 && ZX_POL_OVERRIDE_ALLOW == 0);

const POLICY_BIT_MASK: u64 = (1u64 << JOB_POLICY_BITS_PER_POLICY) - 1;
const OVERRIDE_MASK: u64 = 0x1;
const ACTION_MASK: u64 = POLICY_BIT_MASK & !OVERRIDE_MASK;

/// This struct provides the encoded bit fields of each policy, by performing proper
/// offsetting into the underlying storage bucket and bit range.
///
/// JobPolicyCollection objects are thread-compatible. In particular, const instances are immutable.
/// This guarantee is important because const instances of JobPolicy may be accessed concurrently by
/// multiple threads without any synchronization.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPolicyCollection {
    storage: [u64; JOB_POLICY_JOB_POLICY_BUCKETS],
}

zr::static_assert!(core::mem::size_of::<JobPolicyCollection>() == 16);
zr::static_assert!(core::mem::align_of::<JobPolicyCollection>() == 8);

impl JobPolicyCollection {
    /// Creates a new [`JobPolicyCollection`] with all entries initialized to zero.
    pub const fn new() -> Self {
        Self { storage: [0; JOB_POLICY_JOB_POLICY_BUCKETS] }
    }

    /// Retrieves the policy entry for the specified `policy`.
    pub fn get(&self, mut policy: usize) -> Policy<'_> {
        assert!(
            policy < ZX_POL_MAX as usize,
            "Attempting to retrieve policy entry for {policy} and max is {ZX_POL_MAX}"
        );

        // This is a synthetic policy, that doesn't require bits.
        assert!(policy != ZX_POL_NEW_ANY as usize);
        // `ZX_POL_NEW_ANY` is a synthetic policy in terms of required storage. Its job is to apply
        // the policy action and override to ALL new object policies.
        if policy > ZX_POL_NEW_ANY as usize {
            policy -= 1;
        }
        Policy::new(&self.storage, policy * JOB_POLICY_BITS_PER_POLICY)
    }

    /// Retrieves a mutable policy entry for the specified `policy`.
    fn get_mut(&mut self, mut policy: usize) -> PolicyMut<'_> {
        assert!(
            policy < ZX_POL_MAX as usize,
            "Attempting to retrieve policy entry for {policy} and max is {ZX_POL_MAX}"
        );

        // This is a synthetic policy, that doesn't require bits.
        assert!(policy != ZX_POL_NEW_ANY as usize);
        // `ZX_POL_NEW_ANY` is a synthetic policy in terms of required storage. Its job is to apply
        // the policy action and override to ALL new object policies.
        if policy > ZX_POL_NEW_ANY as usize {
            policy -= 1;
        }
        PolicyMut::new(&mut self.storage, policy * JOB_POLICY_BITS_PER_POLICY)
    }
}

impl Default for JobPolicyCollection {
    fn default() -> Self {
        Self::new()
    }
}

/// Individual policy bits (read-only view).
pub struct Policy<'a> {
    storage: &'a [u64; JOB_POLICY_JOB_POLICY_BUCKETS],
    policy_offset: usize,
}

impl<'a> Policy<'a> {
    const fn new(storage: &'a [u64; JOB_POLICY_JOB_POLICY_BUCKETS], policy_offset: usize) -> Self {
        Self { storage, policy_offset }
    }

    const fn policy_range(&self) -> (usize, usize) {
        (
            self.policy_offset / JOB_POLICY_BITS_PER_BUCKET,
            self.policy_offset % JOB_POLICY_BITS_PER_BUCKET,
        )
    }

    /// Returns the action for this policy.
    pub const fn action(&self) -> u32 {
        let (bucket, bit_offset) = self.policy_range();
        (((self.storage[bucket] >> bit_offset) & ACTION_MASK) >> 1) as u32
    }

    /// Returns whether this policy allows override.
    pub const fn r#override(&self) -> bool {
        let (bucket, bit_offset) = self.policy_range();
        // Bit is flipped such that default value is 0.
        !(((self.storage[bucket] >> bit_offset) & OVERRIDE_MASK) != 0)
    }
}

/// Individual policy bits (mutable view).
struct PolicyMut<'a> {
    storage: &'a mut [u64; JOB_POLICY_JOB_POLICY_BUCKETS],
    policy_offset: usize,
}

impl<'a> PolicyMut<'a> {
    fn new(storage: &'a mut [u64; JOB_POLICY_JOB_POLICY_BUCKETS], policy_offset: usize) -> Self {
        Self { storage, policy_offset }
    }

    const fn policy_range(&self) -> (usize, usize) {
        (
            self.policy_offset / JOB_POLICY_BITS_PER_BUCKET,
            self.policy_offset % JOB_POLICY_BITS_PER_BUCKET,
        )
    }

    /// Returns the action for this policy.
    const fn action(&self) -> u32 {
        let (bucket, bit_offset) = self.policy_range();
        (((self.storage[bucket] >> bit_offset) & ACTION_MASK) >> 1) as u32
    }

    /// Sets the action for this policy.
    fn set_action(&mut self, action: u32) {
        assert!(
            action < ZX_POL_ACTION_MAX,
            "Attempting to set policy entry({})'s action to {} and max is {}",
            self.policy_offset / JOB_POLICY_BITS_PER_POLICY,
            action,
            ZX_POL_ACTION_MAX
        );

        let (bucket, bit_offset) = self.policy_range();
        // `action` must be set to the three most significant bits of the 4 bit bitfield.
        self.storage[bucket] = (self.storage[bucket] & !(ACTION_MASK << bit_offset))
            | ((action as u64) << (bit_offset + 1));
    }

    /// Returns whether this policy allows override.
    const fn r#override(&self) -> bool {
        let (bucket, bit_offset) = self.policy_range();
        // Bit is flipped such that default value is 0.
        !(((self.storage[bucket] >> bit_offset) & OVERRIDE_MASK) != 0)
    }

    /// Sets whether this policy allows override.
    fn set_override(&mut self, r#override: bool) {
        // Override bit can either be allow(0) or deny(1), so the flag is flipped for transforming
        // into the proper bit flag.
        let (bucket, bit_offset) = self.policy_range();
        // Bit is flipped such that default value is 0.
        self.storage[bucket] = (self.storage[bucket] & !(OVERRIDE_MASK << bit_offset))
            | ((!r#override as u64) << bit_offset);
    }
}

// It is critical that this array contain all "new object" policies because it's used to implement
// ZX_NEW_ANY.
const NEW_OBJECT_POLICIES: [u32; 13] = [
    ZX_POL_NEW_VMO,
    ZX_POL_NEW_CHANNEL,
    ZX_POL_NEW_EVENT,
    ZX_POL_NEW_EVENTPAIR,
    ZX_POL_NEW_PORT,
    ZX_POL_NEW_SOCKET,
    ZX_POL_NEW_FIFO,
    ZX_POL_NEW_TIMER,
    ZX_POL_NEW_PROCESS,
    ZX_POL_NEW_PROFILE,
    ZX_POL_NEW_PAGER,
    ZX_POL_NEW_IOB,
    ZX_POL_NEW_SAMPLER,
];

zr::static_assert!(NEW_OBJECT_POLICIES.len() + 5 == ZX_POL_MAX as usize);

fn policy_override_is_valid(r#override: u32) -> bool {
    matches!(r#override, ZX_POL_OVERRIDE_DENY | ZX_POL_OVERRIDE_ALLOW)
}

fn add_partial(
    mode: u32,
    condition: u32,
    action: u32,
    r#override: u32,
    bits: &mut JobPolicyCollection,
) -> Result<(), Status> {
    if action >= ZX_POL_ACTION_MAX {
        return Err(Status::NOT_SUPPORTED);
    }

    if !policy_override_is_valid(r#override) {
        return Err(Status::INVALID_ARGS);
    }

    if condition >= ZX_POL_MAX || condition == ZX_POL_NEW_ANY {
        return Err(Status::INVALID_ARGS);
    }

    let override_bit = r#override == ZX_POL_OVERRIDE_ALLOW;
    let mut condition_bits = bits.get_mut(condition as usize);
    if condition_bits.r#override() {
        condition_bits.set_action(action);
        condition_bits.set_override(override_bit);
        return Ok(());
    }

    if condition_bits.action() == action && !override_bit {
        return Ok(());
    }

    if mode == ZX_JOB_POL_ABSOLUTE { Err(Status::ALREADY_EXISTS) } else { Ok(()) }
}

/// JobPolicy is a value type that provides a space-efficient encoding of the policies defined in
/// the policy.h public header.
///
/// JobPolicy encodes two kinds of policy, basic and timer slack.
///
/// Basic policy is logically an array of zx_policy_basic elements. For example:
///
/// ```text
///   zx_policy_basic policy[] = {
///      { ZX_POL_BAD_HANDLE, ZX_POL_ACTION_KILL },
///      { ZX_POL_NEW_CHANNEL, ZX_POL_ACTION_ALLOW },
///      { ZX_POL_NEW_FIFO, ZX_POL_ACTION_ALLOW_EXCEPTION },
///      { ZX_POL_VMAR_WX, ZX_POL_ACTION_KILL }}
/// ```
///
/// Timer slack policy defines the type and minimum amount of slack that will be applied to timer
/// and deadline events.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPolicy {
    // Remember, JobPolicy is a value type so think carefully before increasing its size.
    //
    // Const instances of JobPolicy must be immutable to ensure thread-safety.
    collection: JobPolicyCollection,
    slack: TimerSlack,
}

zr::static_assert!(core::mem::size_of::<JobPolicy>() == 32);
zr::static_assert!(core::mem::align_of::<JobPolicy>() == 8);

// Counts policy violations resulting in ZX_POL_ACTION_DENY or ZX_POL_ACTION_DENY_EXCEPTION.
define_kcounter!(POLICY_DENY_BAD_HANDLE, "policy.deny.bad_handle", Sum);
define_kcounter!(POLICY_DENY_WRONG_OBJECT, "policy.deny.wrong_object", Sum);
define_kcounter!(POLICY_DENY_VMAR_WX, "policy.deny.vmar_wx", Sum);
define_kcounter!(POLICY_DENY_NEW_VMO, "policy.deny.new_vmo", Sum);
define_kcounter!(POLICY_DENY_NEW_CHANNEL, "policy.deny.new_channel", Sum);
define_kcounter!(POLICY_DENY_NEW_EVENT, "policy.deny.new_event", Sum);
define_kcounter!(POLICY_DENY_NEW_EVENTPAIR, "policy.deny.new_eventpair", Sum);
define_kcounter!(POLICY_DENY_NEW_PORT, "policy.deny.new_port", Sum);
define_kcounter!(POLICY_DENY_NEW_SOCKET, "policy.deny.new_socket", Sum);
define_kcounter!(POLICY_DENY_NEW_FIFO, "policy.deny.new_fifo", Sum);
define_kcounter!(POLICY_DENY_NEW_TIMER, "policy.deny.new_timer", Sum);
define_kcounter!(POLICY_DENY_NEW_PROCESS, "policy.deny.new_process", Sum);
define_kcounter!(POLICY_DENY_NEW_PROFILE, "policy.deny.new_profile", Sum);
define_kcounter!(POLICY_DENY_NEW_PAGER, "policy.deny.new_pager", Sum);
define_kcounter!(POLICY_DENY_AMBIENT_MARK_VMO_EXEC, "policy.deny.ambient_mark_vmo_exec", Sum);
define_kcounter!(POLICY_DENY_NEW_IOB, "policy.deny.new_iob", Sum);
define_kcounter!(POLICY_DENY_NEW_SAMPLER, "policy.deny.new_sampler", Sum);

// Counts policy violations resulting in ZX_POL_ACTION_KILL.
define_kcounter!(POLICY_KILL_BAD_HANDLE, "policy.kill.bad_handle", Sum);
define_kcounter!(POLICY_KILL_WRONG_OBJECT, "policy.kill.wrong_object", Sum);
define_kcounter!(POLICY_KILL_VMAR_WX, "policy.kill.vmar_wx", Sum);
define_kcounter!(POLICY_KILL_NEW_VMO, "policy.kill.new_vmo", Sum);
define_kcounter!(POLICY_KILL_NEW_CHANNEL, "policy.kill.new_channel", Sum);
define_kcounter!(POLICY_KILL_NEW_EVENT, "policy.kill.new_event", Sum);
define_kcounter!(POLICY_KILL_NEW_EVENTPAIR, "policy.kill.new_eventpair", Sum);
define_kcounter!(POLICY_KILL_NEW_PORT, "policy.kill.new_port", Sum);
define_kcounter!(POLICY_KILL_NEW_SOCKET, "policy.kill.new_socket", Sum);
define_kcounter!(POLICY_KILL_NEW_FIFO, "policy.kill.new_fifo", Sum);
define_kcounter!(POLICY_KILL_NEW_TIMER, "policy.kill.new_timer", Sum);
define_kcounter!(POLICY_KILL_NEW_PROCESS, "policy.kill.new_process", Sum);
define_kcounter!(POLICY_KILL_NEW_PROFILE, "policy.kill.new_profile", Sum);
define_kcounter!(POLICY_KILL_NEW_PAGER, "policy.kill.new_pager", Sum);
define_kcounter!(POLICY_KILL_AMBIENT_MARK_VMO_EXEC, "policy.kill.ambient_mark_vmo_exec", Sum);
define_kcounter!(POLICY_KILL_NEW_IOB, "policy.kill.new_iob", Sum);
define_kcounter!(POLICY_KILL_NEW_SAMPLER, "policy.kill.new_sampler", Sum);

impl JobPolicy {
    /// Creates a root policy with default values.
    pub const fn create_root_policy() -> Self {
        Self { collection: JobPolicyCollection::new(), slack: TimerSlack::none() }
    }

    /// Merge slice |policy_input| into this object.
    ///
    /// |mode| controls what happens when the policies in |policy_input| and this object intersect.
    /// |mode| must be one of:
    ///
    /// ZX_JOB_POL_RELATIVE - Conflicting policies are ignored and will not cause the call to fail.
    ///
    /// ZX_JOB_POL_ABSOLUTE - If any of the policies in |policy_input| conflict with those in this
    ///   object, the call will fail with an error and this object will not be modified.
    pub fn add_basic_policy(
        &mut self,
        mode: u32,
        policy_input: &[zx_policy_basic_v2_t],
    ) -> Result<(), Status> {
        // Don't allow overlong policies.
        if policy_input.len() > ZX_POL_MAX as usize {
            return Err(Status::OUT_OF_RANGE);
        }

        let mut updated_collection = self.collection;
        let mut has_new_any = false;
        let mut new_any_override = 0;

        for in_policy in policy_input {
            if in_policy.condition == ZX_POL_NEW_ANY {
                for cond in NEW_OBJECT_POLICIES {
                    add_partial(
                        mode,
                        cond,
                        in_policy.action,
                        ZX_POL_OVERRIDE_ALLOW,
                        &mut updated_collection,
                    )?;
                }
                has_new_any = true;
                new_any_override = in_policy.flags;
            } else {
                add_partial(
                    mode,
                    in_policy.condition,
                    in_policy.action,
                    in_policy.flags,
                    &mut updated_collection,
                )?;
            }
        }

        if has_new_any {
            if !policy_override_is_valid(new_any_override) {
                return Err(Status::INVALID_ARGS);
            }
            let override_bit = new_any_override == ZX_POL_OVERRIDE_ALLOW;
            for cond in NEW_OBJECT_POLICIES {
                updated_collection.get_mut(cond as usize).set_override(override_bit);
            }
        }

        self.collection = updated_collection;
        Ok(())
    }

    /// Returns the action (e.g. ZX_POL_ACTION_ALLOW) for the specified |condition|.
    ///
    /// This method asserts if |condition| is invalid, and returns ZX_POL_ACTION_DENY for all other
    /// failure modes.
    pub fn query_basic_policy(&self, condition: u32) -> u32 {
        if condition >= ZX_POL_MAX || condition == ZX_POL_NEW_ANY {
            return ZX_POL_ACTION_DENY;
        }
        self.collection.get(condition as usize).action()
    }

    /// Returns if the action for the specified condition can be overridden, so it returns
    /// ZX_POL_OVERRIDE_ALLOW or ZX_POL_OVERRIDE_DENY.
    pub fn query_basic_policy_override(&self, condition: u32) -> u32 {
        if condition >= ZX_POL_MAX || condition == ZX_POL_NEW_ANY {
            return ZX_POL_OVERRIDE_DENY;
        }
        if self.collection.get(condition as usize).r#override() {
            ZX_POL_OVERRIDE_ALLOW
        } else {
            ZX_POL_OVERRIDE_DENY
        }
    }

    /// Sets the timer slack policy.
    ///
    /// |slack.amount| must be >= 0.
    pub fn set_timer_slack(&mut self, slack: TimerSlack) {
        self.slack = slack;
    }

    /// Returns the timer slack policy.
    pub fn get_timer_slack(&self) -> TimerSlack {
        self.slack
    }

    /// Increment the kcounter for the given |action| and |condition|.
    ///
    /// action must be < ZX_POL_ACTION_MAX and condition must be < ZX_POL_MAX.
    ///
    /// For example: `JobPolicy::increment_counter(ZX_POL_ACTION_KILL, ZX_POL_NEW_CHANNEL);`
    pub fn increment_counter(action: u32, condition: u32) {
        debug_assert!(action < ZX_POL_ACTION_MAX);
        debug_assert!(condition < ZX_POL_MAX);

        let counter: Option<&'static Counter> = match action {
            ZX_POL_ACTION_DENY | ZX_POL_ACTION_DENY_EXCEPTION => match condition {
                ZX_POL_BAD_HANDLE => Some(&POLICY_DENY_BAD_HANDLE),
                ZX_POL_WRONG_OBJECT => Some(&POLICY_DENY_WRONG_OBJECT),
                ZX_POL_VMAR_WX => Some(&POLICY_DENY_VMAR_WX),
                ZX_POL_NEW_ANY => None, // ZX_POL_NEW_ANY is a pseudo condition
                ZX_POL_NEW_VMO => Some(&POLICY_DENY_NEW_VMO),
                ZX_POL_NEW_CHANNEL => Some(&POLICY_DENY_NEW_CHANNEL),
                ZX_POL_NEW_EVENT => Some(&POLICY_DENY_NEW_EVENT),
                ZX_POL_NEW_EVENTPAIR => Some(&POLICY_DENY_NEW_EVENTPAIR),
                ZX_POL_NEW_PORT => Some(&POLICY_DENY_NEW_PORT),
                ZX_POL_NEW_SOCKET => Some(&POLICY_DENY_NEW_SOCKET),
                ZX_POL_NEW_FIFO => Some(&POLICY_DENY_NEW_FIFO),
                ZX_POL_NEW_TIMER => Some(&POLICY_DENY_NEW_TIMER),
                ZX_POL_NEW_PROCESS => Some(&POLICY_DENY_NEW_PROCESS),
                ZX_POL_NEW_PROFILE => Some(&POLICY_DENY_NEW_PROFILE),
                ZX_POL_NEW_PAGER => Some(&POLICY_DENY_NEW_PAGER),
                ZX_POL_AMBIENT_MARK_VMO_EXEC => Some(&POLICY_DENY_AMBIENT_MARK_VMO_EXEC),
                ZX_POL_NEW_IOB => Some(&POLICY_DENY_NEW_IOB),
                ZX_POL_NEW_SAMPLER => Some(&POLICY_DENY_NEW_SAMPLER),
                _ => None,
            },
            ZX_POL_ACTION_KILL => match condition {
                ZX_POL_BAD_HANDLE => Some(&POLICY_KILL_BAD_HANDLE),
                ZX_POL_WRONG_OBJECT => Some(&POLICY_KILL_WRONG_OBJECT),
                ZX_POL_VMAR_WX => Some(&POLICY_KILL_VMAR_WX),
                ZX_POL_NEW_ANY => None, // ZX_POL_NEW_ANY is a pseudo condition
                ZX_POL_NEW_VMO => Some(&POLICY_KILL_NEW_VMO),
                ZX_POL_NEW_CHANNEL => Some(&POLICY_KILL_NEW_CHANNEL),
                ZX_POL_NEW_EVENT => Some(&POLICY_KILL_NEW_EVENT),
                ZX_POL_NEW_EVENTPAIR => Some(&POLICY_KILL_NEW_EVENTPAIR),
                ZX_POL_NEW_PORT => Some(&POLICY_KILL_NEW_PORT),
                ZX_POL_NEW_SOCKET => Some(&POLICY_KILL_NEW_SOCKET),
                ZX_POL_NEW_FIFO => Some(&POLICY_KILL_NEW_FIFO),
                ZX_POL_NEW_TIMER => Some(&POLICY_KILL_NEW_TIMER),
                ZX_POL_NEW_PROCESS => Some(&POLICY_KILL_NEW_PROCESS),
                ZX_POL_NEW_PROFILE => Some(&POLICY_KILL_NEW_PROFILE),
                ZX_POL_NEW_PAGER => Some(&POLICY_KILL_NEW_PAGER),
                ZX_POL_AMBIENT_MARK_VMO_EXEC => Some(&POLICY_KILL_AMBIENT_MARK_VMO_EXEC),
                ZX_POL_NEW_IOB => Some(&POLICY_KILL_NEW_IOB),
                ZX_POL_NEW_SAMPLER => Some(&POLICY_KILL_NEW_SAMPLER),
                _ => None,
            },
            _ => None,
        };
        let Some(counter) = counter else {
            return;
        };
        counter.add(1);
    }
}

#[cfg(ktest)]
#[unittest::suite(name = "job_policy_rust")]
/// Tests for JobPolicy and JobPolicyCollection.
mod tests {
    use unittest::{expect_eq, expect_ok, expect_true};
    use zx_types::{ZX_JOB_POL_RELATIVE, ZX_POL_ACTION_ALLOW_EXCEPTION};

    #[test]
    /// Tests bit manipulation in JobPolicyCollection.
    fn test_job_policy_collection() {
        // Verify that our helper struct fiddles with the bits properly. The interaction and
        // translation to proper constants where needed is checked in the query policy API.
        let mut policies = JobPolicyCollection::new();

        const ACTIONS: [u32; 4] = [
            ZX_POL_ACTION_DENY,
            ZX_POL_ACTION_DENY_EXCEPTION,
            ZX_POL_ACTION_ALLOW_EXCEPTION,
            ZX_POL_ACTION_KILL,
        ];
        const OVERRIDE: [bool; 2] = [true, false];

        // Set a policy, then verify that everything before and after remains the same.
        for i in 0..ZX_POL_MAX as usize {
            if i == ZX_POL_NEW_ANY as usize {
                continue;
            }
            policies.get_mut(i).set_action(ACTIONS[i % 4]);
            policies.get_mut(i).set_override(OVERRIDE[i % 2]);

            for j in 0..i {
                if j == ZX_POL_NEW_ANY as usize {
                    continue;
                }
                expect_eq!(policies.get(j).action(), ACTIONS[j % 4]);
                expect_eq!(policies.get(j).r#override(), OVERRIDE[j % 2]);
            }

            for j in (i + 1)..ZX_POL_MAX as usize {
                if j == ZX_POL_NEW_ANY as usize {
                    continue;
                }
                // Arbitrary default values, from being constructed from 0.
                expect_eq!(policies.get(j).action(), 0);
                expect_eq!(policies.get(j).r#override(), true);
            }

            // After all those ops, check that new state is what we expect.
            expect_eq!(policies.get(i).action(), ACTIONS[i % 4]);
            expect_eq!(policies.get(i).r#override(), OVERRIDE[i % 2]);
        }
    }

    #[test]
    /// Tests initial state of root job policy.
    fn test_initial_state() {
        let p = JobPolicy::create_root_policy();

        for pol in 0..ZX_POL_MAX {
            if pol == ZX_POL_NEW_ANY {
                continue;
            }
            expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(pol));
            expect_eq!(ZX_POL_OVERRIDE_ALLOW, p.query_basic_policy_override(pol));
        }

        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_ANY));
        expect_true!(p.get_timer_slack() == TimerSlack::none());
    }

    #[test]
    /// Tests add_basic_policy when widening is not allowed.
    fn test_add_basic_policy_no_widening() {
        // Verify that add_basic_policy prevents "widening" of a deny all policy.
        let mut p = JobPolicy::create_root_policy();

        // Start with deny all.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_DENY,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));

        // Attempt to allow event creation.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_EVENT,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        // Fails because mode is ZX_JOB_POL_ABSOLUTE.
        expect_true!(
            p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]) == Err(Status::ALREADY_EXISTS)
        );
        // Does not fail because mode is ZX_JOB_POL_RELATIVE.
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_RELATIVE, &[policy]));

        // However, action is still deny.
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_VMO));
    }

    #[test]
    /// Tests add_basic_policy when widening is allowed.
    fn test_add_basic_policy_allow_widening() {
        let mut p = JobPolicy::create_root_policy();

        // Start with deny all, but allowing override.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_DENY,
            flags: ZX_POL_OVERRIDE_ALLOW,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));

        // Allow event creation.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_EVENT,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));
        // Test that it in fact, allows for event, but denies for VMO.
        expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_NEW_EVENT));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_VMO));
    }

    #[test]
    /// Tests add_basic_policy prevents widening of policy using NEW_ANY.
    fn test_add_basic_policy_no_widening_with_any() {
        // Verify that add_basic_policy prevents "widening" of policy using NEW_ANY.
        let mut p = JobPolicy::create_root_policy();

        // Start with deny event creation.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_EVENT,
            action: ZX_POL_ACTION_DENY,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));

        // Attempt to allow event creation.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_EVENT,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        // Fails because mode is ZX_JOB_POL_ABSOLUTE.
        expect_true!(
            p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]) == Err(Status::ALREADY_EXISTS)
        );
        // Does not fail because mode is ZX_JOB_POL_RELATIVE.
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_RELATIVE, &[policy]));

        // However, action is still deny.
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));

        // Attempt to allow any.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        expect_true!(
            p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]) == Err(Status::ALREADY_EXISTS)
        );
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_RELATIVE, &[policy]));

        // Still deny.
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));
    }

    #[test]
    /// Tests add_basic_policy fails if invalid override flag is used with NEW_ANY.
    fn test_add_basic_policy_invalid_override_fails_with_any() {
        // Verify that add_basic_policy fails if we use an invalid value for flags.
        let mut p = JobPolicy::create_root_policy();

        // Deny creating new kernel objects, but allow override.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_DENY,
            flags: ZX_POL_OVERRIDE_ALLOW,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));

        // Override works.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_ALLOW,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));

        // Using an invalid override should not work.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY + 1,
        };
        expect_true!(
            p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]) == Err(Status::INVALID_ARGS)
        );
    }

    #[test]
    /// Tests add_basic_policy fails if invalid override flag is used.
    fn test_add_basic_policy_invalid_override_fails() {
        // Verify that add_basic_policy fails if we use an invalid value for flags.
        let mut p = JobPolicy::create_root_policy();

        // Deny creating new vmos, but allow override.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_VMO,
            action: ZX_POL_ACTION_DENY,
            flags: ZX_POL_OVERRIDE_ALLOW,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));

        // Override works.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_VMO,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_ALLOW,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));

        // Using an invalid override should not work.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_VMO,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY + 1,
        };
        expect_true!(
            p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]) == Err(Status::INVALID_ARGS)
        );
    }

    #[test]
    /// Tests add_basic_policy allows widening with NEW_ANY.
    fn test_add_basic_policy_allow_widening_with_any() {
        let mut p = JobPolicy::create_root_policy();

        // Start with deny event creation.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_EVENT,
            action: ZX_POL_ACTION_DENY,
            flags: ZX_POL_OVERRIDE_ALLOW,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));

        // Change it to allow any.
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_ANY,
            action: ZX_POL_ACTION_ALLOW,
            flags: ZX_POL_OVERRIDE_DENY,
        };
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));

        // Verify event can now be created.
        expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_NEW_EVENT));
    }

    #[test]
    /// Tests add_basic_policy with ZX_JOB_POL_ABSOLUTE mode.
    fn test_add_basic_policy_absolute() {
        let mut p = JobPolicy::create_root_policy();
        // TODO(cpu). Don't allow this. It is proably a logic bug in the caller.
        let repeated = [
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags: ZX_POL_OVERRIDE_DENY,
            },
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags: ZX_POL_OVERRIDE_DENY,
            },
        ];
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &repeated));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));

        let conflicting = [
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags: ZX_POL_OVERRIDE_DENY,
            },
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_ALLOW,
                flags: ZX_POL_OVERRIDE_DENY,
            },
        ];
        expect_true!(
            p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &conflicting) == Err(Status::ALREADY_EXISTS)
        );
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_VMO));
    }

    #[test]
    /// Tests add_basic_policy with ZX_JOB_POL_RELATIVE mode.
    fn test_add_basic_policy_relative() {
        let mut p = JobPolicy::create_root_policy();
        // TODO(cpu). Don't allow this. It is proably a logic bug in the caller.
        let repeated = [
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags: ZX_POL_OVERRIDE_DENY,
            },
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags: ZX_POL_OVERRIDE_DENY,
            },
        ];
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_RELATIVE, &repeated));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_TIMER));

        let conflicting = [
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags: ZX_POL_OVERRIDE_DENY,
            },
            zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_ALLOW,
                flags: ZX_POL_OVERRIDE_DENY,
            },
        ];
        expect_ok!(p.add_basic_policy(ZX_JOB_POL_RELATIVE, &conflicting));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_FIFO));
    }

    #[test]
    /// Tests add_basic_policy unmodified on error with and without override.
    fn test_add_basic_policy_unmodified_on_error() {
        // Test that add_basic_policy does not modify JobPolicy when it fails.
        for flags in [ZX_POL_OVERRIDE_DENY, ZX_POL_OVERRIDE_ALLOW] {
            let mut p = JobPolicy::create_root_policy();

            let policy = [
                zx_policy_basic_v2_t {
                    condition: ZX_POL_NEW_VMO,
                    action: ZX_POL_ACTION_ALLOW_EXCEPTION,
                    flags,
                },
                zx_policy_basic_v2_t {
                    condition: ZX_POL_NEW_CHANNEL,
                    action: ZX_POL_ACTION_KILL,
                    flags,
                },
            ];

            expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &policy));
            expect_eq!(ZX_POL_ACTION_ALLOW_EXCEPTION, p.query_basic_policy(ZX_POL_NEW_VMO));
            expect_eq!(ZX_POL_ACTION_KILL, p.query_basic_policy(ZX_POL_NEW_CHANNEL));

            let orig = p;

            let mut new_policy =
                zx_policy_basic_v2_t { condition: ZX_POL_NEW_ANY, action: u32::MAX, flags };
            expect_true!(
                p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, core::slice::from_ref(&new_policy))
                    == Err(Status::NOT_SUPPORTED)
            );
            expect_true!(orig == p);

            if flags == ZX_POL_OVERRIDE_DENY {
                new_policy = zx_policy_basic_v2_t {
                    condition: ZX_POL_NEW_VMO,
                    action: ZX_POL_ACTION_ALLOW,
                    flags,
                };
                expect_true!(
                    p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, core::slice::from_ref(&new_policy))
                        == Err(Status::ALREADY_EXISTS)
                );
                expect_true!(orig == p);
            }
        }
    }

    #[test]
    /// Tests add_basic_policy deny any new with and without override.
    fn test_add_basic_policy_deny_any_new() {
        for flags in [ZX_POL_OVERRIDE_DENY, ZX_POL_OVERRIDE_ALLOW] {
            let mut p = JobPolicy::create_root_policy();
            let policy = zx_policy_basic_v2_t {
                condition: ZX_POL_NEW_ANY,
                action: ZX_POL_ACTION_DENY,
                flags,
            };
            expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));

            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_VMO));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_CHANNEL));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENT));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_EVENTPAIR));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_PORT));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_SOCKET));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_FIFO));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_TIMER));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_PROCESS));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_PROFILE));
            expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_PAGER));

            expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_BAD_HANDLE));
            expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_WRONG_OBJECT));
            expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_VMAR_WX));
            expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_AMBIENT_MARK_VMO_EXEC));
        }
    }

    #[test]
    /// Tests set_timer_slack and get_timer_slack.
    fn test_set_get_timer_slack() {
        use crate::kernel::deadline::{DurationUnknown, SlackMode};
        let mut p = JobPolicy::create_root_policy();

        p.set_timer_slack(TimerSlack::new(DurationUnknown(1200), SlackMode::Early));
        expect_eq!(1200, p.get_timer_slack().amount().0);
        expect_true!(p.get_timer_slack().mode() == SlackMode::Early);
    }

    #[test]
    /// Tests add_basic_policy denying only process creation.
    fn test_add_basic_policy_deny_process_only() {
        let mut p = JobPolicy::create_root_policy();
        let policy = zx_policy_basic_v2_t {
            condition: ZX_POL_NEW_PROCESS,
            action: ZX_POL_ACTION_DENY,
            flags: 0,
        };

        expect_ok!(p.add_basic_policy(ZX_JOB_POL_ABSOLUTE, &[policy]));
        expect_eq!(ZX_POL_ACTION_DENY, p.query_basic_policy(ZX_POL_NEW_PROCESS));
        expect_eq!(ZX_POL_ACTION_ALLOW, p.query_basic_policy(ZX_POL_NEW_PROFILE));
    }

    #[test]
    /// Tests increment_counter across all action and condition pairs.
    fn test_increment_counters() {
        // There's no programmatic interface to read kcounters so there's nothing to assert (aside
        // from not crashing).
        let p = JobPolicy::create_root_policy();

        for action in 0..ZX_POL_ACTION_MAX {
            for condition in 0..ZX_POL_MAX {
                JobPolicy::increment_counter(action, condition);
            }
        }
        let _ = p;
    }
}
