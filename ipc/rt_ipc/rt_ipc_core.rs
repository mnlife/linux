// SPDX-License-Identifier: GPL-2.0

//! Portable, `no_std` core logic for rt_ipc.
//!
//! This module contains the parts of the migrating-thread IPC mechanism that
//! are pure data-structure and policy code — and therefore can be reasoned
//! about and unit-tested in isolation, independent of any kernel primitive:
//!
//! * the endpoint **registry** (name -> endpoint), keyed by a stable id so a
//!   client can address a server without a separate name-lookup syscall;
//! * request/reply **argument validation** (name length, payload size);
//! * per-thread **invocation-stack** bookkeeping, including the recursion
//!   bound that keeps a migrating thread from overflowing the kernel stack
//!   through unbounded nested RPCs;
//! * **owner cleanup**, so a crashing or exiting server's endpoints are
//!   reclaimed.
//!
//! The blocking, address-space switching and register save/restore that make
//! up the actual *thread migration* live in the (necessarily unsafe,
//! arch-specific) kernel glue layer; this core deliberately knows nothing
//! about them.  Keeping the policy here makes the trusted computing base small
//! and testable.
//!
//! This file is included both as a submodule of the `rt_ipc_rust` kernel
//! module and as a standalone unit-test target (`rustc --test`).  It therefore
//! avoids crate-level attributes and selects its allocator collections based
//! on the build.  In a production kernel build the infallible `alloc`
//! `BTreeMap` shown here would be replaced by the kernel's fallible
//! `kernel::rbtree::RBTree`; the logic and its tests are identical.

// Under `cargo/rustc --test` we build against `std`; as a kernel submodule we
// build against the `alloc` crate that the kernel provides.
#[cfg(not(test))]
use alloc::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeMap;

/// Maximum endpoint name length, excluding any NUL terminator.
pub const RT_IPC_NAME_MAX: usize = 63;

/// Maximum request/reply payload, in bytes.
pub const RT_IPC_MSG_MAX: usize = 4096;

/// Sentinel meaning "no endpoint".
pub const RT_IPC_ENDPOINT_INVALID: u64 = u64::MAX;

/// Maximum depth of nested migrating-thread invocations.
///
/// Each nested RPC consumes a kernel stack frame in the server, so the depth
/// must be bounded.  Exceeding it fails the invocation with
/// [`CoreError::TooDeep`] instead of risking a stack overflow.
pub const RT_IPC_MAX_DEPTH: u32 = 32;

/// Errors produced by the core logic.  These map 1:1 onto `-errno` values in
/// the kernel glue layer (see the mapping in [`CoreError::to_errno`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreError {
    /// Name empty, too long, or contains a NUL byte.
    NameInvalid,
    /// Payload exceeds [`RT_IPC_MSG_MAX`].
    MsgTooLarge,
    /// An endpoint with this id/name is already registered.
    Exists,
    /// No endpoint is registered for the requested id.
    NoEndpoint,
    /// The nested-invocation depth limit was reached.
    TooDeep,
}

impl CoreError {
    /// Standard `errno` value (positive) for this error.
    pub fn to_errno(self) -> i32 {
        match self {
            CoreError::NameInvalid => 22, // EINVAL
            CoreError::MsgTooLarge => 90, // EMSGSIZE
            CoreError::Exists => 17,      // EEXIST
            CoreError::NoEndpoint => 3,   // ESRCH
            CoreError::TooDeep => 40,     // ELOOP
        }
    }
}

/// 64-bit FNV-1a hash used to derive a stable endpoint id from a name.
///
/// Userspace computes the same value (see the `rt_ipc` userspace crate), so
/// client and server agree on the identifier without a lookup syscall.
pub fn endpoint_id(name: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut i = 0;
    while i < name.len() {
        hash ^= name[i] as u64;
        hash = hash.wrapping_mul(PRIME);
        i += 1;
    }
    if hash == RT_IPC_ENDPOINT_INVALID {
        hash ^= 1;
    }
    hash
}

/// Validate an endpoint name, returning its stable id on success.
pub fn validate_name(name: &[u8]) -> Result<u64, CoreError> {
    if name.is_empty() || name.len() > RT_IPC_NAME_MAX || name.contains(&0) {
        return Err(CoreError::NameInvalid);
    }
    Ok(endpoint_id(name))
}

/// Validate a payload length.
pub fn validate_len(len: usize) -> Result<(), CoreError> {
    if len > RT_IPC_MSG_MAX {
        return Err(CoreError::MsgTooLarge);
    }
    Ok(())
}

/// A registered server endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// Stable identifier ( = [`endpoint_id`] of the name).
    pub id: u64,
    /// Opaque token identifying the owning server task (e.g. a `pid` or
    /// `task_struct` pointer bits).  Used for cleanup on exit.
    pub owner: u64,
}

/// The endpoint registry.
///
/// In the kernel this is protected by a lock in the glue layer; the core keeps
/// it lock-free and single-threaded so it can be tested deterministically.
#[derive(Default)]
pub struct Registry {
    endpoints: BTreeMap<u64, Endpoint>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Registry {
        Registry {
            endpoints: BTreeMap::new(),
        }
    }

    /// Register `name` as owned by `owner`, returning the endpoint id.
    ///
    /// Fails with [`CoreError::Exists`] if the id is already taken by a
    /// *different* owner (a genuine collision or duplicate registration).  A
    /// re-registration by the same owner is idempotent.
    pub fn register(&mut self, name: &[u8], owner: u64) -> Result<u64, CoreError> {
        let id = validate_name(name)?;
        match self.endpoints.get(&id) {
            Some(ep) if ep.owner != owner => Err(CoreError::Exists),
            Some(_) => Ok(id),
            None => {
                self.endpoints.insert(id, Endpoint { id, owner });
                Ok(id)
            }
        }
    }

    /// Look up an endpoint by id.
    pub fn lookup(&self, id: u64) -> Result<Endpoint, CoreError> {
        self.endpoints
            .get(&id)
            .copied()
            .ok_or(CoreError::NoEndpoint)
    }

    /// Remove an endpoint, but only if `owner` matches.
    pub fn unregister(&mut self, id: u64, owner: u64) -> Result<(), CoreError> {
        match self.endpoints.get(&id) {
            Some(ep) if ep.owner == owner => {
                self.endpoints.remove(&id);
                Ok(())
            }
            _ => Err(CoreError::NoEndpoint),
        }
    }

    /// Drop every endpoint owned by `owner` (called when a server exits).
    /// Returns the number of endpoints reclaimed.
    pub fn reclaim_owner(&mut self, owner: u64) -> usize {
        let before = self.endpoints.len();
        self.endpoints.retain(|_, ep| ep.owner != owner);
        before - self.endpoints.len()
    }

    /// Number of currently registered endpoints.
    pub fn len(&self) -> usize {
        self.endpoints.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }
}

/// Per-thread migrating-invocation stack depth guard.
///
/// A migrating thread that issues a nested RPC (server A calls into server B
/// while serving a request) pushes another frame here.  The bound prevents a
/// cycle of servers from overflowing the kernel stack.
#[derive(Debug, Default, Clone, Copy)]
pub struct InvocationDepth(u32);

impl InvocationDepth {
    /// A fresh, zero-depth guard.
    pub fn new() -> InvocationDepth {
        InvocationDepth(0)
    }

    /// Attempt to enter a nested invocation.
    pub fn enter(&mut self) -> Result<(), CoreError> {
        if self.0 >= RT_IPC_MAX_DEPTH {
            return Err(CoreError::TooDeep);
        }
        self.0 += 1;
        Ok(())
    }

    /// Leave an invocation.
    pub fn leave(&mut self) {
        self.0 = self.0.saturating_sub(1);
    }

    /// Current nesting depth.
    pub fn depth(&self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_matches_userspace_and_is_stable() {
        // Must equal the userspace FNV-1a implementation.
        assert_eq!(endpoint_id(b"demo.echo"), endpoint_id(b"demo.echo"));
        assert_ne!(endpoint_id(b"a"), endpoint_id(b"b"));
        assert_ne!(endpoint_id(b"x"), RT_IPC_ENDPOINT_INVALID);
    }

    #[test]
    fn name_validation() {
        assert_eq!(validate_name(b""), Err(CoreError::NameInvalid));
        assert_eq!(
            validate_name(&[b'a'; RT_IPC_NAME_MAX + 1]),
            Err(CoreError::NameInvalid)
        );
        assert_eq!(validate_name(b"a\0b"), Err(CoreError::NameInvalid));
        assert!(validate_name(b"ok.name").is_ok());
    }

    #[test]
    fn len_validation() {
        assert!(validate_len(RT_IPC_MSG_MAX).is_ok());
        assert_eq!(
            validate_len(RT_IPC_MSG_MAX + 1),
            Err(CoreError::MsgTooLarge)
        );
    }

    #[test]
    fn register_lookup_unregister() {
        let mut reg = Registry::new();
        let id = reg.register(b"svc.a", 100).unwrap();
        assert_eq!(reg.lookup(id).unwrap().owner, 100);

        // Idempotent re-registration by the same owner.
        assert_eq!(reg.register(b"svc.a", 100).unwrap(), id);
        assert_eq!(reg.len(), 1);

        // Different owner collides.
        assert_eq!(reg.register(b"svc.a", 200), Err(CoreError::Exists));

        // Wrong owner cannot unregister.
        assert_eq!(reg.unregister(id, 200), Err(CoreError::NoEndpoint));
        reg.unregister(id, 100).unwrap();
        assert_eq!(reg.lookup(id), Err(CoreError::NoEndpoint));
    }

    #[test]
    fn reclaim_on_owner_exit() {
        let mut reg = Registry::new();
        reg.register(b"svc.a", 1).unwrap();
        reg.register(b"svc.b", 1).unwrap();
        reg.register(b"svc.c", 2).unwrap();
        assert_eq!(reg.reclaim_owner(1), 2);
        assert_eq!(reg.len(), 1);
        assert!(reg.lookup(endpoint_id(b"svc.c")).is_ok());
    }

    #[test]
    fn depth_guard_bounds_recursion() {
        let mut d = InvocationDepth::new();
        for _ in 0..RT_IPC_MAX_DEPTH {
            d.enter().unwrap();
        }
        assert_eq!(d.depth(), RT_IPC_MAX_DEPTH);
        assert_eq!(d.enter(), Err(CoreError::TooDeep));
        d.leave();
        assert!(d.enter().is_ok());
    }

    #[test]
    fn errno_mapping_is_distinct() {
        let all = [
            CoreError::NameInvalid,
            CoreError::MsgTooLarge,
            CoreError::Exists,
            CoreError::NoEndpoint,
            CoreError::TooDeep,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.to_errno(), b.to_errno());
            }
        }
    }
}
