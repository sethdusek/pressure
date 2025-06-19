//! Pressure monitoring library, using Pressure Stall Information (PSI) On Linux.
//! # Example:
//! ```
//! use pressure::PressureMonitor;
//! fn main() {
//!     let mut monitor = PressureMonitor::new().unwrap();
//!     std::thread::spawn(move || {
//!         loop {
//!             monitor.wait().unwrap();
//!             // Code to handle pressure event, for example by calling malloc_trim(), or dropping caches
//!         }
//!     });
//! }
//! ```
#[cfg(not(target_os = "linux"))]
compile_error!("pressure is only supported on Linux-based operating systems");

use std::{
    env::VarError,
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::{fs::FileTypeExt, net::UnixStream},
    },
};

use base64::Engine;
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("nix error: {0}")]
    Nix(#[from] nix::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    VarError(#[from] VarError),
    #[error("expected regular file, fifo or socket, got something else")]
    UnexpectedFileType,
}

/// Represents a pressure monitor that can be used to wait for memory pressure events
pub struct PressureMonitor {
    pressure_file: MonitorType,
}

impl PressureMonitor {
    pub fn new() -> Result<Self, Error> {
        let pressure_file = init_monitor()?;
        Ok(Self { pressure_file })
    }
    /// Wait for a single pressure event to occur.
    /// It is safe to call this function in a busy loop, as even if memory pressure persists the kernel limits the amount of events sent
    pub fn wait(&mut self) -> Result<(), Error> {
        let (pollflag, needs_read) = match &self.pressure_file {
            MonitorType::File(_) => (PollFlags::POLLPRI, false),
            MonitorType::Fifo(_) | MonitorType::Socket(_) => (PollFlags::POLLIN, true),
        };
        nix::poll::poll(
            &mut [PollFd::new(self.pressure_file.as_fd(), pollflag)],
            PollTimeout::NONE,
        )
        .unwrap();
        if needs_read {
            let mut buf = [0; 1024];
            match nix::unistd::read(self.pressure_file.as_fd(), &mut buf) {
                Ok(_) => {}
                Err(Errno::EWOULDBLOCK) => {}
                Err(e) => Err(e)?,
            }
        }
        Ok(())
    }
}

#[cfg(feature = "tokio")]
pub mod tokio {
    //! Asynchronous pressure monitoring using Tokio's event loop
    use std::os::fd::AsFd;

    use nix::errno::Errno;
    use tokio::io::{Interest, unix::AsyncFd};

    use crate::{Error, MonitorType, init_monitor};

    /// Asynchronous equivalent to [PressureMonitor](`super::PressureMonitor`)
    pub struct PressureMonitor {
        pressure_file: AsyncFd<MonitorType>,
    }

    impl PressureMonitor {
        pub fn new() -> Result<Self, Error> {
            let pressure_file = init_monitor()?;
            Ok(Self {
                pressure_file: AsyncFd::new(pressure_file)?,
            })
        }

        /// Wait for a single pressure event to occur.
        /// It is safe to call this function in a busy loop, as even if memory pressure persists the kernel limits the amount of events sent
        pub async fn wait(&mut self) -> Result<(), Error> {
            let (pollflag, needs_read) = match self.pressure_file.get_ref() {
                MonitorType::File(_) => (Interest::PRIORITY, false),
                MonitorType::Fifo(_) | MonitorType::Socket(_) => (Interest::READABLE, true),
            };
            self.pressure_file.ready(pollflag).await?.clear_ready();
            if needs_read {
                let mut buf = [0; 512];
                match nix::unistd::read(self.pressure_file.get_ref().as_fd(), &mut buf) {
                    Ok(_) => {}
                    Err(Errno::EWOULDBLOCK) => {}
                    Err(e) => Err(e)?,
                }
            }
            Ok(())
        }
    }
}

pub(crate) enum MonitorType {
    File(OwnedFd),
    Fifo(OwnedFd),
    Socket(OwnedFd),
}

impl AsFd for MonitorType {
    fn as_fd(&self) -> BorrowedFd {
        match self {
            MonitorType::File(owned_fd) => owned_fd.as_fd(),
            MonitorType::Fifo(owned_fd) => owned_fd.as_fd(),
            MonitorType::Socket(owned_fd) => owned_fd.as_fd(),
        }
    }
}

impl AsRawFd for MonitorType {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        match self {
            MonitorType::File(fd) => fd.as_raw_fd(),
            MonitorType::Fifo(fd) => fd.as_raw_fd(),
            MonitorType::Socket(fd) => fd.as_raw_fd(),
        }
    }
}

const DEFAULT_PRESSURE: &[u8; 19] = b"some 20000 2000000\x00";

fn init_monitor() -> Result<MonitorType, Error> {
    let source = std::env::var("MEMORY_PRESSURE_WATCH");
    let (path, write) = match source.as_deref() {
        // Systemd sets MEMORY_PRESSURE_WATCH to /dev/null to indicate memory pressure monitoring is disabled for this service/unit
        // Instead of disabling memory pressure handling entirely we instead default to /proc/pressure/memory
        Ok("/dev/null") | Err(VarError::NotPresent) => {
            ("/proc/pressure/memory".into(), DEFAULT_PRESSURE.into())
        }
        Ok(path) => match std::env::var("MEMORY_PRESSURE_WRITE") {
            Ok(write) => {
                let write = base64::prelude::BASE64_STANDARD.decode(&write).unwrap();
                (path, write)
            }
            Err(_) => (path, Vec::new()),
        },
        Err(e) => Err(e.clone())?,
    };

    let file_type = std::fs::metadata(&path)?.file_type();

    if file_type.is_file() || file_type.is_fifo() {
        let fd = nix::fcntl::open(
            &path[..],
            nix::fcntl::OFlag::O_RDWR
                | nix::fcntl::OFlag::O_CLOEXEC
                | nix::fcntl::OFlag::O_NONBLOCK,
            nix::sys::stat::Mode::empty(),
        )?;
        nix::unistd::write(&fd, &write)?;
        if file_type.is_file() {
            Ok(MonitorType::File(fd))
        } else {
            Ok(MonitorType::Fifo(fd))
        }
    } else if file_type.is_socket() {
        let mut stream = UnixStream::connect(&path[..])?;
        stream.set_nonblocking(true)?;
        stream.write_all(&write)?;
        let fd: OwnedFd = stream.into();
        Ok(MonitorType::Socket(fd))
    } else {
        Err(Error::UnexpectedFileType)
    }
}
