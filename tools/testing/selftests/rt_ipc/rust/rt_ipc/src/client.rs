// SPDX-License-Identifier: GPL-2.0

//! High-level client API.

use crate::abi::RT_IPC_MSG_MAX;
use crate::error::Result;
use crate::transport::{self, Backend, ClientInner};

/// A connection to an rt_ipc server endpoint.
///
/// On the kernel backend an invocation migrates the calling thread into the
/// server's address space, so the RPC runs with *this* thread's scheduling
/// attributes — the property that eliminates priority inversion.
pub struct Client {
    inner: ClientInner,
    endpoint: String,
}

impl Client {
    /// Connect to the named endpoint using the automatically selected backend.
    pub fn connect(name: &str) -> Result<Client> {
        Client::connect_with(name, Backend::Auto)
    }

    /// Connect using an explicit [`Backend`].
    pub fn connect_with(name: &str, backend: Backend) -> Result<Client> {
        Ok(Client {
            inner: transport::client_connect(name, backend)?,
            endpoint: name.to_string(),
        })
    }

    /// The endpoint name this client is connected to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Perform an RPC, writing the reply into `resp` and returning its length.
    ///
    /// This is the zero-allocation fast path preferred by latency-sensitive
    /// callers and the benchmark.
    pub fn invoke(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize> {
        transport::client_invoke(&mut self.inner, req, resp)
    }

    /// Convenience wrapper that allocates and returns the reply as a `Vec`.
    pub fn call(&mut self, req: &[u8]) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; RT_IPC_MSG_MAX];
        let n = self.invoke(req, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}
