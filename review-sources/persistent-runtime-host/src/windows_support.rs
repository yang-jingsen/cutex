//! Platform-neutral pieces of the Windows launch and tray contract.
//!
//! Keeping quoting, environment ordering, endpoint derivation, and the
//! suspended-launch gate portable lets the Linux CI host exercise the exact
//! decisions consumed by the Windows-only backend.

use std::collections::BTreeMap;

pub const WINDOWS_INHERITED_HANDLE_ROLES: [&str; 3] = ["stdin", "stdout", "stderr"];

/// Quote one argument using the parsing rules used by the Microsoft C runtime
/// and `CommandLineToArgvW`-compatible applications.
pub fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return argument.to_owned();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0_usize;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        if character == '"' {
            quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            quoted.push('"');
        } else {
            quoted.extend(std::iter::repeat_n('\\', backslashes));
            quoted.push(character);
        }
        backslashes = 0;
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

pub fn build_windows_command_line(executable: &str, arguments: &[String]) -> String {
    std::iter::once(executable)
        .chain(arguments.iter().map(String::as_str))
        .map(quote_windows_argument)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Merge the controlled base environment with definition overrides using
/// Windows' case-insensitive variable-name semantics.
pub fn merge_windows_environment(
    base: impl IntoIterator<Item = (String, String)>,
    additions: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut merged = BTreeMap::<String, (String, String)>::new();
    for (name, value) in base {
        merged.insert(name.to_lowercase(), (name, value));
    }
    for (name, value) in additions {
        merged.insert(name.to_lowercase(), (name.clone(), value.clone()));
    }
    merged.into_values().collect()
}

/// Encode a sorted Windows Unicode environment block, including its required
/// double-NUL terminator.
pub fn encode_windows_environment(entries: &[(String, String)]) -> Vec<u16> {
    let mut block = Vec::new();
    for (name, value) in entries {
        block.extend(name.encode_utf16());
        block.push('=' as u16);
        block.extend(value.encode_utf16());
        block.push(0);
    }
    block.push(0);
    if entries.is_empty() {
        block.push(0);
    }
    block
}

/// Derive a stable per-state-directory named-pipe endpoint. The name is not a
/// security token; the server applies an owner-only DACL separately.
pub fn windows_pipe_name_for_state(state_directory: &str) -> String {
    let mut normalized = state_directory.replace('/', "\\").to_lowercase();
    while normalized.len() > 3 && normalized.ends_with('\\') {
        normalized.pop();
    }
    let identity = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("prh-state:{normalized}").as_bytes(),
    );
    format!(r"\\.\pipe\persistent-runtime-host-v1-{identity}")
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchPhase {
    Prepared,
    CreatedSuspended,
    AssignedToJob,
    ResumeAuthorized,
}

/// A fail-closed ordering guard used around the unsafe Windows launch calls.
/// It makes it impossible for the backend's resume path to run before a
/// successful suspended creation and Job assignment.
#[cfg(any(test, target_os = "windows"))]
#[derive(Debug)]
pub(crate) struct WindowsLaunchGate {
    phase: LaunchPhase,
}

#[cfg(any(test, target_os = "windows"))]
impl WindowsLaunchGate {
    pub(crate) fn new() -> Self {
        Self {
            phase: LaunchPhase::Prepared,
        }
    }

    pub(crate) fn process_created_suspended(&mut self) -> Result<(), &'static str> {
        if self.phase != LaunchPhase::Prepared {
            return Err("suspended process creation recorded out of order");
        }
        self.phase = LaunchPhase::CreatedSuspended;
        Ok(())
    }

    pub(crate) fn assigned_to_job(&mut self) -> Result<(), &'static str> {
        if self.phase != LaunchPhase::CreatedSuspended {
            return Err("Job assignment recorded before suspended creation");
        }
        self.phase = LaunchPhase::AssignedToJob;
        Ok(())
    }

    pub(crate) fn authorize_resume(&mut self) -> Result<(), &'static str> {
        if self.phase != LaunchPhase::AssignedToJob {
            return Err("refusing to resume a process not assigned to its Job");
        }
        self.phase = LaunchPhase::ResumeAuthorized;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_gate_refuses_resume_before_job_assignment() {
        let mut gate = WindowsLaunchGate::new();
        assert!(gate.authorize_resume().is_err());
        gate.process_created_suspended().unwrap();
        assert!(gate.authorize_resume().is_err());
        gate.assigned_to_job().unwrap();
        gate.authorize_resume().unwrap();
    }
}
