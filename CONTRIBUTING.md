# Contributing to Blink

Thanks for wanting to help. Bug reports, ideas and pull requests are all welcome.

## Reporting a bug

Open an [issue](https://github.com/blink-screenshot/blink/issues/new/choose) and include:

- your Blink version (Settings > About) and Windows version
- your monitor setup if it is about capturing (number of monitors, scaling per monitor)
- what you did, what you expected, and what happened instead; a screenshot helps

Security problems: please do not open a public issue, see [SECURITY.md](SECURITY.md).

## Suggesting a feature

Open an issue describing the problem you want to solve before writing code. Blink tries to stay small and focused on capture, annotate, redact and present; features that pull in a lot of new UI or dependencies need a good reason.

## Building

You need a stable Rust toolchain ([rustup](https://rustup.rs)).

```
cargo test
cargo run --release              # on Windows: the full app in the tray
cargo run --release -- --open some-image.png   # the editor only, also on Linux
```

On Linux, the editor and every test work; capturing, the tray and hotkeys are Windows-only. [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) explains the architecture, the installer and the release process.

## Pull requests

- Keep a pull request to one change. Smaller is easier to review.
- Run `cargo fmt` and `cargo test` before pushing. CI runs the tests on Windows and Linux.
- Every user-visible text goes through `t("...")`; add the Dutch translation to `src/i18n_nl.rs` (a test fails when one is missing). If you do not speak Dutch, say so in the pull request and a maintainer will add it.
- If you change dependencies, run `python tools/third_party_notices.py`.
- Describe user-visible changes in your pull request; the maintainer adds them to `CHANGELOG.md` at release time.
- Colours come from `src/theme.rs`; do not hard-code new ones.

By submitting a pull request you agree that your contribution is licensed under the [GPL-3.0-or-later](LICENSE), like the rest of Blink.

## Code of conduct

Be kind and assume good intent. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
