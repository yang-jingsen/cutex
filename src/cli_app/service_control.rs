//! Explicit Human service operations. Never invoked by runtime discovery.
use anyhow::{ensure, Context};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action { Status, Start, Stop, Restart }

#[derive(Clone, Copy, Debug)]
pub(super) enum Service { Bus, Management }

impl Service {
    pub(super) fn name(self) -> &'static str {
        match self { Self::Bus => "Agent Bus", Self::Management => "Management API" }
    }
    fn unit(self) -> &'static str {
        match self { Self::Bus => "cutex-agent-bus.service", Self::Management => "cutex-management-api.service" }
    }
}

pub(super) fn run(service: Service, action: Action) -> anyhow::Result<String> {
    let root = cutex::config::paths::runtime_dir()?.join("services");
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(root.join(format!("{}.lock", service.unit())))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("A service action is already running")?;
    run_inner(service, action)
}

fn run_inner(service: Service, action: Action) -> anyhow::Result<String> {
    #[cfg(target_os = "linux")]
    {
        let verb = match action { Action::Status => "show", Action::Start => "start", Action::Stop => "stop", Action::Restart => "restart" };
        let mut command = Command::new("systemctl");
        command.args(["--user", verb, service.unit()]);
        if action == Action::Status {
            command.args(["--property=LoadState,ActiveState,SubState,MainPID", "--no-pager"]);
        }
        let output = command.output().context("Cannot run systemctl --user")?;
        ensure!(output.status.success(), "{}: {}", service.name(), String::from_utf8_lossy(&output.stderr));
        if action != Action::Status { return run_inner(service, Action::Status); }
        Ok(format!("{}\n{}", service.name(), String::from_utf8_lossy(&output.stdout).trim()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let config = cutex::config::store::load_codez_config_checked()?;
        let (role, port) = match service {
            Service::Bus => ("agent", config.agent_bus_port.unwrap_or(24260)),
            Service::Management => ("management", cutex::management::service::DEFAULT_MANAGEMENT_PORT),
        };
        let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
        let exe = quote(&std::env::current_exe()?.to_string_lossy());
        let stop = matches!(action, Action::Stop | Action::Restart);
        let start = matches!(action, Action::Start | Action::Restart);
        let script = format!(r#"
$ErrorActionPreference='Stop'
$listeners=@(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
foreach($servicePid in $listeners) {{
  $process=Get-CimInstance Win32_Process -Filter "ProcessId=$servicePid"
  if (!$process -or [IO.Path]::GetFileName($process.ExecutablePath) -ne 'cutex.exe' -or $process.CommandLine -notmatch '\b{role}\s+serve\b') {{ throw 'Port belongs to another process; no service action performed' }}
}}
if ({stop}) {{ foreach($servicePid in $listeners) {{ Stop-Process -Id $servicePid -ErrorAction Stop; Wait-Process -Id $servicePid -Timeout 15 -ErrorAction SilentlyContinue; if(Get-Process -Id $servicePid -ErrorAction SilentlyContinue) {{ throw 'Service has not stopped' }} }}; $listeners=@() }}
if ({start} -and $listeners.Count -eq 0) {{
  $created=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{CommandLine=('"'+{exe}+'" {role} serve --port {port}')}}
  if($created.ReturnValue -ne 0) {{ throw ('Service creation failed: '+$created.ReturnValue) }}
  for($attempt=0;$attempt -lt 40;$attempt++) {{
    $listeners=@(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
    if($listeners.Count -gt 0) {{ break }}
    Start-Sleep -Milliseconds 250
  }}
  if($listeners.Count -eq 0) {{ throw 'Service did not listen; run cutex {role} serve --port {port} manually for diagnostics' }}
}}
if($listeners.Count -eq 0) {{ 'Stopped' }} else {{ 'Listening: PID '+($listeners -join ', ') }}
"#, stop=if stop { "$true" } else { "$false" }, start=if start { "$true" } else { "$false" });
        let output = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000).output()?;
        ensure!(output.status.success(), "{}: {}", service.name(), String::from_utf8_lossy(&output.stderr));
        Ok(format!("{}\n{}", service.name(), String::from_utf8_lossy(&output.stdout).trim()))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    anyhow::bail!("Service controls currently support Linux and Windows")
}
