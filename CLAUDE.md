# Working agreements for Claude

Blink is a public, open-source (GPL-3.0-or-later) screenshot tool for Windows, written in Rust.
Repository: `blink-screenshot/blink`. Website: `site/`, published to GitHub Pages. Read
`docs/DEVELOPMENT.md` for the architecture and the release process before larger changes.

## Commits and pull requests

- No attribution in commits, pull requests or GitHub comments: no "Generated with Claude Code" line, no Co-Authored-By trailer and no link to the Claude session.
- Commit as `Blink <blink-screenshot@users.noreply.github.com>`: pass `-c user.name="Blink" -c user.email="blink-screenshot@users.noreply.github.com"` to `git commit`. Never commit with a personal name or email address.
- Never push to `main`. Work on a branch and open a pull request; the maintainer merges.
- Commit messages and pull requests in English. Talk to the maintainer in Dutch.
- The repository is public: never commit personal data, internal planning, pricing or strategy documents. Those belong outside the repository.

## Releasing

- A version is released by setting `version` in `Cargo.toml` and adding a `## [x.y.z] - date` section at the top of `CHANGELOG.md` (*New features*, *Improvements*, *Bug fixes*). Merging that to `main` publishes a **pre-release**; nobody gets it until the maintainer tests it and runs **Actions > Promote**. Never promote yourself, and never mark a release as latest in a workflow.
- Bump the version in every pull request that changes the app: new features `x.y+1.0`, only fixes or small improvements `x.y.z+1`. Changes to only the site, docs or CI need no version.
- `CHANGELOG.md` is what users read in the update window and in Settings > Updates > What's new: write for users, not developers, and say what they will notice.

## Code

- Run `cargo fmt` and `cargo test` before committing. Tests also run on Linux; capture, tray and hotkeys are Windows-only (`src/platform/win/`).
- Every user-visible text goes through `t("...")`, with a translation in every table: `src/i18n_nl.rs`, `i18n_de.rs`, `i18n_fr.rs`, `i18n_es.rs` and `i18n_pt.rs` (Brazilian Portuguese); a test fails if one is missing.
- Colours come from `src/theme.rs` only.
- After changing dependencies, run `python tools/third_party_notices.py`; CI fails if `THIRD-PARTY-NOTICES.md` is out of date.
- Keep claims on the website and in the README true: check them against the code (counts of backgrounds, devices, emoji, capture modes) when features change.
