# Local Development

This guide covers the Windows development environment, running and testing Resonance, and building the Windows installer.

## Overview

Resonance is a Windows-only Rust desktop application. Running it locally requires access to a configured Resonance API endpoint.

## Prerequisites

- Rust stable toolchain
- Visual Studio Build Tools with the **Desktop development with C++** workload
- A reachable Resonance API endpoint

Ensure Rust stable is available in `PATH`.

## Run the application

Set the exact URL of the PHP endpoint and run the desktop app from the repository root.

**PowerShell**

```powershell
$env:RESONANCE_API_URL = "https://example.com/resonance_api.php"
cargo run --bin resonance
```

At startup, the app checks the API and database connection. HTTP is permitted only for local loopback addresses such as `http://127.0.0.1/`; non-local HTTP URLs are rejected. Use HTTPS for hosted endpoints.

## Test and build

Run the test suite:

```sh
cargo test
```

Build a release binary:

```sh
cargo build --release
```

## Build local installers

The installer is built on Windows. Local packages are written to `dist/`.

### Windows

Install Inno Setup 6, set the API URL, and run:

```powershell
$env:RESONANCE_DEFAULT_API_URL = "https://example.com/resonance_api.php"
.\installer\build-local.ps1
```

The script runs `cargo test` first. Pass `-SkipTests` to skip that step.

## Release workflows

The manually triggered GitHub Actions workflow builds the Windows installer.

Set the repository variable `RESONANCE_DEFAULT_API_URL` to the hosted HTTPS API endpoint and provide an existing release tag when starting a workflow. Builds fail if the URL is missing or is not HTTPS. Each workflow attaches its installer to the selected GitHub release.

## Data and local development notes

Local chat data is stored under `%LOCALAPPDATA%\Resonance\chats`. Chat history is encrypted before it is stored; existing plaintext files are migrated automatically at login.

## Related documentation

- [Resonance overview](../README.md)
- [Getting Started](FRIENDS.md)
- [Server Deployment](DEPLOYMENT.md)

For code navigation, the main areas are `src/views/` for UI, `src/services/` for API and storage services, `src/models/` for data models, and `installer/` for Windows packaging.
