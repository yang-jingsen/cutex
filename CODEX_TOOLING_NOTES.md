# Codex Tooling Notes

Already planned:
- rg
- fd-find (alias `fd`)
- jq

Additional tools that would materially improve Codex's effectiveness in this image:
- curl
  Needed for quick fetches, health checks, and downloading small artifacts.
- unzip
  Common dependency for inspecting downloaded archives.
- zip
  Useful when packaging outputs or fixtures.
- less
  Makes long command output and file inspection practical.
- file
  Helps identify binary/text content quickly.
- xxd
  Useful for inspecting binary files and odd encodings.
- tree
  Fast directory-shape inspection when `fd` output is too flat.

Optional but nice:
- fzf
  Speeds up interactive file/path selection.
- lsof
  Useful for debugging file/port usage.
- iproute2
  Provides `ss` for socket inspection.
- netcat-openbsd
  Provides `nc` for quick connectivity checks.

Not needed just for Codex editing flow in this image:
- cargo/rustc, unless the image should build Rust projects itself
- docker client inside the container, unless nested container workflows are intentional
- full editors like vim/nano, because Codex edits files directly
