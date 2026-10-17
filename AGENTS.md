# Working in this repository

sqlautopsy reads SQL Server memory dumps (`SQLDumpNNNN.txt`, `.log`, `.mdmp`)
on any platform. This file is for coding agents and humans alike.

## Before calling a change done

    cargo fmt
    cargo clippy --all-targets -- -D warnings
    cargo test

All three must pass. Do not add `#[allow]` to silence clippy without a comment
that says why; crate-wide exceptions go in `[lints]` in `Cargo.toml`.

## The input is hostile

A dump comes from someone else's server. Parse it as untrusted bytes:
`unsafe_code` is forbidden, every offset and length read from the file is
checked before use (`get`, `checked_add`, `try_from`), allocations are bounded
by the file size, and a malformed file is an error value with the offset, never
a panic.

## Never commit a real dump

A dump holds the memory of a production server: query text, data, sometimes
credentials. Fixtures are synthetic or come from a lab instance
(`DBCC STACKDUMP`), and stay small. This is a public repository.

## The CLI contract

So that an agent can drive the tool without reading its source:

- non-interactive: no prompt, no pager, no color unless stdout is a terminal;
- data on stdout, diagnostics on stderr;
- `--format json` with a `schema` version field; fields are added, never
  renamed or removed within a schema version;
- exit codes: 0 success, 1 the input could not be analyzed, 2 usage error;
- network access only to download public symbols from
  `msdl.microsoft.com`, never with dump content, and an offline mode that works
  from the local cache.

## Documentation

- `README.md`: what the tool does and how to use it.
- `docs/dump-files.md`: what SQL Server writes, with sources.
- `docs/development.md`: build, fixtures, release.
- `CHANGELOG.md`: written before each tag; its section is the release note.

English with American spelling. No bold, no em dash. A claim about SQL Server
links to its source.
