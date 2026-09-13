//! Package-private Linux containment helper.
//!
//! The host gives each sentinel the read side of a fresh socket pair on stdin.
//! EOF identifies the exact host occurrence without consulting a reusable PID.

#[cfg(target_os = "linux")]
use clap::Parser;
#[cfg(target_os = "linux")]
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
#[derive(Parser)]
#[command(name = "prh-linux-sentinel", hide = true)]
struct Args {
    #[arg(long)]
    process_group: i32,
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("prh-linux-sentinel: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run(args: Args) -> Result<(), String> {
    if args.process_group <= 1 {
        return Err("refusing unsafe process-group ID".to_owned());
    }
    if !process_group_exists(args.process_group) {
        return Err("service process group does not exist".to_owned());
    }
    set_stdin_nonblocking()?;
    println!("READY");
    std::io::stdout()
        .flush()
        .map_err(|error| format!("failed to acknowledge readiness: {error}"))?;

    let mut probe = [0_u8; 1];
    loop {
        if !process_group_exists(args.process_group) {
            return Ok(());
        }
        match std::io::stdin().read(&mut probe) {
            Ok(0) => return kill_contained_group(args.process_group),
            Ok(_) => {
                return Err("host liveness channel contained unexpected data".to_owned());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(format!("host liveness channel failed: {error}"));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
fn set_stdin_nonblocking() -> Result<(), String> {
    // SAFETY: fcntl operates on the process-owned stdin descriptor and does
    // not dereference memory.
    let flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL) };
    if flags == -1 {
        return Err(format!(
            "failed to inspect host liveness channel: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: the retrieved flags are reused with O_NONBLOCK added.
    if unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(format!(
            "failed to make host liveness channel nonblocking: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn kill_contained_group(process_group: i32) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_group_exists(process_group) && Instant::now() < deadline {
        signal_process_group(process_group, libc::SIGKILL)
            .map_err(|error| format!("failed to clean contained process group: {error}"))?;
        std::thread::sleep(Duration::from_millis(10));
    }
    if process_group_exists(process_group) {
        Err("contained process group survived the cleanup deadline".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn signal_process_group(process_group: i32, signal: i32) -> std::io::Result<()> {
    // SAFETY: the caller validates a positive process-group ID and uses a
    // constant signal. kill does not dereference memory.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(target_os = "linux")]
fn process_group_exists(process_group: i32) -> bool {
    // SAFETY: signal zero only checks existence and permission.
    let result = unsafe { libc::kill(-process_group, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("prh-linux-sentinel is available only on Linux");
    std::process::exit(2);
}
