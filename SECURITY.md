# Security policy

## Supported versions

Only the latest release of Flare gets security fixes. Update to the newest
[release](https://github.com/Maxaubert/Flare/releases/latest) before you report a problem.

## Report a vulnerability

Please report a hole privately. Do not open a public issue.

1. Open the [Security tab](https://github.com/Maxaubert/Flare/security) of this repository.
2. Click **Report a vulnerability**.
3. Fill in the form. GitHub keeps the report private between you and the maintainer.

Say what you found, which version and system you used, and the steps to reproduce it. A small proof of
concept helps. We reply in the advisory, fix the problem in a new release, and credit you if you want.

## Scope

These parts matter most:

- **The updater.** How Flare finds, downloads, checks and applies a new release.
- **Plugin zip install.** Installing a plugin from a zip file: paths, file names and what the zip may
  write or run.
- **Pairing and remote sessions.** Adding a machine, the ssh connection and the session that runs on the
  other machine.
- **The control socket.** The local control channel (`flare-ctl`: a named pipe on Windows, a Unix socket on
  macOS) that agents and tools use to talk to a running Flare.

A problem in another part of Flare is welcome too. Problems in a third-party program Flare ships
(for example CEF, whisper.cpp or ConPTY) belong with that project first; tell us as well if Flare uses
it in an unsafe way.
