# Extended Events call stacks

`sqlautopsy stack` names the frames that the `package0.callstack_rva` action
captures. The action appeared with SQL Server 2022
([Bob Dorr, 2022](https://techcommunity.microsoft.com/t5/sql-server-blog/capture-a-call-stack-without-generating-a-dump/ba-p/3245860)).
Each frame carries its module's PDB name, GUID and age, so the stack can be
named on any machine, from the public symbol server. Trace flag 3656 is not
needed, and neither is a copy of the instance's modules. Microsoft's
SQLCallStackResolver did this on Windows until its repository was archived in
October 2026.

## Capturing

A histogram target groups identical stacks and counts them. This is the right
target for "which code path leads to this wait". A ring buffer or an
`event_file` keeps one stack per event; sqlautopsy merges identical ones and
counts them the same way.

```sql
CREATE EVENT SESSION callstacks ON SERVER
ADD EVENT sqlos.wait_info
(
    ACTION (package0.callstack_rva)
    WHERE wait_type = N'PAGEIOLATCH_SH' AND duration > 0
)
ADD TARGET package0.histogram
    (SET source = N'package0.callstack_rva', source_type = 1);
ALTER EVENT SESSION callstacks ON SERVER STATE = START;
```

`callstack_rva` costs more than `package0.callstack`. Bob Dorr keeps it for
targeted debugging and rare events. Filter the event tightly and stop the
session when done.

## Reading

```sh
sqlcmd -y 0 -Q "SELECT CAST(t.target_data AS nvarchar(max))
  FROM sys.dm_xe_session_targets t
  JOIN sys.dm_xe_sessions s ON s.address = t.event_session_address
  WHERE s.name = 'callstacks' AND t.target_name = 'histogram'" > stacks.xml
sqlautopsy stack stacks.xml
```

The input can be the histogram or ring buffer XML, an `event_file` read back
with `sys.fn_xe_file_target_read_file`, or the `<frame ... />` elements alone,
from a file or standard input. The frames may be wrapped in CDATA or escaped as
`&lt;frame`. A stack starts at frame `00`.

## Observed

On SQL Server 2025 CU7 (17.0.4065.4, Linux container), 17 October 2026:

- A frame reads `<frame id="00" address="0xB87426742" pdb="sqllang.pdb" age="2"
  guid="A1EE9134-BEFA-47A3-B2A6-1129DC226721" module="sqllang" rva="0x1876742" />`.
  The module has no extension.
- The symbol server key is the GUID without dashes followed by the age in hex.
  The age is written in decimal: SQLCallStackResolver parses it with
  `int.Parse` and hands it to dbghelp's `SymFindFileInPath`
  ([ModuleInfoHelper.cs](https://github.com/microsoft/SQLCallStackResolver/blob/main/Engine/ModuleInfoHelper.cs)).
  With an age of 2, both readings give the same key, and `msdl.microsoft.com`
  answered 200 on it for `sqllang.pdb` and `SqlDK.pdb`.
- On Linux, the `kernel32` and `ntdll` frames belong to SQLPAL, and their PDBs
  are not on the symbol server (404). Those frames stay unnamed.
- The top frame named itself `IServerConfiguration::SubscribeAndGetDynamicStringSetting+0x12A`
  above `GenericEvent::PublishAndCallAction`. That is most likely the
  action's own capture code, which public symbols do not name: a public PDB
  names a function missing from it after the nearest public symbol before it.
