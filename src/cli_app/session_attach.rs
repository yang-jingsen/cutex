use anyhow::Context;

use super::management_focus;

use cutex::runtime::alden::cute_alden_attach_plan;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

pub(crate) fn cmd_session_attach(name: &str, takeover: bool) -> anyhow::Result<()> {
    let plan = cute_alden_attach_plan(name, takeover)?;

    if let Err(err) = management_focus::append_pc_attach_focus_event(&plan.session_name, takeover) {
        eprintln!("{YELLOW}warning:{RESET} failed to record PC attach focus: {err:#}");
    }

    let mut command = plan.command();
    let exit_code = exit_code_from_status(command.status().with_context(|| {
        let mode = if takeover { " --takeover" } else { "" };
        format!(
            "Failed to start {} --attach {}{mode}",
            plan.program, plan.session_name
        )
    })?);

    std::process::exit(exit_code);
}

fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
