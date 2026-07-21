// SPDX-License-Identifier: GPL-2.0

//! Error type for the rt_ipc userspace library.

use crate::abi::errno;
use std::fmt;

/// Errors returned by rt_ipc operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The kernel does not provide the rt_ipc syscalls (returned `ENOSYS`).
    ///
    /// This is the signal callers use to fall back to the socket-backed
    /// reference transport when running on a stock kernel.
    NotSupported,
    /// The endpoint name was empty or longer than [`crate::abi::RT_IPC_NAME_MAX`].
    InvalidName,
    /// A payload exceeded [`crate::abi::RT_IPC_MSG_MAX`].
    MessageTooLarge,
    /// No server is currently registered for the requested endpoint.
    NoSuchEndpoint,
    /// The endpoint name is already registered.
    EndpointExists,
    /// The operation was interrupted by a signal.
    Interrupted,
    /// Any other errno reported by the kernel.
    Os(i32),
    /// An error originating from the std I/O layer (socket transport).
    Io(String),
}

impl Error {
    /// Convert a negative syscall return value (`-errno`) into an [`Error`].
    pub(crate) fn from_neg_errno(ret: i64) -> Error {
        let e = (-ret) as i32;
        match e {
            errno::ENOSYS => Error::NotSupported,
            errno::ENAMETOOLONG | errno::EINVAL => Error::InvalidName,
            errno::EMSGSIZE | errno::ENOSPC => Error::MessageTooLarge,
            errno::ESRCH | errno::ECONNREFUSED => Error::NoSuchEndpoint,
            errno::EEXIST => Error::EndpointExists,
            errno::EINTR => Error::Interrupted,
            other => Error::Os(other),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotSupported => write!(f, "rt_ipc syscalls not available (ENOSYS)"),
            Error::InvalidName => write!(f, "invalid endpoint name"),
            Error::MessageTooLarge => write!(f, "message exceeds RT_IPC_MSG_MAX"),
            Error::NoSuchEndpoint => write!(f, "no server registered for endpoint"),
            Error::EndpointExists => write!(f, "endpoint name already registered"),
            Error::Interrupted => write!(f, "operation interrupted by signal"),
            Error::Os(e) => write!(f, "os error {e}"),
            Error::Io(s) => write!(f, "io error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e.to_string())
    }
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;
