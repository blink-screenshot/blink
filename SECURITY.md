# Security policy

## Supported versions

Only the [latest release](https://github.com/blink-screenshot/blink/releases/latest) gets fixes. Blink updates itself, so please check that you run the newest version before reporting.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through GitHub: [Security > Report a vulnerability](https://github.com/blink-screenshot/blink/security/advisories/new).

Include what an attacker could do, the steps to reproduce it, and the Blink and Windows versions. You get a first reply within a week. Once a fix is released, the advisory is published and you are credited, unless you prefer not to be.

Areas where a problem would matter most:

- the updater (`src/update.rs`): it downloads and runs an installer
- redaction: blur or pixelate that can be reversed would leak what users meant to hide
- the keyboard hook (`src/platform/win/kbhook.rs`)
