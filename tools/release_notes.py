"""The notes of a GitHub release, from CHANGELOG.md.

    python3 tools/release_notes.py 1.3.3 releases.txt > notes.md
    python3 tools/release_notes.py --self-test

The in-app updater shows the notes of the release it offers, and it offers only the newest
promoted release. Someone who skips versions (1.2.0 -> 1.3.3, with 1.3.0 to 1.3.2 never
promoted) would only see what is new in 1.3.3. So the notes of a release hold its own section
plus the sections of every version since the previous promoted release, newest first:
exactly what is new for someone coming from the version the updater offered before.

`releases.txt` lists the published releases, one per line: the tag and whether it is a
pre-release, as `gh release list --exclude-drafts --json tagName,isPrerelease --jq
'.[] | "\\(.tagName) \\(.isPrerelease)"'` prints them. A release that is not a pre-release is a
promoted one.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
HEADING = re.compile(r"^## \[(\d+\.\d+\.\d+)\]")


def key(version):
    return tuple(int(p) for p in version.split("."))


def sections(changelog):
    """Version -> the Markdown under its `## [x.y.z]` heading, in file order (newest first)."""
    out = {}
    current = None
    for line in changelog.splitlines():
        m = HEADING.match(line)
        if m:
            current = m.group(1)
            out[current] = []
            continue
        if line.startswith("## ") or re.match(r"^\[[0-9.]+\]: ", line):
            current = None
            continue
        if current is not None:
            out[current].append(line)
    return {v: "\n".join(lines).strip() for v, lines in out.items()}


def promoted(releases):
    """The promoted versions in a `releases.txt` listing."""
    out = set()
    for line in releases.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1] == "false" and re.fullmatch(r"v\d+\.\d+\.\d+", parts[0]):
            out.add(parts[0][1:])
    return out


def notes(changelog, version, promoted_versions):
    """The release notes of `version`: its section, or, when versions came between it and the
    previous promoted release, all of them with a heading each."""
    secs = sections(changelog)
    if version not in secs:
        raise SystemExit(f"CHANGELOG.md has no section for {version}")
    older = [v for v in promoted_versions if key(v) < key(version)]
    since = max(older, key=key) if older else None
    included = sorted(
        (v for v in secs if key(v) <= key(version) and (since is None or key(v) > key(since))),
        key=key,
        reverse=True,
    )
    if since is None or len(included) == 1:
        # The first release, or nothing in between: only its own section, as before.
        return secs[version] + "\n"
    parts = [f"Everything that is new since {since}."]
    for v in included:
        parts.append(f"## {v}\n\n{secs[v]}")
    return "\n\n".join(parts) + "\n"


def self_test():
    changelog = """# Changelog

Intro.

## [1.3.3] - 2026-11-21

### Improvements

- Less memory.

## [1.3.2] - 2026-11-21

### Improvements

- Smooth recordings.

## [1.3.1] - 2026-11-21

### Bug fixes

- Recording works.

## [1.2.0] - 2026-11-21

### New features

- History.

## [1.1.0] - 2026-11-19

- First.
"""
    releases = "v1.3.3 true\nv1.3.2 true\nv1.3.1 true\nv1.2.0 false\nv1.1.0 false\nv1.0.0 false\nnonsense\n"
    stable = promoted(releases)
    assert stable == {"1.2.0", "1.1.0", "1.0.0"}, stable
    # Skipped versions come along, newest first, each under its own heading.
    got = notes(changelog, "1.3.3", stable)
    assert got.startswith("Everything that is new since 1.2.0.\n\n## 1.3.3\n\n### Improvements\n\n- Less memory."), got
    assert got.index("## 1.3.3") < got.index("## 1.3.2") < got.index("## 1.3.1"), got
    assert "History" not in got and "First" not in got, got
    # Right after a promoted release: only its own section, unchanged.
    assert notes(changelog, "1.2.0", stable) == "### New features\n\n- History.\n"
    # The oldest release (nothing promoted before it): its own section.
    assert notes(changelog, "1.1.0", stable) == "- First.\n"
    # Once 1.3.3 is promoted, a newer release only lists what is new since then.
    stable2 = promoted("v1.3.4 true\nv1.3.3 false\nv1.2.0 false\n")
    changelog2 = changelog.replace("## [1.3.3]", "## [1.3.4] - 2026-11-22\n\n- Next.\n\n## [1.3.3]")
    assert notes(changelog2, "1.3.4", stable2) == "- Next.\n"
    # A promoted pre-release in between (1.3.2 promoted, 1.3.3 not yet): from there.
    got = notes(changelog, "1.3.3", promoted("v1.3.3 true\nv1.3.2 false\nv1.2.0 false\n"))
    assert got == "### Improvements\n\n- Less memory.\n", got
    # The real CHANGELOG.md parses, and every version in it has notes.
    real = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    secs = sections(real)
    assert secs and all(secs.values()), [v for v, s in secs.items() if not s]
    print("release_notes.py: ok")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    version = sys.argv[1].removeprefix("v")
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    releases = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")
    sys.stdout.write(notes(changelog, version, promoted(releases)))


if __name__ == "__main__":
    main()
