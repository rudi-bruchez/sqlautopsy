//! Reads `SQLDumpNNNN.txt`, the text SQL Server writes next to each dump.
//!
//! Layout, as observed on 2008 R2, 2012 and 2019 dumps: a "`BugCheck` Dump"
//! header (build, machine, memory), sometimes DBCC results, then a block
//! framed by star lines that starts at `* BEGIN STACK DUMP:` and carries the
//! condition or the exception, the input buffer, the module table and the
//! registers, then `* Short Stack Dump` and the frames. Exceptions carry
//! modules and frames; non-yielding and DBCC dumps usually carry neither.
//!
//! 2025 (17.0.1000.7, a `DBCC STACKDUMP`) writes no `BEGIN STACK DUMP:`
//! block: after the header comes every thread, each a `**Dump thread - spid
//! = N` line, the condition between star lines, then its frames in the
//! `<frame ... pdb guid age rva />` form of Extended Events, all with
//! `id="00"`. Those name themselves from the public PDBs, without the `.mdmp`.

use serde::Serialize;

use crate::xestack;

#[derive(Debug, Default, Serialize)]
pub struct StackDump {
    /// Build, as written: `15.0.2080.9`.
    pub version: Option<String>,
    /// `SQL Server 2019`, from the major and minor version.
    pub product: Option<String>,
    /// When the dump was taken, `2022-02-18T13:59:15`, server local time.
    pub time: Option<String>,
    pub spid: Option<u32>,
    /// What triggered the dump when it is not an exception:
    /// `Non-yielding Scheduler`, `Latch timeout`, `DBCC database corruption`.
    pub condition: Option<String>,
    pub exception: Option<Exception>,
    /// Text of the batch the session was running. May hold sensitive data.
    pub input_buffer: Option<String>,
    pub os: Option<String>,
    pub cpu: Option<String>,
    pub memory_load_percent: Option<u32>,
    pub physical_memory_mb: Option<u64>,
    pub available_physical_memory_mb: Option<u64>,
    pub stack: Vec<Frame>,
    pub modules: Vec<Module>,
    /// The stack of every thread, as 2025 writes them.
    pub threads: Vec<Thread>,
}

#[derive(Debug, Serialize)]
pub struct Thread {
    /// The session the thread was running, 0 for a system thread.
    pub spid: Option<u32>,
    pub frames: Vec<xestack::Frame>,
}

#[derive(Debug, Serialize)]
pub struct Exception {
    /// `c0000005`.
    pub code: String,
    /// `EXCEPTION_ACCESS_VIOLATION`.
    pub name: Option<String>,
    pub address: Option<String>,
    pub module: Option<String>,
    pub offset: Option<String>,
    /// From the public symbols: `CXStmtQuery::ErsqExecuteQuery+0x1A4`.
    pub function: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Frame {
    pub address: String,
    pub module: Option<String>,
    pub offset: Option<String>,
    /// From the public symbols, as for the exception.
    pub function: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Module {
    pub name: String,
    pub base: String,
    pub end: String,
    pub size: String,
}

pub fn parse(lines: &[String]) -> StackDump {
    let mut dump = StackDump::default();
    parse_header(lines, &mut dump);
    if let Some(begin) = lines
        .iter()
        .position(|l| star_content(l) == Some("BEGIN STACK DUMP:"))
    {
        parse_block(&lines[begin + 1..], &mut dump);
    } else {
        // Older versions also write one `**Dump thread` line, before the block.
        parse_threads(lines, &mut dump);
    }
    if let Some(short) = lines
        .iter()
        .position(|l| star_content(l) == Some("Short Stack Dump"))
    {
        let frames = lines[short + 1..]
            .iter()
            .map(|l| l.trim())
            .skip_while(|l| l.is_empty() || l.starts_with("Stack range "));
        dump.stack = frames.clone().map_while(parse_frame).collect();
        // 2025 writes the short stack in the `<frame ... />` form: the
        // dumping thread's stack, with each frame's PDB.
        let named: Vec<_> = frames.map_while(xestack::parse_frame).collect();
        if !named.is_empty() {
            dump.threads.push(Thread {
                spid: dump.spid,
                frames: named,
            });
        }
    }
    dump
}

/// The `**Dump thread` sections of a 2025 dump. The first condition line
/// gives the cause: `User initiated stack dump.  This is not a server
/// exception dump.` becomes `User initiated stack dump`.
fn parse_threads(lines: &[String], dump: &mut StackDump) {
    for line in lines {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("**Dump thread - spid = ") {
            dump.threads.push(Thread {
                spid: rest.split([',', ' ']).next().and_then(|s| s.parse().ok()),
                frames: Vec::new(),
            });
        } else if line.starts_with("<frame ") {
            if let (Some(thread), Some(frame)) =
                (dump.threads.last_mut(), xestack::parse_frame(line))
            {
                thread.frames.push(frame);
            }
        } else if let Some(content) = star_content(line)
            && !content.is_empty()
            && !content.starts_with('*')
            && !dump.threads.is_empty()
            && dump.condition.is_none()
            && dump.exception.is_none()
        {
            let first = content.split(".  ").next().unwrap_or(content);
            dump.condition = Some(first.trim_end_matches('.').to_string());
        }
    }
}

fn parse_header(lines: &[String], dump: &mut StackDump) {
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.starts_with("This file is generated by Microsoft SQL Server") {
            dump.version = lines
                .get(i + 1)
                .and_then(|next| next.trim().strip_prefix("version "))
                .map(|v| v.trim().to_string());
            dump.product = dump.version.as_deref().and_then(product);
        } else if let Some(cpu) = line.strip_prefix("Computer type is ") {
            dump.cpu = Some(cpu.trim_end_matches('.').trim().to_string());
        } else if line.starts_with("Windows NT ") {
            dump.os = Some(line.trim_end_matches('.').trim().to_string());
        } else if let Some(v) = line.strip_prefix("MemoryLoad = ") {
            dump.memory_load_percent = v.trim_end_matches('%').parse().ok();
        } else if let Some(v) = line.strip_prefix("Total Physical = ") {
            dump.physical_memory_mb = megabytes(v);
        } else if let Some(v) = line.strip_prefix("Available Physical = ") {
            dump.available_physical_memory_mb = megabytes(v);
        } else if let Some(rest) = line.strip_prefix("SqlDumpExceptionHandler: ") {
            // "Process 98 generated fatal exception c0000094 EXCEPTION_INT_DIVIDE_BY_ZERO. ..."
            if let Some(after) = rest.split("fatal exception ").nth(1) {
                let mut words = after.split_whitespace();
                if let Some(code) = words.next() {
                    dump.exception.get_or_insert_with(|| Exception {
                        code: code.to_string(),
                        name: words.next().map(|n| n.trim_end_matches('.').to_string()),
                        address: None,
                        module: None,
                        offset: None,
                        function: None,
                    });
                }
            }
        } else if let Some(now) = line.strip_prefix("Current time is ") {
            // "12:20:00 10/17/26.": when the file was written, the dump time
            // when no block gives one.
            let mut words = now.trim_end_matches('.').split_whitespace();
            if let (Some(time), Some(date)) = (words.next(), words.next()) {
                dump.time = iso_time(&format!("{date} {time}"));
            }
        } else if line.starts_with("* BEGIN STACK DUMP:") {
            break;
        }
    }
}

/// The framed block after `BEGIN STACK DUMP:`, up to its closing star line.
fn parse_block(lines: &[String], dump: &mut StackDump) {
    let mut seen_time = false;
    let mut i = 0;
    while let Some(line) = lines.get(i) {
        i += 1;
        let Some(content) = star_content(line) else {
            continue;
        };
        if content.starts_with("****") {
            break;
        }
        if content.is_empty() {
            continue;
        }
        if !seen_time {
            // "02/18/22 13:59:15 spid 126"
            seen_time = true;
            dump.time = iso_time(content).or(dump.time.take());
            dump.spid = content
                .rsplit("spid ")
                .next()
                .and_then(|s| s.trim().parse().ok());
        } else if let Some(v) = content.strip_prefix("Exception Address = ") {
            let mut words = v.split_whitespace();
            let address = words.next().map(str::to_string);
            let (module, offset) = words.next().map(module_offset).unwrap_or_default();
            let exception = dump.exception.get_or_insert_with(|| Exception {
                code: String::new(),
                name: None,
                address: None,
                module: None,
                offset: None,
                function: None,
            });
            exception.address = address;
            exception.module = module;
            exception.offset = offset;
        } else if let Some(v) = content.strip_prefix("Exception Code") {
            let mut words = v.trim_start_matches([' ', '=']).split_whitespace();
            if let Some(exception) = dump.exception.as_mut() {
                exception.code = words.next().unwrap_or_default().to_string();
                exception.name = words.next().map(str::to_string);
            }
        } else if content.starts_with("Input Buffer ") {
            i += read_input_buffer(&lines[i..], dump);
        } else if content.starts_with("MODULE ") {
            dump.modules = lines[i..].iter().map_while(|l| parse_module(l)).collect();
            i += dump.modules.len();
        } else if content.starts_with("Server was configured to produce dump") {
            // A dump trigger: `... - ex_dump_if_requested: Exception raised,
            // major=12, minor=5, ...` is error 1205.
            dump.condition = Some(content.to_string());
        } else if dump.condition.is_none() && dump.exception.is_none() && !content.contains(": ") {
            dump.condition = Some(content.to_string());
        }
    }
}

/// The input buffer follows its header as lines of `*  ` and 72 characters,
/// cut at fixed columns, so the pieces are joined without separator.
fn read_input_buffer(lines: &[String], dump: &mut StackDump) -> usize {
    let mut text = String::new();
    let mut used = 0;
    for line in lines {
        let Some(piece) = line.strip_prefix("*  ") else {
            break;
        };
        if piece.trim().is_empty() {
            break;
        }
        text.extend(piece.chars().take(72));
        used += 1;
    }
    let text = text.trim();
    if !text.is_empty() {
        dump.input_buffer = Some(text.to_string());
    }
    used
}

/// `* sqlservr                       00000000003D0000  0000000003F7DFFF  03bae000`
fn parse_module(line: &str) -> Option<Module> {
    let mut words = line.strip_prefix("* ")?.split_whitespace();
    let (name, base, end, size) = (words.next()?, words.next()?, words.next()?, words.next()?);
    let hex = |s: &str, len| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit());
    (words.next().is_none() && hex(base, 16) && hex(end, 16) && hex(size, 8)).then(|| Module {
        name: name.to_string(),
        base: base.to_string(),
        end: end.to_string(),
        size: size.to_string(),
    })
}

/// `0000000001C553A6 Module(sqlservr+00000000018853A6)`
fn parse_frame(line: &str) -> Option<Frame> {
    let (address, rest) = line.split_once(' ')?;
    if address.len() != 16 || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (module, offset) = module_offset(rest.trim());
    Some(Frame {
        address: address.to_string(),
        module,
        offset,
        function: None,
    })
}

/// `Module(sqllang+00000000006690A4)` into its name and offset.
fn module_offset(s: &str) -> (Option<String>, Option<String>) {
    let Some(inner) = s.strip_prefix("Module(").and_then(|s| s.split(')').next()) else {
        return (None, None);
    };
    match inner.rsplit_once('+') {
        Some((module, offset)) => (Some(module.to_string()), Some(offset.to_string())),
        None => (Some(inner.to_string()), None),
    }
}

/// The text after a leading `*` and spaces, or `None` when the line has no star.
fn star_content(line: &str) -> Option<&str> {
    line.trim_end().strip_prefix('*').map(str::trim)
}

/// `02/18/22 13:59:15 spid 126` into `2022-02-18T13:59:15`. SQL Server writes
/// month first whatever the locale (a dump of 27 June reads `06/27/13`).
fn iso_time(s: &str) -> Option<String> {
    let mut words = s.split_whitespace();
    let mut date = words.next()?.split('/');
    let (month, day, year) = (date.next()?, date.next()?, date.next()?);
    let time = words.next()?;
    let digits = |s: &str, n| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    (digits(month, 2) && digits(day, 2) && digits(year, 2) && time.len() == 8)
        .then(|| format!("20{year}-{month}-{day}T{time}"))
}

fn megabytes(s: &str) -> Option<u64> {
    s.trim().strip_suffix("MB")?.trim().parse().ok()
}

/// Product name from the build number. Unknown majors give `None`.
fn product(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let name = match (major, minor) {
        (9, _) => "2005",
        (10, 50) => "2008 R2",
        (10, _) => "2008",
        (11, _) => "2012",
        (12, _) => "2014",
        (13, _) => "2016",
        (14, _) => "2017",
        (15, _) => "2019",
        (16, _) => "2022",
        (17, _) => "2025",
        _ => return None,
    };
    Some(format!("SQL Server {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::lines;

    // Synthetic, in the layout of a 2008 R2 divide-by-zero dump.
    const EXCEPTION: &str = "Current time is 14:50:00 10/11/12.
=====================================================================
       BugCheck Dump
=====================================================================

This file is generated by Microsoft SQL Server
version 10.50.1600.1
upon detection of fatal unexpected error.

Computer type is Intel(R) Xeon(R) CPU           E5620  @ 2.40GHz.
16 X64 level 8664, 2 Mhz processor (s).
Windows NT 6.1 Build 7600 CSD .

Memory
MemoryLoad = 63%
Total Physical = 73716 MB
Available Physical = 27222 MB
***Stack Dump being sent to C:\\LOG\\SQLDump0002.txt
SqlDumpExceptionHandler: Process 98 generated fatal exception c0000094 EXCEPTION_INT_DIVIDE_BY_ZERO. SQL Server i
s terminating this process.
* *******************************************************************************
*
* BEGIN STACK DUMP:
*   10/11/12 14:50:00 spid 98
*
*
*   Exception Address = 0000000001C553A6 Module(sqlservr+00000000018853A6)
*   Exception Code    = c0000094 EXCEPTION_INT_DIVIDE_BY_ZERO
* Input Buffer 172 bytes -
*          SELECT a / b FROM dbo.t WHERE id = 1  UPDATE dbo.t SET b = 0 WHE
*  RE id = 2
*
*
*  MODULE                          BASE      END       SIZE
* sqlservr                       00000000003D0000  0000000003F7DFFF  03bae000
* ntdll                          00000000775E0000  000000007778AFFF  001ab000
*
*        Rax: 0000000000000010:
* *******************************************************************************
* -------------------------------------------------------------------------------
* Short Stack Dump
0000000001C553A6 Module(sqlservr+00000000018853A6)
0000000001C351CF Module(sqlservr+00000000018651CF)

CSession @0x000000081B5843B0
";

    // Synthetic, in the layout of a non-yielding scheduler dump.
    const NON_YIELDING: &str = "This file is generated by Microsoft SQL Server
version 11.0.3000.0
***Unable to get thread context for spid 0
* *******************************************************************************
*
* BEGIN STACK DUMP:
*   06/27/13 18:53:25 spid 18388
*
* Non-yielding Scheduler
*
* *******************************************************************************
";

    // Synthetic, in the layout of a 2025 DBCC STACKDUMP, frames shortened.
    const THREADS: &str = r#"Current time is 12:20:00 10/17/26.
=====================================================================
       Dump
=====================================================================

This file is generated by Microsoft SQL Server
version 17.0.1000.7
upon detection of fatal unexpected error.

Memory
MemoryLoad = 43%
**Dump thread - spid = 0, EC = 0x000001F9AA502730 Connection = 0x000001F9AA5020B0
*
* User initiated stack dump.  This is not a server exception dump.
*
Stack range 0x00000064EC9FE008 - 0x00000064ECA00000
<frame id="00" address="0x7FFEC7322D34" pdb="ntdll.pdb" age="1" guid="FF98FE65-248A-4F35-130A-D2BCEDC0BE37" module="ntdll" rva="0xA2D34" />
<frame id="00" address="0x7FFEA5A865CA" pdb="SqlDK.pdb" age="2" guid="7C8CC84D-DE6A-4C69-ADE9-26476DE1880A" module="SqlDK" rva="0x65CA" />
**Dump thread - spid = 61, EC = 0x000001F99D6C2730 Connection = 0x000001F99D6C20B0
*
* User initiated stack dump.  This is not a server exception dump.
*
Stack range 0x00000064E05FE0E8 - 0x00000064E0600000
<frame id="00" address="0x7FFEC34A9709" pdb="kernelbase.pdb" age="1" guid="E7C1891B-9880-87BD-7F07-596A6F866BC7" module="kernelbase" rva="0x39709" />
<frame id="00" address="0x7FFEA9A86FF6" pdb="sqlmin.pdb" age="2" guid="FA357897-3BDF-44C2-809E-7A18F87290F6" module="sqlmin" rva="0x1D71588" />
<frame id="00" address="0x7FFEA5A89165" pdb="SqlDK.pdb" age="2" guid="7C8CC84D-DE6A-4C69-ADE9-26476DE1880A" module="SqlDK" rva="0x9165" />
"#;

    /// Real files pad every line with spaces to the wrap width: 113 up to
    /// 2019, 187 on 2025.
    fn padded(text: &str) -> Vec<String> {
        padded_to(text, 113)
    }

    fn padded_to(text: &str, width: usize) -> Vec<String> {
        let text: Vec<String> = text.lines().map(|l| format!("{l:<width$}")).collect();
        lines(&text.join("\n"))
    }

    #[test]
    fn reads_the_threads_of_a_2025_dump() {
        let dump = parse(&padded_to(THREADS, 187));
        assert_eq!(dump.product.as_deref(), Some("SQL Server 2025"));
        assert_eq!(dump.time.as_deref(), Some("2026-10-17T12:20:00"));
        assert_eq!(dump.condition.as_deref(), Some("User initiated stack dump"));
        assert_eq!(dump.threads.len(), 2);
        assert_eq!(
            (dump.threads[0].spid, dump.threads[0].frames.len()),
            (Some(0), 2)
        );
        let t = &dump.threads[1];
        assert_eq!((t.spid, t.frames.len()), (Some(61), 3));
        assert_eq!(t.frames[1].module, "sqlmin");
        assert_eq!(t.frames[1].rva, "0x1D71588");
        assert_eq!(
            t.frames[1].debug_id.as_deref(),
            Some("FA3578973BDF44C2809E7A18F87290F62")
        );
    }

    #[test]
    fn reads_a_2025_dump_trigger() {
        let text = r#"This file is generated by Microsoft SQL Server
version 17.0.1000.7
**Dump thread - spid = 86, EC = 0x000001BA11DED7F0 Connection = 0x000001BA11DEFE90
* *******************************************************************************
*
* BEGIN STACK DUMP:
*   10/17/26 12:36:03 spid 86
*
* Server was configured to produce dump on the following event - ex_dump_if_requested: Exception raised, major=12, minor=5, state=51, severity=13
*
* *******************************************************************************
* -------------------------------------------------------------------------------
* Short Stack Dump
Stack range 0x0000000C70FF4480 - 0x0000000C71000000
<frame id="00" address="0x7FFC422F9709" pdb="kernelbase.pdb" age="1" guid="E7C1891B-9880-87BD-7F07-596A6F866BC7" module="kernelbase" rva="0x39709" />
<frame id="00" address="0x7FFC29A11E55" pdb="sqllang.pdb" age="2" guid="C2AB4FCA-3E24-4AAB-8C6F-2C3131B0041F" module="sqllang" rva="0x1381E55" />
"#;
        let dump = parse(&padded(text));
        assert!(
            dump.condition
                .as_deref()
                .is_some_and(|c| c.ends_with("major=12, minor=5, state=51, severity=13"))
        );
        assert!(dump.stack.is_empty());
        assert_eq!(dump.threads.len(), 1);
        assert_eq!(dump.threads[0].spid, Some(86));
        assert_eq!(dump.threads[0].frames[1].module, "sqllang");
    }

    #[test]
    fn the_dump_thread_line_of_older_versions_is_not_a_thread() {
        let text = format!("**Dump thread - spid = 0, EC = 0x0000000000000000\n{NON_YIELDING}");
        let dump = parse(&padded(&text));
        assert!(dump.threads.is_empty());
        assert_eq!(dump.condition.as_deref(), Some("Non-yielding Scheduler"));
    }

    #[test]
    fn reads_an_exception_dump() {
        let dump = parse(&padded(EXCEPTION));
        assert_eq!(dump.version.as_deref(), Some("10.50.1600.1"));
        assert_eq!(dump.product.as_deref(), Some("SQL Server 2008 R2"));
        assert_eq!(dump.time.as_deref(), Some("2012-10-11T14:50:00"));
        assert_eq!(dump.spid, Some(98));
        assert_eq!(dump.condition, None);
        let e = dump.exception.unwrap();
        assert_eq!(e.code, "c0000094");
        assert_eq!(e.name.as_deref(), Some("EXCEPTION_INT_DIVIDE_BY_ZERO"));
        assert_eq!(e.module.as_deref(), Some("sqlservr"));
        assert_eq!(e.offset.as_deref(), Some("00000000018853A6"));
        assert_eq!(
            dump.input_buffer.as_deref(),
            Some("SELECT a / b FROM dbo.t WHERE id = 1  UPDATE dbo.t SET b = 0 WHERE id = 2")
        );
        assert_eq!(dump.modules.len(), 2);
        assert_eq!(dump.modules[1].name, "ntdll");
        assert_eq!(dump.stack.len(), 2);
        assert_eq!(dump.stack[1].offset.as_deref(), Some("00000000018651CF"));
        assert_eq!(dump.memory_load_percent, Some(63));
        assert_eq!(dump.physical_memory_mb, Some(73716));
        assert_eq!(dump.os.as_deref(), Some("Windows NT 6.1 Build 7600 CSD"));
    }

    #[test]
    fn reads_a_condition_dump() {
        let dump = parse(&padded(NON_YIELDING));
        assert_eq!(dump.product.as_deref(), Some("SQL Server 2012"));
        assert_eq!(dump.condition.as_deref(), Some("Non-yielding Scheduler"));
        assert_eq!(dump.spid, Some(18388));
        assert!(dump.exception.is_none() && dump.stack.is_empty() && dump.modules.is_empty());
    }

    #[test]
    fn garbage_gives_an_empty_report() {
        let dump = parse(&lines(
            "* BEGIN STACK DUMP:\n*   99/99\n*  MODULE\n* x y\n* Short Stack Dump\nzz",
        ));
        assert!(dump.version.is_none() && dump.time.is_none() && dump.modules.is_empty());
    }
}
