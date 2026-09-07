//! The owned stdio host must be able to stop even when its consumer stops reading.
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

pub fn write_frames(receiver: mpsc::Receiver<Value>, stop: &CancellationToken) -> io::Result<()> {
    #[cfg(unix)]
    let mut output = {
        use std::os::fd::{AsFd, AsRawFd};
        let descriptor = std::io::stdout().as_fd().try_clone_to_owned()?;
        // SAFETY: descriptor is owned and live for both fcntl calls. Preserve
        // existing flags and use no variadic pointer arguments.
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe {
                libc::fcntl(
                    descriptor.as_raw_fd(),
                    libc::F_SETFL,
                    flags | libc::O_NONBLOCK,
                )
            } < 0
        {
            return Err(io::Error::last_os_error());
        }
        std::fs::File::from(descriptor)
    };
    // Only Linux has lifecycle acceptance evidence. Other platforms still need
    // their own cancellable pipe implementation before advertising support.
    #[cfg(not(unix))]
    let mut output = std::io::stdout();

    for value in receiver {
        let mut line = value.to_string();
        if line.len() > super::MAX_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output frame too large",
            ));
        }
        line.push('\n');
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut remaining = line.as_bytes();
        while !remaining.is_empty() {
            match output.write(remaining) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => remaining = &remaining[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if stop.is_cancelled() || std::time::Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "consumer stopped reading host output",
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => return Err(error),
            }
        }
        output.flush()?;
    }
    Ok(())
}
