#!/usr/bin/env python3
"""Generate the Tauri updater manifest `latest.json` for a Windows release.

Usage:
  make_latest_json.py --version 0.2.0 --tag v0.2.0 --repo ottrboii/setup-coach \
      --installer "Setup Coach_0.2.0_x64-setup.exe" --sig-file "<installer>.sig" \
      [--notes "text"] [--out latest.json]

The download URL points at the release asset. GitHub replaces spaces in asset
names with dots, so `--asset-name` can override the name used in the URL
(default: the installer file name with spaces turned into dots).
"""
import argparse
import datetime
import json
import re
import sys
import urllib.parse

DEFAULT_NOTES = "Improvements and fixes."


def normalize_version(v: str) -> str:
    v = v.strip()
    if v.startswith("v"):
        v = v[1:]
    if not re.fullmatch(r"\d+\.\d+\.\d+([-+][0-9A-Za-z.\-+]+)?", v):
        raise ValueError(f"not a semantic version: {v!r}")
    return v


def asset_name(installer_file: str) -> str:
    """Name GitHub gives an uploaded asset (spaces become dots)."""
    return installer_file.replace(" ", ".")


def build_manifest(version, tag, repo, installer_file, signature, notes=None, pub_date=None, asset=None):
    version = normalize_version(version)
    signature = signature.strip()
    if not signature:
        raise ValueError("empty signature")
    name = asset or asset_name(installer_file)
    url = f"https://github.com/{repo}/releases/download/{urllib.parse.quote(tag)}/{urllib.parse.quote(name)}"
    if pub_date is None:
        pub_date = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    notes = (notes or "").strip() or DEFAULT_NOTES
    return {
        "version": version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": {
            "windows-x86_64": {"signature": signature, "url": url},
        },
    }


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--installer", required=True, help="installer file name (as built)")
    ap.add_argument("--sig-file", required=True)
    ap.add_argument("--asset-name")
    ap.add_argument("--notes", default="")
    ap.add_argument("--out", default="latest.json")
    a = ap.parse_args(argv)
    with open(a.sig_file, encoding="utf-8") as f:
        sig = f.read()
    m = build_manifest(a.version, a.tag, a.repo, a.installer, sig, a.notes, asset=a.asset_name)
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(m, f, indent=2)
        f.write("\n")
    print(f"wrote {a.out} for {m['version']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
