#!/usr/bin/env python3

"""Materialize per-account auth/config files from a legacy codez accounts store.

This reads the legacy `accounts.json` store and writes:

  <output-root>/<account-id>/auth.json
  <output-root>/<account-id>/config.toml

The script does not modify `accounts.json`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
from typing import Any


def default_store_path() -> Path:
    return Path.home() / ".codez-cli" / "accounts.json"


def default_output_root() -> Path:
    return Path.home() / ".codez-cli" / "profiles"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Materialize per-account auth/config files from a legacy codez accounts.json store."
    )
    parser.add_argument(
        "--store",
        type=Path,
        default=default_store_path(),
        help="Path to legacy accounts.json (default: %(default)s)",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=default_output_root(),
        help="Directory where per-account files are written (default: %(default)s)",
    )
    parser.add_argument(
        "--account",
        action="append",
        default=[],
        help="Limit migration to one or more account ids or names.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print intended changes without writing files.",
    )
    return parser.parse_args()


def utc_now_rfc3339() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace(
        "+00:00", "Z"
    )


def legacy_auth_json_from_auth_data(auth: dict[str, Any]) -> str:
    mode = auth.get("mode")
    if mode == "apikey":
        value = {
            "openai_api_key": auth.get("key"),
            "tokens": None,
            "last_refresh": None,
        }
    elif mode == "chatgpt":
        value = {
            "openai_api_key": None,
            "tokens": {
                "id_token": auth.get("id_token"),
                "access_token": auth.get("access_token"),
                "refresh_token": auth.get("refresh_token"),
                "account_id": auth.get("account_id"),
            },
            "last_refresh": utc_now_rfc3339(),
        }
    else:
        raise ValueError(f"unsupported legacy auth mode: {mode!r}")

    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def read_store(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fh:
        return json.load(fh)


def selected_accounts(accounts: list[dict[str, Any]], filters: list[str]) -> list[dict[str, Any]]:
    if not filters:
        return accounts
    wanted = set(filters)
    return [
        account
        for account in accounts
        if account.get("id") in wanted or account.get("name") in wanted
    ]


def normalize_text(value: str) -> str:
    if value.endswith("\n"):
        return value
    return value + "\n"


def write_text_if_changed(path: Path, contents: str, dry_run: bool) -> str:
    normalized = normalize_text(contents)
    existing = None
    if path.exists():
        existing = path.read_text(encoding="utf-8")
    if existing == normalized:
        return "unchanged"
    if dry_run:
        return "would-write"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(normalized, encoding="utf-8")
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass
    return "wrote"


def remove_file_if_present(path: Path, dry_run: bool) -> str:
    if not path.exists():
        return "absent"
    if dry_run:
        return "would-remove"
    path.unlink()
    return "removed"


def materialize_account(account: dict[str, Any], output_root: Path, dry_run: bool) -> tuple[str, str]:
    account_id = account.get("id")
    if not account_id:
        raise ValueError("account is missing id")

    auth_text = account.get("raw_auth_json")
    if not auth_text:
        auth_data = account.get("auth")
        if not isinstance(auth_data, dict):
            raise ValueError(f"account {account_id} has neither raw_auth_json nor auth")
        auth_text = legacy_auth_json_from_auth_data(auth_data)

    profile_dir = output_root / account_id
    auth_path = profile_dir / "auth.json"
    config_path = profile_dir / "config.toml"

    auth_status = write_text_if_changed(auth_path, auth_text, dry_run)

    raw_config_toml = account.get("raw_config_toml")
    if raw_config_toml:
        config_status = write_text_if_changed(config_path, raw_config_toml, dry_run)
    else:
        config_status = remove_file_if_present(config_path, dry_run)

    return auth_status, config_status


def main() -> int:
    args = parse_args()
    store = read_store(args.store)
    accounts = store.get("accounts")
    if not isinstance(accounts, list):
        raise SystemExit(f"invalid accounts store: {args.store}")

    targets = selected_accounts(accounts, args.account)
    if not targets:
        raise SystemExit("no matching accounts found")

    print(f"store: {args.store}")
    print(f"output-root: {args.output_root}")
    print(f"accounts: {len(targets)}")

    for account in targets:
        account_id = account.get("id", "<missing-id>")
        account_name = account.get("name", "<unnamed>")
        auth_status, config_status = materialize_account(account, args.output_root, args.dry_run)
        print(
            f"- {account_name} ({account_id}): auth={auth_status}, config={config_status}"
        )

    if args.dry_run:
        print("dry-run only; no files were changed")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
