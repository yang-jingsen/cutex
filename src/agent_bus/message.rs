//! Agent bus message presentation helpers.

pub fn format_agent_message_content(
    prefix_template: Option<&str>,
    suffix_template: Option<&str>,
    from: &str,
    to: &str,
    content: &str,
) -> String {
    let prefix = render_agent_message_template(prefix_template.unwrap_or(""), from, to);
    let suffix = render_agent_message_template(suffix_template.unwrap_or(""), from, to);
    format!("{prefix}{content}{suffix}")
}

pub fn render_agent_message_template(template: &str, from: &str, to: &str) -> String {
    template.replace("{from}", from).replace("{to}", to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_message_template_wraps_content() {
        let content = format_agent_message_content(
            Some("[message from {from}] "),
            Some(" [to {to}]"),
            "agent.ceo",
            "aria-it.124f234",
            "please inspect logs",
        );

        assert_eq!(
            content,
            "[message from agent.ceo] please inspect logs [to aria-it.124f234]"
        );
    }
}
