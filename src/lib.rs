#[cfg(not(target_os = "linux"))]
compile_error!("pressure is only supported on Linux-based operating systems");

use std::{
    fs::OpenOptions,
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::fs::{FileTypeExt, OpenOptionsExt},
    },
};

use base64::Engine;
use nix::{
    libc::O_NONBLOCK,
    poll::{PollFd, PollFlags, PollTimeout},
};
use tokio::io::Interest;

pub struct PressureMonitor {
    pressure_file: MonitorType,
}

impl PressureMonitor {
    pub fn new() -> Self {
        let pressure_file = init_monitor();
        Self { pressure_file }
    }
    pub fn wait(&mut self) {
        match &self.pressure_file {
            MonitorType::File(owned_fd) => {
                nix::poll::poll(
                    &mut [PollFd::new(owned_fd.as_fd(), PollFlags::POLLPRI)],
                    PollTimeout::NONE,
                )
                .unwrap();
            }
            MonitorType::Fifo(owned_fd) | MonitorType::Socket(owned_fd) => {}
        }
        nix::poll::poll(
            &mut [PollFd::new(self.pressure_file.as_fd(), PollFlags::POLLPRI)],
            PollTimeout::NONE,
        )
        .unwrap();
    }
    // #[cfg(feature = "tokio")]
    // pub async fn wait_async(&mut self) {
    //     tokio::io::unix::AsyncFd::with_interest(
    //         self.pressure_file.try_clone().unwrap(),
    //         Interest::PRIORITY,
    //     )
    //     .unwrap()
    //     .ready(Interest::PRIORITY)
    //     .await
    //     .unwrap()
    //     .clear_ready();
    // }
}

enum MonitorType {
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

fn init_monitor() -> MonitorType {
    let source =
        std::env::var("MEMORY_PRESSURE_WATCH").unwrap_or_else(|_| "/proc/pressure/memory".into());
    let mut pressure_file = OpenOptions::new()
        .custom_flags(O_NONBLOCK)
        .write(true)
        .open(source)
        .unwrap();
    match std::env::var("MEMORY_PRESSURE_WRITE") {
        Ok(var) => pressure_file
            .write_all(&base64::prelude::BASE64_STANDARD.decode(&var).unwrap())
            .unwrap(),
        Err(std::env::VarError::NotPresent) => {
            pressure_file.write_all(b"some 20000 2000000\x00").unwrap()
        }
        Err(e) => Err(e).unwrap(),
    }
    let file_type = pressure_file.metadata().unwrap().file_type();
    let pressure_fd: OwnedFd = pressure_file.into();
    if file_type.is_file() {
        MonitorType::File(pressure_fd)
    } else if file_type.is_fifo() {
        MonitorType::Fifo(pressure_fd)
    } else if file_type.is_socket() {
        MonitorType::Socket(pressure_fd)
    } else {
        todo!();
    }
}
