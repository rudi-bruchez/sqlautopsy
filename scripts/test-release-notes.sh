#!/usr/bin/env bash
# Self-check for release-notes.sh. Run: bash test-release-notes.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fail=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1"; echo "  want: $3"; echo "  got:  $2"; fail=1; fi; }

cat > "$t/kac.md" <<'MD'
# Changelog

Preamble.

## [Unreleased]

- not yet.

## [1.0.0-rc.1] - 2026-10-08

- candidate.

## [0.2.0] - 2026-10-06

Why this release.

### Added

- a thing.

## [0.1.0] - 2026-09-01

- first.

[0.2.0]: https://example.invalid/compare/v0.1.0...v0.2.0
[0.1.0]: https://example.invalid/releases/tag/v0.1.0
MD
cat > "$t/plain.md" <<'MD'
## 0.7.0, 13 September 2026

- seven.

## 0.6.0, 12 September 2026

## 0.5.0, 31 August 2026

- five.
MD

# Empty: no tag list, so the link falls back to the next heading below, and
# no test depends on the network or on the repository it is run from.
export RELEASE_NOTES_TAGS=
n() { "$here/release-notes.sh" "$@" 2>&1; }
out=$(n v0.2.0 o/r "" "$t/kac.md")
check "body starts at the first non-blank line" "$(printf '%s\n' "$out" | head -1)" "Why this release."
check "sub-headings kept" "$(printf '%s\n' "$out" | grep -c '^### Added$')" "1"
check "compare link to the next heading below" "$(printf '%s\n' "$out" | tail -1)" "Full changelog: https://github.com/o/r/compare/v0.1.0...v0.2.0"
check "no download block without checksums" "$(printf '%s\n' "$out" | grep -c 'Download')" "0"
out=$(n 0.1.0 o/r "" "$t/kac.md")
check "oldest: link references dropped, no compare link" "$out" "- first."
out=$(n 1.0.0-rc.1 o/r "" "$t/kac.md")
check "pre-release version matched exactly" "$(printf '%s\n' "$out" | tail -1)" "Full changelog: https://github.com/o/r/compare/v0.2.0...v1.0.0-rc.1"
check "Unreleased never taken for previous or body" "$(printf '%s\n' "$out" | grep -c 'not yet')" "0"
out=$(n 0.7.0 o/r r_0.7.0_checksums.txt "$t/plain.md")
check "plain heading form" "$(printf '%s\n' "$out" | head -1)" "- seven."
check "download block names the checksums file" "$(printf '%s\n' "$out" | grep -c 'sha256sum -c --ignore-missing r_0.7.0_checksums.txt')" "1"
out=$(n 0.6.0 o/r "" "$t/plain.md"); rc=$?
check "empty section refused" "$rc:$out" "1:the section for 0.6.0 in $t/plain.md is empty"
out=$(n 0.4.0 o/r "" "$t/plain.md"); rc=$?
check "missing section refused" "$rc:$out" "1:$t/plain.md has no section for 0.4.0"
cat > "$t/gap.md" <<'MD'
## [0.44.0] - 2026-09-30

- forty-four.

## [0.43.0] - 2026-09-29

- untagged.

## [0.42.0] - 2026-09-28

- untagged too.

## [0.38.0] - 2026-09-17

- tagged.
MD
out=$(RELEASE_NOTES_TAGS=$'v0.38.0\nv0.44.0' n 0.44.0 o/r "" "$t/gap.md")
check "untagged older versions skipped for the compare link" "$(printf '%s\n' "$out" | tail -1)" "Full changelog: https://github.com/o/r/compare/v0.38.0...v0.44.0"
out=$(RELEASE_NOTES_TAGS=$'v0.44.0' n 0.44.0 o/r "" "$t/gap.md")
check "no tagged older version, no compare link" "$out" "- forty-four."
exit $fail
