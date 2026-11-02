//! Feature reports over hidraw (`HIDIOCSFEATURE` / `HIDIOCGFEATURE`).

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

/// `_IOC(_IOC_WRITE|_IOC_READ, 'H', nr, len)`
const fn ioc(nr: u64, len: usize) -> u64 {
    (3 << 30) | ((len as u64) << 16) | ((b'H' as u64) << 8) | nr
}

pub fn set_feature(dev: &File, data: &[u8]) -> io::Result<()> {
    // SAFETY: the kernel reads `data.len()` bytes from the pointer, which matches the ioctl size.
    let r = unsafe { libc::ioctl(dev.as_raw_fd(), ioc(0x06, data.len()) as _, data.as_ptr()) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// `buf[0]` must hold the report ID; the kernel fills in the rest.
pub fn get_feature(dev: &File, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: the kernel writes at most `buf.len()` bytes to the pointer.
    let r = unsafe { libc::ioctl(dev.as_raw_fd(), ioc(0x07, buf.len()) as _, buf.as_mut_ptr()) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r as usize) }
}
