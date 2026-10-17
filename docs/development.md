# Development

## Build and check

    cargo fmt
    cargo clippy --all-targets -- -D warnings
    cargo test

CI runs the same three commands on every push to `main` and every pull request
(`.github/workflows/ci.yml`). The lint policy is in `Cargo.toml`: no unsafe code,
pedantic clippy as warnings, and warnings fail the build.

## Test fixtures

Fixtures are synthetic or produced on a lab instance with `DBCC STACKDUMP`, and
small. A dump from a real server contains that server's memory, query text and
data: it never enters this repository, not even trimmed. `.gitignore` refuses
`SQLDump*` and `*.mdmp` outside `tests/fixtures/`.

## Releasing

1. Move the `[Unreleased]` entries of `CHANGELOG.md` under a new
   `## [x.y.z] - YYYY-MM-DD` heading, with an opening paragraph that says what
   the version is for. That section is the GitHub release note, word for word.
2. Set the same `version` in `Cargo.toml`, run `cargo build` to update
   `Cargo.lock`, commit.
3. Preview the release page:

       scripts/release-notes.sh x.y.z rudi-bruchez/sqlautopsy sqlautopsy_x.y.z_checksums.txt

4. Tag and push: `git tag vx.y.z && git push origin vx.y.z`.

The release workflow (`.github/workflows/release.yml`) checks that the tag
matches `Cargo.toml`, builds and smoke-tests the binary on Linux x64 and arm64
(static, musl), macOS arm64 and Windows x64, then publishes the archives, a
SHA-256 checksums file and the changelog section. A tag without a changelog
section fails before anything is built.
