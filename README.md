# sqlautopsy

A reader for SQL Server memory dumps that runs on Linux, macOS and Windows.

When SQL Server hits a non-yielding scheduler (17883), a latch timeout (844/845),
an access violation, an assertion or a `DBCC STACKDUMP`, SQLDumper writes a set of
files into the `LOG` folder: `SQLDumpNNNN.txt`, `SQLDumpNNNN.log` and
`SQLDumpNNNN.mdmp`. Today, reading them means a Windows machine with WinDbg, and
Microsoft's SQLCallStackResolver repository was archived in October 2026.
sqlautopsy aims to turn a dump folder into a short diagnosis without either.

## Status

Early work. Nothing is usable yet.

## What it will report

For each dump in a folder:

- the SQL Server build and the exception or condition that triggered the dump;
- the stack of the faulting thread, resolved with Microsoft's public symbols;
- the loaded modules that do not come from Microsoft (antivirus, backup agents,
  OLE DB providers, extended stored procedures), a frequent cause of crashes;
- the input buffer of the session, when the dump carries it.

## How it works

Two layers, used in that order:

1. The text files. `SQLDumpNNNN.txt` and `.log` already carry the exception, the
   module list, a short stack and the input buffer. They need no symbols and no
   network access.
2. The minidump. The `.mdmp` is parsed with Mozilla's
   [rust-minidump](https://github.com/rust-minidump/rust-minidump) crates, x64
   stacks are unwound, and addresses are resolved against the public PDBs of the
   SQL Server build, downloaded once from
   `https://msdl.microsoft.com/download/symbols` and kept in a SymStore-layout
   cache under `~/.cache/sqlautopsy`.

Public symbols carry function names only: no types, no local variables, no line
numbers. SQLOS structures (schedulers, workers, tasks) cannot be decoded from a
dump with them, and sqlautopsy does not try.

## Using it from an AI agent

The CLI is meant to be driven by a coding agent as well as by a person: no
prompts, data on stdout and diagnostics on stderr, `--format json` with a
versioned schema, documented exit codes. The contract is in
[AGENTS.md](AGENTS.md).

## Documentation

- [What SQL Server writes when it dumps](docs/dump-files.md)
- [Development and releases](docs/development.md)
- [Changelog](CHANGELOG.md)

## License

MIT
