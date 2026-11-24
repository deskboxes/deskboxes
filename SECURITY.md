# Security policy

## Supported versions

Only the [latest release](https://github.com/deskboxes/deskboxes/releases/latest) gets security fixes. Installed copies update themselves, so most people are on it within a day of a release.

## Reporting a vulnerability

**Please do not report security vulnerabilities through public issues, discussions or pull requests.**

Report them privately through GitHub: go to the repository's **Security** tab and choose **[Report a vulnerability](https://github.com/deskboxes/deskboxes/security/advisories/new)**.

Please include as much of this as you can:

- The kind of issue (for example code execution, privilege escalation or tampering with an update).
- The Deskboxes and Windows versions.
- Step-by-step instructions to reproduce it.
- Proof-of-concept or exploit code, if you have it.
- What an attacker could do with it.

You will get an answer within a few days. Once the issue is confirmed, a fix is released as soon as possible and you are credited in the release notes, unless you prefer not to be.

## Scope

These matter most:

- **Updates.** Installed copies download installers from this repository's releases and check their SHA-256 hash before installing them.
- **The installer** (`DeskboxesSetup.exe`), which installs per user without administrator rights.
- **File operations** that Deskboxes performs on your desktop (move, copy, delete, rename, undo).
