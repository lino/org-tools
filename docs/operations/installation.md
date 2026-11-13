<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Multi-Platform Installation & Verification

## Overview

**What you will learn:**

- Install the standalone `org` Command-Line Interface (`CLI`) across macOS, Linux, Windows, and containers.
- Verify SHA-256 package digests and clear macOS Gatekeeper quarantine attributes.
- Configure shell completions and Unix `roff` (`ROFF`) manual pages (`man 1 org` and `man 5 org-tools.toml`).

Standardizing `org` installation across developer workstations and Continuous Integration (`CI`) runners eliminates Emacs runtime drift. Prebuilt static binaries start in milliseconds and require zero external language runtimes. Every operator and automation pipeline executes identical formatting, linting, and query rules.

## Prerequisites

- A 64-bit macOS, Linux, or Windows host (`x86_64` or `aarch64` / `arm64`).
- Standard SHA-256 digest tools (`sha256sum`, `shasum`, or PowerShell `Get-FileHash`).
- Optional package manager or toolchain: Homebrew, `dpkg`, `rpm`, Winget, Scoop, Docker, or Rust 1.83+ with `mise`.

## Steps

1. Install the prebuilt binary, shell completions, and manual pages from the Homebrew tap on macOS or Linux.

```sh
brew install --cask lino/tap/org-tools
brew upgrade --cask lino/tap/org-tools
```

2. Install static `musl` system packages on Debian, Ubuntu, Fedora, RHEL, or openSUSE after verifying `checksums.txt`.

```sh
sha256sum --ignore-missing -c checksums.txt

# Debian & Ubuntu (.deb)
sudo dpkg -i org-tools_<ver>_linux_amd64.deb
sudo dpkg -i org-tools_<ver>_linux_arm64.deb

# Fedora, RHEL & openSUSE (.rpm)
sudo rpm -Uvh org-tools-<ver>-1.x86_64.rpm
sudo rpm -Uvh org-tools-<ver>-1.aarch64.rpm
```

3. Install on Windows using Winget, Scoop, or a checksum-verified PowerShell `.zip` release archive.

```powershell
# Option A: Windows Package Manager (Winget) or Scoop
winget install lino.org-tools
scoop install org-tools

# Option B: Precompiled ZIP archive
Get-FileHash .\org-tools_<ver>_windows_amd64.zip -Algorithm SHA256
Get-Content .\checksums.txt | Select-String "windows_amd64.zip"
Expand-Archive .\org-tools_<ver>_windows_amd64.zip -DestinationPath "$HOME\bin\org-tools"
```

4. Extract a precompiled release tarball on macOS or Linux when system package managers are unavailable.

| Platform              | Target Triple                | Archive Asset                         |
| --------------------- | ---------------------------- | ------------------------------------- |
| macOS (Apple Silicon) | `aarch64-apple-darwin`       | `org-tools_<ver>_darwin_arm64.tar.gz` |
| macOS (Intel)         | `x86_64-apple-darwin`        | `org-tools_<ver>_darwin_amd64.tar.gz` |
| Linux (x86_64 musl)   | `x86_64-unknown-linux-musl`  | `org-tools_<ver>_linux_amd64.tar.gz`  |
| Linux (ARM64 musl)    | `aarch64-unknown-linux-musl` | `org-tools_<ver>_linux_arm64.tar.gz`  |
| Windows (x86_64 MSVC) | `x86_64-pc-windows-msvc`     | `org-tools_<ver>_windows_amd64.zip`   |
| Windows (ARM64 MSVC)  | `aarch64-pc-windows-msvc`    | `org-tools_<ver>_windows_arm64.zip`   |

```sh
shasum -a 256 --ignore-missing -c checksums.txt
INSTALL_BIN_DIR="${INSTALL_BIN_DIR:-$HOME/.local/bin}"
INSTALL_MAN_DIR="${INSTALL_MAN_DIR:-$HOME/.local/share/man}"
mkdir -p "$INSTALL_BIN_DIR" "$INSTALL_MAN_DIR/man1" "$INSTALL_MAN_DIR/man5"
tar -xzf org-tools_<ver>_<os>_<arch>.tar.gz
install -m 0755 org "$INSTALL_BIN_DIR/org"
install -m 0644 man/org.1 "$INSTALL_MAN_DIR/man1/org.1"
install -m 0644 man/org-tools.toml.5 "$INSTALL_MAN_DIR/man5/org-tools.toml.5"
```

5. Build and install from `crates.io` or a local repository checkout using Cargo or `mise`.

```sh
# Install published crate from crates.io
cargo install org-tools

# Or build and install from source
git clone https://github.com/lino/org-tools.git
cd org-tools
mise install
mise run install
```

6. Generate `bash`, `zsh`, and `fish` completions into XDG Base Directory Specification (`XDG`) user paths or source them in your shell profile.

```sh
# Option A: One-line interactive shell setup
eval "$(org --completions zsh)"
org --completions fish | source
eval "$(org --completions bash)"

# Option B: Persist completion files in XDG user directories
mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"
org --completions bash > "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions/org"

mkdir -p "$HOME/.zsh/completions"
org --completions zsh > "$HOME/.zsh/completions/_org"

mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
org --completions fish > "${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions/org.fish"
```

7. Verify the installed binary version, subcommand help, and Unix manual pages (`man 1 org` and `man 5 org-tools.toml`).

```sh
org --version
man 1 org
man 5 org-tools.toml
```

## Expected Outcome

Running the verification commands prints the installed `org` version and resolves both Unix manual pages. Every command exits cleanly with status code `0`.

```text
$ org --version
org 0.1.4
$ echo $?
0
$ man -w 1 org
/usr/share/man/man1/org.1
$ man -w 5 org-tools.toml
/usr/share/man/man5/org-tools.toml.5
$ echo $?
0
```

## Power-User & Scripting Options

Operators can override target installation directories using `INSTALL_BIN_DIR`, `INSTALL_MAN_DIR`, and `CARGO_INSTALL_ROOT`. You can also run the GitHub Container Registry (`GHCR`) image directly without installing host packages. Consult the [Docker & MCP Runtime Guide](docker-and-mcp-runtime.md) and the [Glossary and Abbreviations](../glossary.md) for container and terminology details.

```sh
# Run containerized org without installing a local binary
docker run --rm -v "$PWD:/workspace" ghcr.io/lino/org-tools:latest fmt check .

# Export machine-readable JSON Schemas after installation
org schema --list --format json
```

??? info "Deep Dive: Package Contents & Quarantine Removal"

    Every release archive, `.deb`, `.rpm`, and Homebrew Cask bundles `org`, `LICENSE`, and `README.org`. Each package also includes shell completions (`completions/`) and Unix `roff` manpages (`man/org.1`, `man/org-tools.toml.5`).

    On macOS, the Homebrew Cask post-install hook runs `xattr -dr com.apple.quarantine` automatically. If you download a `.tar.gz` archive in a web browser, clear the attribute manually before first execution:

    ```sh
    xattr -dr com.apple.quarantine "$INSTALL_BIN_DIR/org"
    ```

## Troubleshooting

| Symptom                                                 | Cause                                                     | Fix                                                                                |
| ------------------------------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `"org" cannot be opened because Apple cannot check`     | Browser download attached `com.apple.quarantine` on macOS | Run `xattr -dr com.apple.quarantine "$(command -v org)"` or reinstall via Homebrew |
| `org: command not found`                                | Installation directory is missing from shell `PATH`       | Add `$HOME/.local/bin` or `$HOME/.cargo/bin` to `PATH` in your shell profile       |
| `No manual entry for org` or `org-tools.toml`           | Local `man` directory is missing from `MANPATH`           | Export `MANPATH="$HOME/.local/share/man:${MANPATH:-}"` and run `man 1 org`         |
| `sha256sum: WARNING: 1 computed checksum did NOT match` | Corrupted archive download or mismatched release version  | Re-download both the archive and `checksums.txt` from the matching GitHub Release  |
