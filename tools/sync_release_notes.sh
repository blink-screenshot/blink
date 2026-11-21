#!/usr/bin/env bash
# Sets the notes of every GitHub release from CHANGELOG.md (see tools/release_notes.py: a
# release lists everything that is new since the previous promoted release). Used by the
# Build workflow after a push to main and by the Promote workflow, which changes what
# "previous promoted release" means. Needs gh with GH_TOKEN and GH_REPO set.
set -euo pipefail
cd "$(dirname "$0")/.."

# Editing a release can make GitHub pick another "latest": put the current one back after.
latest=$(gh release view --json tagName --jq .tagName 2>/dev/null || true)
gh release list --limit 200 --exclude-drafts --json tagName,isPrerelease \
  --jq '.[] | "\(.tagName) \(.isPrerelease)"' > releases.txt
for version in $(grep -oP '^## \[\K[0-9]+\.[0-9]+\.[0-9]+(?=\])' CHANGELOG.md); do
  tag="v$version"
  if ! grep -q "^$tag " releases.txt; then
    echo "No release $tag (yet), skipped"
    continue
  fi
  python3 tools/release_notes.py "$version" releases.txt > notes.md
  gh release edit "$tag" --notes-file notes.md --title "Blink $version" > /dev/null
  echo "Updated $tag"
done
rm -f releases.txt notes.md
if [ -n "$latest" ]; then
  gh release edit "$latest" --latest > /dev/null
  echo "Latest (unchanged): $latest"
fi
