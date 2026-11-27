#[cfg(not(target_os = "macos"))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

use super::domain::MonitorError;
use super::ports::{ProcessSignaler, SystemProbe};

pub fn default_probe() -> Box<dyn SystemProbe> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacosProbe::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(linux::LinuxProcProbe::new())
    }
}

pub struct PosixSignaler;

impl ProcessSignaler for PosixSignaler {
    fn signal(&self, pid: i32, sig: i32) -> Result<(), MonitorError> {
        if pid <= 1 {
            return Err(MonitorError::ProtectedPid(pid));
        }
        let res = unsafe { libc::kill(pid, sig) };
        if res == 0 {
            Ok(())
        } else {
            Err(MonitorError::SignalFailed {
                pid,
                reason: std::io::Error::last_os_error().to_string(),
            })
        }
    }
}

pub fn send_process_signal(pid: i32, sig: i32) -> Result<(), MonitorError> {
    PosixSignaler.signal(pid, sig)
}
