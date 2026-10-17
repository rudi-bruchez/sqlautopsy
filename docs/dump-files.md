# What SQL Server writes when it dumps

The input sqlautopsy reads, as described by public sources. Each claim links to
its source; what no source confirms is listed at the end.

## When a dump is written

SQL Server calls SQLDumper.exe itself on exceptions, assertions, non-yielding
schedulers, latch timeouts and database corruption. By default it writes a
minidump with indirectly referenced memory
([Use Sqldumper.exe](https://learn.microsoft.com/en-us/troubleshoot/sql/tools/use-sqldumper-generate-dump-file)).
The errorlog frames each case with `BEGIN STACK DUMP` followed by the cause:
`Non-yielding Scheduler`, `Deadlocked Schedulers`, `Non-yielding IOCP Listener`,
`Non-yielding Resource Monitor`. `DBCC STACKDUMP` writes one on demand.

## The files

Dumps go to the `MSSQL\LOG` folder of the instance unless the Dump Directory was
changed in Configuration Manager. Each dump is a set of three files
([Pinal Dave, 2017](https://blog.sqlauthority.com/2017/11/08/sql-server-many-sqldump-files-consuming-lot-disk-space/)):

| File | Content | Needs symbols |
|---|---|---|
| `SQLDumpNNNN.txt` | machine header, exception, module table, registers, short stack, formatted session structures, input buffer | no |
| `SQLDumpNNNN.log` | snapshot of the errorlog at dump time, including the build | no |
| `SQLDumpNNNN.mdmp` | the minidump: threads, stacks, module list, referenced memory | yes, to name frames |

SQLDumper also writes `SQLDUMPER_ERRORLOG.log`. A manual dump is named
`SQLDmprNNNN.mdmp`. With trace flag 2616 (SQL Server 2022 CU12 and later) the
name carries the process id and a timestamp, and only one dump is kept per stack
signature per hour
([trace flags](https://learn.microsoft.com/sql/t-sql/database-console-commands/dbcc-traceon-trace-flags-transact-sql)).

`sys.dm_server_memory_dumps` lists the dump files with `filename`,
`creation_time` and `size_in_bytes`
([Learn](https://learn.microsoft.com/sql/relational-databases/system-dynamic-management-objects/sys-dm-server-memory-dumps-transact-sql)).

## The text dump

A complete published `SQLDump.txt` (SQL Server 2005, access violation,
[SQLServerCentral, 2010](https://www.sqlservercentral.com/forums/topic/sql-server-stack-dump))
shows the sections in this order:

1. Machine header: BIOS, time, processors, Windows version, memory load.
2. `BEGIN STACK DUMP`, spid, `Exception Address = ... Module(sqlservr+00FDEB09)`,
   `Exception Code`.
3. The `MODULE BASE END SIZE` table: every loaded module with base address and
   size. Third-party DLLs show up here (in that example, the extended procedure
   of a third-party backup tool).
4. Registers, with a few memory words.
5. `Short Stack Dump`: a list of `Module(module+offset)` addresses.
6. Structures already formatted by SQL Server: `PSS`, `CSession`, `CBatch`, and
   the `Input Buffer` of a user session.

For an access violation, the text carries lines such as
`Exception Address = 00007FFA414ED763 Module(sqlmin+000000000000D763)`,
`Exception Code = c0000005 EXCEPTION_ACCESS_VIOLATION`, then the registers
([KB 4033835](https://learn.microsoft.com/en-us/troubleshoot/sql/database-engine/backup-restore/access-violations-memory-dump-files)).

Since SQL Server 2019 CU2, `DBCC STACKDUMP ... TEXT_DUMP = LIMITED` is the
default and shortens the text file.

## The errorlog lines

Since SQL Server 2019 the errorlog output is short and ends with
`Stack Signature for the dump is 0x...` then
`External dump process return code 0x20000001.`. The engine uses the signature
to avoid dumping the same exception twice.

## Dump types

| SQLDumper flags | Type | Typical size |
|---|---|---|
| `0x0120` | minidump | a few MB |
| `0x0128` | minidump with referenced memory (default of automatic dumps) | rarely above 20 to 30 MB |
| `0x8100` | filtered | 30 to 60 % of process memory |
| `0x01100` | full | the process memory |

A minidump does not include code sections nor the `.pdata`/`.xdata` unwind
tables of the modules
([KB 919711](https://learn.microsoft.com/en-us/troubleshoot/sql/analysis-services/configure-ssas-generate-memory-dump-files)).
Unwinding x64 stacks therefore needs the module images or their unwind data from
another source: the binaries from the Microsoft symbol server, or the PDB.

## Observed in real dumps

Measured on thirteen dumps from SQL Server 2005, 2008 R2, 2012 and 2019
instances (exceptions, non-yielding schedulers, a stalled dispatcher, a latch
timeout, DBCC corruption). These are what the parser relies on; the dumps
themselves stay out of this repository.

- The `.txt` is UTF-16 LE with a BOM and CRLF line ends. Lines are padded with
  spaces to 113 characters, and a longer line is cut into 113-character pieces
  with no marker: a piece that was cut ends on a non-space character exactly at
  column 113.
- The input buffer is written as lines of `*  ` followed by 72 characters, cut
  at fixed columns, in the middle of a word.
- Dates in the header are month first whatever the server's locale:
  `06/27/13 18:53:25 spid 18388`.
- The module table and the short stack are present for exceptions only.
  Non-yielding, latch timeout and DBCC dumps carry the cause and, when there is
  a session, the input buffer, but neither modules nor frames.
- The `.log` is the last 64 KB of the errorlog, UTF-16 LE without a BOM; its
  first entry is cut. Exception dumps write
  `***Stack Dump being sent to ...\SQLDumpNNNN.txt` there; non-yielding dumps
  write `***Unable to get thread context for spid 0` instead.
- The errorlog carries `Stack Signature for the dump is 0x...` on 2005,
  2008 R2, 2012 and 2019 alike. Three divide-by-zero exceptions of one
  instance, written over two months, share one signature.

## Not confirmed

- Which sections remain in a `.txt` written with `TEXT_DUMP = LIMITED`.
- How the stack signature is computed.
- Whether `sqlmin.dll`, `sqllang.dll` and `sqldk.dll` are served by
  `msdl.microsoft.com` by timestamp and image size (verified for `sqlos`,
  `hkengine` and `hkruntime` of a Linux 2022 build only).
