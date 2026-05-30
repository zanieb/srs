//! An implementation of IPC locks, guaranteed to be released if a process dies
//!
//! This module implements a locking server/client where the main `cargo fix`
//! process will start up a server and then all the client processes will
//! connect to it. The main purpose of this file is to let read-only compiler
//! passes overlap while ensuring that each crate (aka file entry point) is only
//! fixed by one process at a time. Once a writer is waiting, later readers wait
//! as well so a steady stream of compiler passes cannot starve a fix.
//!
//! The basic design here is to use a TCP server which is pretty portable across
//! platforms. For simplicity it just uses threads as well. Clients connect to
//! the main server, inform the server what its name is, and then wait for the
//! server to give it the lock and report the current write generation.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Error};

use crate::util::network::LOCALHOST;

pub struct LockServer {
    listener: TcpListener,
    addr: SocketAddr,
    threads: HashMap<String, ServerClient>,
    done: Arc<AtomicBool>,
}

pub struct LockServerStarted {
    done: Arc<AtomicBool>,
    addr: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

pub struct LockServerClient {
    _socket: TcpStream,
    generation: u64,
}

struct ServerClient {
    threads: Vec<JoinHandle<()>>,
    lock: Arc<(Mutex<LockState>, Condvar)>,
}

#[derive(Copy, Clone)]
enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Default)]
struct LockState {
    readers: usize,
    writer: bool,
    waiting_writers: usize,
    generation: u64,
}

impl LockServer {
    pub fn new() -> Result<LockServer, Error> {
        let listener = TcpListener::bind(&LOCALHOST[..])
            .context("failed to bind TCP listener to manage locking")?;
        let addr = listener.local_addr()?;
        Ok(LockServer {
            listener,
            addr,
            threads: HashMap::new(),
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn addr(&self) -> &SocketAddr {
        &self.addr
    }

    pub fn start(self) -> Result<LockServerStarted, Error> {
        let addr = self.addr;
        let done = self.done.clone();
        let thread = thread::spawn(|| {
            self.run();
        });
        Ok(LockServerStarted {
            addr,
            thread: Some(thread),
            done,
        })
    }

    fn run(mut self) {
        while let Ok((client, _)) = self.listener.accept() {
            if self.done.load(Ordering::SeqCst) {
                break;
            }

            // Learn the name of our connected client to figure out if it needs
            // to wait for another process to release the lock.
            let mut client = BufReader::new(client);
            let mut request = String::new();
            if client.read_line(&mut request).is_err() {
                continue;
            }
            let client = client.into_inner();
            let Some((mode, name)) = request.trim_end().split_once(' ') else {
                continue;
            };
            let mode = match mode {
                "shared" => LockMode::Shared,
                "exclusive" => LockMode::Exclusive,
                _ => continue,
            };
            let server_client =
                self.threads
                    .entry(name.to_string())
                    .or_insert_with(|| ServerClient {
                        threads: Vec::new(),
                        lock: Arc::new((Mutex::new(LockState::default()), Condvar::new())),
                    });
            server_client.threads.retain(|thread| !thread.is_finished());
            let lock = server_client.lock.clone();
            let lock2 = lock.clone();
            register(&lock, mode);
            let thread = thread::spawn(move || {
                let generation = acquire(&lock2, mode);
                let mut client = client;
                // Inform this client that it now has the lock and wait for it
                // to disconnect by waiting for EOF.
                if client.write_all(&generation.to_be_bytes()).is_ok() {
                    let mut dst = Vec::new();
                    drop(client.read_to_end(&mut dst));
                }
                release(&lock2, mode);
            });
            server_client.threads.push(thread);
        }
    }
}

fn register(lock: &Arc<(Mutex<LockState>, Condvar)>, mode: LockMode) {
    if let LockMode::Exclusive = mode {
        let (state, _) = &**lock;
        state.lock().unwrap().waiting_writers += 1;
    }
}

fn acquire(lock: &Arc<(Mutex<LockState>, Condvar)>, mode: LockMode) -> u64 {
    let (state, available) = &**lock;
    let mut state = state.lock().unwrap();
    match mode {
        LockMode::Shared => {
            state = available
                .wait_while(state, |state| state.writer || 0 < state.waiting_writers)
                .unwrap();
            state.readers += 1;
            state.generation
        }
        LockMode::Exclusive => {
            state = available
                .wait_while(state, |state| state.writer || 0 < state.readers)
                .unwrap();
            state.waiting_writers -= 1;
            state.writer = true;
            let generation = state.generation;
            state.generation += 1;
            generation
        }
    }
}

fn release(lock: &Arc<(Mutex<LockState>, Condvar)>, mode: LockMode) {
    let (state, available) = &**lock;
    let mut state = state.lock().unwrap();
    match mode {
        LockMode::Shared => state.readers -= 1,
        LockMode::Exclusive => state.writer = false,
    }
    available.notify_all();
}

impl Drop for LockServer {
    fn drop(&mut self) {
        for (_, mut client) in self.threads.drain() {
            for thread in client.threads.drain(..) {
                drop(thread.join());
            }
        }
    }
}

impl Drop for LockServerStarted {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        // Ignore errors here as this is largely best-effort
        if TcpStream::connect(&self.addr).is_err() {
            return;
        }
        drop(self.thread.take().unwrap().join());
    }
}

impl LockServerClient {
    pub fn lock(addr: &SocketAddr, name: impl AsRef<[u8]>) -> Result<LockServerClient, Error> {
        Self::lock_mode(addr, name, LockMode::Exclusive)
    }

    pub fn lock_shared(
        addr: &SocketAddr,
        name: impl AsRef<[u8]>,
    ) -> Result<LockServerClient, Error> {
        Self::lock_mode(addr, name, LockMode::Shared)
    }

    pub fn lock_exclusive(
        addr: &SocketAddr,
        name: impl AsRef<[u8]>,
    ) -> Result<LockServerClient, Error> {
        Self::lock_mode(addr, name, LockMode::Exclusive)
    }

    fn lock_mode(
        addr: &SocketAddr,
        name: impl AsRef<[u8]>,
        mode: LockMode,
    ) -> Result<LockServerClient, Error> {
        let mut client =
            TcpStream::connect(&addr).context("failed to connect to parent lock server")?;
        let mode = match mode {
            LockMode::Shared => b"shared ".as_slice(),
            LockMode::Exclusive => b"exclusive ".as_slice(),
        };
        client
            .write_all(mode)
            .and_then(|_| client.write_all(name.as_ref()))
            .and_then(|_| client.write_all(b"\n"))
            .context("failed to write to lock server")?;
        let mut buf = [0; std::mem::size_of::<u64>()];
        client
            .read_exact(&mut buf)
            .context("failed to acquire lock")?;
        Ok(LockServerClient {
            _socket: client,
            generation: u64::from_be_bytes(buf),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::{LockMode, LockServer, LockServerClient, LockState, acquire, register, release};
    use std::sync::{Arc, Condvar, Mutex};

    #[test]
    fn shared_locks_can_overlap() {
        let server = LockServer::new().unwrap();
        let addr = *server.addr();
        let _started = server.start().unwrap();
        let first = LockServerClient::lock_shared(&addr, "shared").unwrap();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let second = thread::spawn(move || {
            let _second = LockServerClient::lock_shared(&addr, "shared").unwrap();
            acquired_tx.send(()).unwrap();
        });

        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(first);
        second.join().unwrap();
    }

    #[test]
    fn exclusive_lock_waits_for_shared_lock() {
        let server = LockServer::new().unwrap();
        let addr = *server.addr();
        let _started = server.start().unwrap();
        let shared = LockServerClient::lock_shared(&addr, "exclusive").unwrap();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let exclusive = thread::spawn(move || {
            let _exclusive = LockServerClient::lock_exclusive(&addr, "exclusive").unwrap();
            acquired_tx.send(()).unwrap();
        });

        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(shared);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        exclusive.join().unwrap();
    }

    #[test]
    fn waiting_exclusive_lock_blocks_later_shared_lock() {
        let lock = Arc::new((Mutex::new(LockState::default()), Condvar::new()));
        acquire(&lock, LockMode::Shared);
        register(&lock, LockMode::Exclusive);
        let (exclusive_acquired_tx, exclusive_acquired_rx) = mpsc::channel();
        let (release_exclusive_tx, release_exclusive_rx) = mpsc::channel();
        let writer_lock = lock.clone();
        let exclusive = thread::spawn(move || {
            acquire(&writer_lock, LockMode::Exclusive);
            exclusive_acquired_tx.send(()).unwrap();
            release_exclusive_rx.recv().unwrap();
            release(&writer_lock, LockMode::Exclusive);
        });

        let (shared_acquired_tx, shared_acquired_rx) = mpsc::channel();
        let reader_lock = lock.clone();
        let later_shared = thread::spawn(move || {
            acquire(&reader_lock, LockMode::Shared);
            shared_acquired_tx.send(()).unwrap();
            release(&reader_lock, LockMode::Shared);
        });

        assert!(
            shared_acquired_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        release(&lock, LockMode::Shared);
        exclusive_acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(
            shared_acquired_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        release_exclusive_tx.send(()).unwrap();
        shared_acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        exclusive.join().unwrap();
        later_shared.join().unwrap();
    }

    #[test]
    fn generation_advances_for_exclusive_locks() {
        let server = LockServer::new().unwrap();
        let addr = *server.addr();
        let _started = server.start().unwrap();
        let shared = LockServerClient::lock_shared(&addr, "generation").unwrap();
        let preflight_generation = shared.generation();
        assert_eq!(preflight_generation, 0);
        drop(shared);
        let exclusive = LockServerClient::lock_exclusive(&addr, "generation").unwrap();
        assert_eq!(exclusive.generation(), 0);
        drop(exclusive);
        let later_exclusive = LockServerClient::lock_exclusive(&addr, "generation").unwrap();
        assert_eq!(later_exclusive.generation(), 1);
        assert_ne!(later_exclusive.generation(), preflight_generation);
    }
}
