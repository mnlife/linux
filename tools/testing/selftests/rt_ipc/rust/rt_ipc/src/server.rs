// SPDX-License-Identifier: GPL-2.0

//! High-level server API.

use crate::error::Result;
use crate::transport::{self, Backend, Handler, StopFlag};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Builder for an rt_ipc server endpoint.
pub struct Server {
    name: String,
    backend: Backend,
}

impl Server {
    /// Create a server builder for `name` using the auto-selected backend.
    pub fn new(name: &str) -> Server {
        Server {
            name: name.to_string(),
            backend: Backend::Auto,
        }
    }

    /// Select an explicit [`Backend`].
    pub fn backend(mut self, backend: Backend) -> Server {
        self.backend = backend;
        self
    }

    /// Register the endpoint and serve requests in the current thread until
    /// the process is terminated.  This is the shape used by a dedicated
    /// server process (see `userdemo`).
    ///
    /// `handler` receives each request payload and returns the reply payload.
    /// It must be `Send + Sync` because, under the migrating-thread model,
    /// multiple client threads execute it concurrently.
    pub fn run<F>(self, handler: F) -> Result<()>
    where
        F: Fn(&[u8]) -> Vec<u8> + Send + Sync + 'static,
    {
        let stop: StopFlag = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        transport::serve_loop(&self.name, self.backend, handler, &stop)
    }

    /// Register the endpoint and serve requests on a background thread,
    /// returning a [`RunningServer`] handle for graceful shutdown.  This is
    /// the shape used by the integration tests.
    pub fn spawn<F>(self, handler: F) -> Result<RunningServer>
    where
        F: Fn(&[u8]) -> Vec<u8> + Send + Sync + 'static,
    {
        let stop: StopFlag = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handler: Arc<Handler> = Arc::new(handler);
        let name = self.name.clone();
        let backend = self.backend;

        // Rendezvous so callers can `connect()` immediately after `spawn()`
        // without racing the server's bind/register.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let join = std::thread::Builder::new()
            .name(format!("rt_ipc-srv-{name}"))
            .spawn(move || {
                // Signal readiness once the endpoint is guaranteed to exist.
                let armed = transport::serve_prepare(&name, backend);
                let _ = ready_tx.send(armed.as_ref().map(|_| ()).map_err(|e| e.clone()));
                let armed = armed?;
                transport::serve_run(armed, handler, &stop_thread)
            })
            .map_err(|e| crate::error::Error::Io(e.to_string()))?;

        // Propagate any bind/register failure to the caller.
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = join.join();
                return Err(e);
            }
            Err(_) => {
                // Thread died before signalling; surface its error.
                return match join.join() {
                    Ok(res) => res.map(|_| unreachable!()),
                    Err(_) => Err(crate::error::Error::Io("server thread panicked".into())),
                };
            }
        }

        Ok(RunningServer {
            stop,
            join: Some(join),
        })
    }
}

/// Handle to a server running on a background thread.
pub struct RunningServer {
    stop: StopFlag,
    join: Option<JoinHandle<Result<()>>>,
}

impl RunningServer {
    /// Request a graceful shutdown without waiting for the loop to exit.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Request shutdown and wait for the serve loop to finish.
    pub fn shutdown(mut self) -> Result<()> {
        self.stop();
        self.join
            .take()
            .map(|j| {
                j.join().unwrap_or_else(|_| {
                    Err(crate::error::Error::Io("server thread panicked".into()))
                })
            })
            .unwrap_or(Ok(()))
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
