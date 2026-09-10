# Human VM Job error inspection — prepared, not executed

Basec2d7a4ef6940e0c73b85deeea315dafbc1672dc0. Product remains c69241c,
native0c425, Jobf3bc9c8; exact bytes match reviewed-aemeath-r1/build-manifest-v2.
New guest root `/home/cutex-linux-test/acceptance-upload/hj1`; r3/r4 untouched.

## Three Human steps

1. From the Linux host Terminal, run:

   ```sh
   python3 /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/source/scripts/human_job_terminal.py
   ```

   Type `START` only when ready. This uses the existing SSH alias/config,
   copies only authorized aemeath auth into a private guest file, creates a NEW
   private reviewed runtime and opens pinned native CLI through
   `cutex session stock-attach <exact-durable-ID>`. It may take several minutes.
   The adapter resolves that exact runtime's canonical native thread and uses
   `resume --remote`; it does not create a second native owner. Terra/low,
   readOnly/on-request and reviewed Job configuration are retained.

2. Paste the ONE Job prompt printed before the CLI starts. Approve only the
   shown fixed Job submission and, if asked, that Job's stdout read. It permits
   Job-only tool_search/CodeMode, not direct shell/exec_command, polling, another
   model or automatic submit retry. Let normal completion arrive. Inspect the
   visible error; do not manually release/retry either old or new held input.

3. Exit the CLI (Ctrl+C as needed). Owned fixture cleanup then runs and removes
   copied auth; the host also attempts exact-file cleanup. The printed local
   `human-errors.jsonl` path contains redacted error descriptions. Inspect it in
   a VM terminal, or using the existing alias, e.g.:

   ```sh
   ssh -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance
   ```

   Then `cat` the exact printed path. Do not paste unreviewed raw text into
   Agent Bus/Task Service; request a reviewed summary when desired. If SSH
   disconnects and cleanup warns, remove only the two exact guest credential
   paths it prints. Do not remove host auth or entire guest fixture/history.

## Error handling and safety boundary

The observer subscribes read-only to the same native thread. It sends no turn,
tool, approval, retry, release or ACK. The CLI is the Human input/approval path.
`willRetry=true` is intermediate and remains visible without observer teardown;
`willRetry=false`/failed completion is terminal evidence. This interactive entry
does not automatically react to either by retrying or fabricating success.
Human controls when to exit; there is no automated paid campaign or prompt.

Only nested error fields are collected. Useful unknown `message` text survives;
additionalDetails/full requests/config/env/conversation are not retained.
Credential-labelled values, bearer/basic strings, JWT/key/opaque shapes, URL
contents and header lines are removed before owner-only0600 append. No auth
file is read by the observer. Redaction is conservative pattern filtering, not
a proof that arbitrary unlabelled prose cannot contain sensitive content:
files stay local/private and MUST be reviewed before sharing. No raw error text
is automatically forwarded to coordination services.

## Verification / omissions

Preparation only: no auth copied, model request, runtime setup/start/restart,
held release or provider call was made by this task. Guest staging transferred
scripts and frozen bytes; hashes matched Cutex908ab036, facade3b7b3736,
Jobd98d5e33 and base-fixtureed1ebdb5. Native bundle remains the frozen vm-r1 copy.
Eight synthetic redaction/extraction checks preserve unknown explanations and
remove sentinel credentials; AST/diff checks pass. Guest synthetic checks also
run without auth/runtime. No rebuild or repeated product suite.

Source inspection confirms supported stock-attach performs exact ready receipt,
runtime identity and generation checks and launches pinned CLI with reviewed
model/permissions, without a legacy sandbox override or K fallback. Previous
same-owner CLI evidence is reused; this NEW interactive composition remains
Human-untested. If attach fails, preserve the error and stop: do not manually
resume a second writer or replace the route with direct daemon/grant commands.

The actual submit/output/A4 partial passes and completion failure remain as in
r4. This entry does not diagnose their cause or complete real-provider
acceptance. No installer/service owner, host/profile change, auth syncback,
production mutation, release approval or new auth scheme is introduced.
