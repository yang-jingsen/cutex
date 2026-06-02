# Docker Test Checklist

This checklist is for validating Docker runtime support used by `cutex`.

The current local runtime image source is:

- `images/cutex-base/Dockerfile`

## Image Requirements

The image must provide:

- `cute-codex` built into the image
- `cutex-codex` and `codex` available on `PATH` as compatibility entrypoints
- support for interactive TTY runs via `docker run -it`
- compatibility with `--user <uid:gid>`
- a writable mounted home directory at `/home/<user_name>`
- a readable mounted profile directory at `/home/<user_name>/.cutex-profiles`
- support for bind-mounting the project directory at the same absolute path as the host
- a recent `cute-codex` binary with Reqwest SOCKS support if SOCKS proxies are used

Recommended extras:

- `bash`
- `bubblewrap`
- `git`
- `ca-certificates`
- common CLI/build tools if prompts or workflows need them

## Build The Image

```bash
docker build -f images/cutex-base/Dockerfile -t cutex-base .
```

If Docker on this machine requires root privileges, use:

```bash
sudo docker build -f images/cutex-base/Dockerfile -t cutex-base .
```

## Test 1: Selected CLI Exists

```bash
docker run --rm -it cutex-base cute-codex --help
```

Pass conditions:

- container starts
- `cute-codex --help` prints normally
- no `command not found` error

## Test 2: Match Cutex Runtime Launch

This mirrors the important parts of `cutex`'s Docker launch path.

```bash
USER_NAME="${USER:-cutex}"
IMAGE="cutex-base"

docker run --rm -it \
  --user "$(id -u):$(id -g)" \
  -e HOME="/home/$USER_NAME" \
  -e USER="$USER_NAME" \
  -e LOGNAME="$USER_NAME" \
  -e CODEX_HOME="/home/$USER_NAME/.codex" \
  -v "$HOME/.cutex/runtime/docker-home:/home/$USER_NAME" \
  -v "$HOME/.cutex/profiles:/home/$USER_NAME/.cutex-profiles" \
  -v "$PWD:$PWD" \
  -w "$PWD" \
  "$IMAGE" \
  cute-codex --help
```

Pass conditions:

- `cute-codex --help` runs successfully
- no permission errors for `HOME` or `CODEX_HOME`
- no working-directory error for `-w "$PWD"`

## Test 3: Verify Mounted Home Is Writable

```bash
USER_NAME="${USER:-cutex}"
IMAGE="cutex-base"

docker run --rm -it \
  --user "$(id -u):$(id -g)" \
  -e HOME="/home/$USER_NAME" \
  -e USER="$USER_NAME" \
  -e LOGNAME="$USER_NAME" \
  -e CODEX_HOME="/home/$USER_NAME/.codex" \
  -v "$HOME/.cutex/runtime/docker-home:/home/$USER_NAME" \
  "$IMAGE" \
  bash -lc 'mkdir -p "$CODEX_HOME" && touch "$HOME/.write-test" "$CODEX_HOME/.codex-write-test" && ls -la "$HOME" "$CODEX_HOME"'
```

Cleanup:

```bash
rm -f ~/.cutex/runtime/docker-home/.write-test
rm -f ~/.cutex/runtime/docker-home/.codex/.codex-write-test
```

## Test 4: Configure A Docker Runtime Profile

```bash
cutex runtime <profile> --docker-image cutex-base --docker-user-name "$USER"
```

Pass conditions:

- command succeeds
- `cutex current` or `cutex list` shows the profile as `docker`

## Test 5: Run Through Cutex

If this machine needs `sudo docker`, set:

```bash
export CUTEX_DOCKER_USE_SUDO=1
```

Or save it once:

```bash
cutex global set --docker-use-sudo true
```

Then run:

```bash
cutex run <profile> -- --help
```

Pass conditions:

- `cutex` switches to the selected profile
- Docker starts successfully
- check the `CLI binary: ...` line to confirm which entrypoint `cutex` resolved

## Test 6: Host Override Still Works

```bash
cutex run <profile> --host -- --help
```

Pass conditions:

- invocation succeeds on the host
- stored runtime remains Docker after the command

## Test 7: Proxy Config Is Injected

Use a harmless local proxy address for this structural check. This verifies command construction and env propagation; it does not require the proxy endpoint to accept traffic.

```bash
cutex proxy set socks5h://127.0.0.1:7890 --no-proxy localhost,127.0.0.1
cutex proxy show <profile>
cutex run <profile> -- --help
```

Pass conditions:

- `cutex proxy show <profile>` reports the global or profile-specific effective proxy
- the Docker launch still starts successfully
- for real provider traffic, `socks5h://...` keeps DNS resolution on the proxy side
- default forced HTTP mode is active unless the proxy was configured with `--force-http false`

Cleanup:

```bash
cutex proxy clear
```

## Compatibility

- The old `CODEZ_DOCKER_USE_SUDO` environment variable is still accepted.
