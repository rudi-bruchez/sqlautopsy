//! Reads `SQLDumpNNNN.log`: the last 64 KB of the errorlog at dump time,
//! UTF-16 LE without a BOM, whose first entry is usually cut.

use serde::Serialize;

/// Entries kept from before the dump: enough for the errors that led to it,
/// few enough not to drown them in backup messages.
const CONTEXT_ENTRIES: usize = 10;

#[derive(Debug, Default, Serialize)]
pub struct ErrorLog {
    /// `0x00000000000000A1`. Dumps of the same signature are the same defect.
    pub stack_signature: Option<String>,
    /// The entries written just before the dump, oldest first, as
    /// `2022-02-18 13:59:15.01 spid126     message`.
    pub before_dump: Vec<String>,
}

/// `txt_name` is the dump's text file name, `SQLDump0008.txt`: the log of a
/// dump can also hold the entries of an earlier one. The dump is found by the
/// `Stack Dump being sent to ...\SQLDump0008.txt` entry, or, for the dumps that
/// write none (non-yielding schedulers), by the last `BEGIN STACK DUMP:` entry,
/// the log being the errorlog's tail at the time of this dump.
pub fn parse(lines: &[String], txt_name: &str) -> ErrorLog {
    let entries = entries(lines);
    let suffix = format!("\\{}", txt_name.to_ascii_lowercase());
    let sent = entries.iter().position(|e| {
        e.contains("Stack Dump being sent to") && e.to_ascii_lowercase().ends_with(&suffix)
    });
    let Some(anchor) = sent.or_else(|| {
        entries
            .iter()
            .rposition(|e| message(e).starts_with("* BEGIN STACK DUMP:"))
    }) else {
        return ErrorLog::default();
    };
    let stack_signature = entries[anchor..]
        .iter()
        .find_map(|e| e.split("Stack Signature for the dump is ").nth(1))
        .map(|s| s.trim().to_string());
    // The dump's own preamble (star lines, "Using 'dbghelp.dll'") is not context.
    let preamble = ["*", "Using 'dbghelp.dll'", "CImageHelper::"];
    let start = entries[..anchor]
        .iter()
        .rposition(|e| !preamble.iter().any(|p| message(e).starts_with(p)))
        .map_or(0, |i| i + 1);
    ErrorLog {
        stack_signature,
        before_dump: entries[start.saturating_sub(CONTEXT_ENTRIES)..start].to_vec(),
    }
}

/// Groups lines into entries: an entry starts with `YYYY-MM-DD hh:mm:ss.cc`,
/// other lines continue the previous one. Lines before the first entry are
/// the cut tail of an older one and are dropped.
fn entries(lines: &[String]) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for line in lines {
        let line = line.trim_end();
        if is_timestamped(line) {
            entries.push(line.to_string());
        } else if let Some(last) = entries.last_mut()
            && !line.is_empty()
        {
            last.push(' ');
            last.push_str(line.trim());
        }
    }
    entries
}

fn is_timestamped(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() > 23
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
        && b[13] == b':'
        && b[19] == b'.'
}

/// The message of an entry, after the timestamp and the source column.
fn message(entry: &str) -> &str {
    entry
        .get(23..)
        .unwrap_or_default()
        .trim_start()
        .split_once(' ')
        .map_or("", |(_, m)| m.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::lines;

    const LOG: &str = "ase: model, creation date(time): 2003/04/08(09:13:36), pages dumped: 186.
2022-02-18 13:58:00.10 spid51      Error: 823, Severity: 24, State: 2.
2022-02-18 13:58:00.10 spid51      The operating system returned error 21
to SQL Server during a read.
2022-02-18 13:59:15.01 spid126     Using 'dbghelp.dll' version '4.0.5'
2022-02-18 13:59:15.01 spid126     **Dump thread - spid = 0, EC = 0x00000189B1CA4A30
2022-02-18 13:59:15.01 spid126     ***Stack Dump being sent to D:\\LOG\\SQLDump0008.txt
2022-02-18 13:59:15.02 spid126     * BEGIN STACK DUMP:
2022-02-18 13:59:15.05 spid126     Stack Signature for the dump is 0x00000000000000A1
";

    #[test]
    fn finds_the_signature_and_what_came_before() {
        let log = parse(&lines(LOG), "SQLDump0008.txt");
        assert_eq!(log.stack_signature.as_deref(), Some("0x00000000000000A1"));
        assert_eq!(log.before_dump.len(), 2);
        assert!(log.before_dump[0].ends_with("Error: 823, Severity: 24, State: 2."));
        assert!(log.before_dump[1].ends_with("error 21 to SQL Server during a read."));
    }

    #[test]
    fn a_dump_without_a_file_line_is_found_by_its_block() {
        let log = "2013-06-27 18:52:54.05 spid8s      SQL Server has encountered 1 occurrence(s) of I/O requests taking longer than 15 seconds
2013-06-27 18:53:01.39 Server      Using 'dbghelp.dll' version '4.0.5'
2013-06-27 18:53:25.34 Server      ***Unable to get thread context for spid 0
2013-06-27 18:53:25.34 Server      * BEGIN STACK DUMP:
2013-06-27 18:53:25.34 Server      * Non-yielding Scheduler
2013-06-27 18:53:25.44 Server      Stack Signature for the dump is 0x0000000000000336
";
        let log = parse(&lines(log), "SQLDump0005.txt");
        assert_eq!(log.stack_signature.as_deref(), Some("0x0000000000000336"));
        assert_eq!(log.before_dump.len(), 1);
        assert!(log.before_dump[0].contains("longer than 15 seconds"));
    }

    #[test]
    fn a_log_without_a_dump_gives_nothing() {
        let log = parse(
            &lines("2022-02-18 13:58:00.10 spid51      Starting up.\n"),
            "SQLDump0009.txt",
        );
        assert!(log.stack_signature.is_none() && log.before_dump.is_empty());
    }
}
