# Changelog

Notable changes to sqlautopsy. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, the minor
version moves for features and for behavior changes alike.

The section of a version is its GitHub release note, word for word
(`scripts/release-notes.sh`). It is written before the tag, and a tag without a
section fails the release workflow.

## [Unreleased]

### Added

- `sqlautopsy <folder-or-file>...` reads the `SQLDumpNNNN.txt` and `.log` files
  of a dump folder, without symbols or network access, and prints for each dump
  the build and product name, the time, the spid, the cause or the exception
  with its module and offset, the stack signature, the input buffer, the short
  stack, the number of loaded modules and the errorlog entries written just
  before the dump.
- `--format json` prints the same report with every frame and module, under
  `"schema": 1`.
- The `.mdmp` is listed with its dump but not read yet.
