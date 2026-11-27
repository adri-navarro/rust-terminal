//! Raw POSIX TTY termios guard and non-buffered stdin reader for the built-in shell.

use std::io::{self, Read};
use std::os::fd::AsRawFd;

pub struct RawModeGuard {
    fd: i32,
    orig: Option<libc::termios>,
}

impl RawModeGuard {
    pub fn enter() -> Self {
        let fd = io::stdin().as_raw_fd();
        unsafe {
            let mut orig: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut orig) != 0 {
                return Self { fd, orig: None };
            }
            let mut raw = orig;
            raw.c_iflag &= !(libc::ICRNL | libc::IXON | libc::BRKINT | libc::INPCK | libc::ISTRIP);
            raw.c_cflag |= libc::CS8;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(fd, libc::TCSADRAIN, &raw);
            Self {
                fd,
                orig: Some(orig),
            }
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(orig) = self.orig {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSADRAIN, &orig);
            }
        }
    }
}

pub struct RawStdin {
    fd: i32,
}

impl RawStdin {
    pub fn new() -> Self {
        Self {
            fd: io::stdin().as_raw_fd(),
        }
    }

    pub fn has_data(&self, timeout_ms: i32) -> bool {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut pfd, 1, timeout_ms) > 0 }
    }
}

impl Read for RawStdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as usize);
        }
    }
}
