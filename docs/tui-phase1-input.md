# TUI phase A: input routing

Based on the e7d0158 expert review, this phase fixes F01–F04 only.

- Recent `/` focuses its own filter. Characters (including `n`, spaces and `/`),
  Backspace/Delete, Left/Right, Home/End and Ctrl+U edit that filter. Enter,
  Escape, Tab and Shift+Tab leave input without activating a row. Query remains.
  While editing, page/action Alt shortcuts are inert; finish editing first.
  A subsequent list Escape retains the legacy return-to-Managed behavior.
- Recent and Project filter paste strips control characters and inserts text
  without submitting. Recent cursor positioning uses terminal display width
  and horizontal scrolling. Hidden Recent focus cannot consume Managed input.
- Horizontal confirmations default to Cancel. Left selects Cancel, Right
  selects Confirm, Tab/Shift+Tab traverse the two choices, Enter activates the
  selected choice, Escape cancels. Existing Up/Down support is retained.
- Project Create/Appearance text fields retain typed spaces, matching paste.
  Space cycles color only while the palette field is selected.
- Production key routing ignores Release and only permits Repeat for editing
  or navigation, never activation or action shortcuts. Runtime operations use
  the existing busy guard; Recent adoption consumes its review before emitting
  an effect. Project confirmation still uses existing synchronous typed calls.

The selector terminal loop and sequence tests share `route_selector_key`;
Projects tests enter the actual `handle_key`/`handle_paste` path. Contract tests
use the `ui_contract_` prefix. One exercises real authenticated HTTP handlers
and provider writes in temporary state; runtime effects are otherwise recorded
as intents, not executed. TestBackend checks a long Unicode filter cursor.

Not changed: root Escape behavior, list wrapping, default takeover, naming or
identity rules, stores, authority, retirement services, Tasks local behavior,
or terminal-shell lifecycle. Thread titles never supply or suggest a formal
Agent name. Full text-editor unification and phases B–E remain separate work.
PTY, Windows and live runtime acceptance are not established by these tests.
