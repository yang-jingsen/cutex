#[cfg(any(target_os = "linux", target_os = "windows"))]
use clap::Parser;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::io::{Read, Write};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::net::TcpListener;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::{Child, Command, Stdio};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Parser, Debug)]
#[command(name = "prh-fixture-service", hide = true)]
struct Args {
    #[arg(long)]
    tcp_port: Option<u16>,
    #[arg(long, default_value_t = 25)]
    heartbeat_ms: u64,
    #[arg(long, default_value_t = 32)]
    payload_bytes: usize,
    #[arg(long)]
    pid_file: Option<PathBuf>,
    #[arg(long)]
    child_pid_file: Option<PathBuf>,
    #[arg(long)]
    ignore_term: bool,
    #[arg(long)]
    exit_after_ms: Option<u64>,
    #[arg(long, default_value_t = 0)]
    exit_code: i32,
    #[arg(long, hide = true)]
    child: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => std::process::exit(args.exit_code),
        Err(error) => {
            eprintln!("fixture-error: {error}");
            std::process::exit(70);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let terminate = Termination::install()?;
    if let Some(path) = &args.pid_file {
        std::fs::write(path, format!("{}\n", std::process::id()))?;
    }

    let mut child = if !args.child {
        spawn_child(args)?
    } else {
        None
    };
    let listener = if let Some(port) = args.tcp_port {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        Some(listener)
    } else {
        None
    };
    println!("fixture-ready pid={}", std::process::id());
    eprintln!("fixture-stderr-ready pid={}", std::process::id());
    std::io::stdout().flush()?;
    std::io::stderr().flush()?;

    let started = Instant::now();
    let payload = "x".repeat(args.payload_bytes.min(16 * 1024));
    let mut heartbeat = 0_u64;
    loop {
        if !args.ignore_term && terminate.requested() {
            break;
        }
        if args
            .exit_after_ms
            .is_some_and(|milliseconds| started.elapsed() >= Duration::from_millis(milliseconds))
        {
            break;
        }
        if let Some(listener) = &listener {
            accept_ready_connections(listener)?;
        }
        println!("fixture-out {heartbeat} {payload}");
        eprintln!("fixture-err {heartbeat} {payload}");
        std::io::stdout().flush()?;
        std::io::stderr().flush()?;
        heartbeat = heartbeat.saturating_add(1);
        std::thread::sleep(Duration::from_millis(args.heartbeat_ms.max(1)));
    }
    if let Some(child) = &mut child {
        let _ = child.wait();
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn spawn_child(args: &Args) -> Result<Option<Child>, Box<dyn std::error::Error>> {
    let Some(child_pid_file) = &args.child_pid_file else {
        return Ok(None);
    };
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--child")
        .arg("--heartbeat-ms")
        .arg(args.heartbeat_ms.to_string())
        .arg("--payload-bytes")
        .arg(args.payload_bytes.to_string())
        .arg("--pid-file")
        .arg(child_pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if args.ignore_term {
        command.arg("--ignore-term");
    }
    Ok(Some(command.spawn()?))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn accept_ready_connections(listener: &TcpListener) -> std::io::Result<()> {
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_millis(25)))?;
                let mut request = [0_u8; 1_024];
                let _ = stream.read(&mut request);
                stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                )?;
                stream.flush()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("prh-fixture-service is available only for isolated platform tests");
    std::process::exit(2);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
struct Termination {
    #[cfg(target_os = "linux")]
    flag: Arc<AtomicBool>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl Termination {
    fn install() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(target_os = "linux")]
        {
            let flag = Arc::new(AtomicBool::new(false));
            signal_hook::flag::register(signal_hook::consts::SIGTERM, flag.clone())?;
            signal_hook::flag::register(signal_hook::consts::SIGINT, flag.clone())?;
            Ok(Self { flag })
        }
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
            WINDOWS_TERMINATE.store(false, Ordering::Relaxed);
            if unsafe { SetConsoleCtrlHandler(Some(windows_console_event), 1) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self {})
        }
    }

    fn requested(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.flag.load(Ordering::Relaxed)
        }
        #[cfg(target_os = "windows")]
        {
            WINDOWS_TERMINATE.load(Ordering::Relaxed)
        }
    }
}

#[cfg(target_os = "windows")]
static WINDOWS_TERMINATE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_console_event(event: u32) -> i32 {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };
    if matches!(
        event,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        WINDOWS_TERMINATE.store(true, Ordering::Relaxed);
        1
    } else {
        0
    }
}
