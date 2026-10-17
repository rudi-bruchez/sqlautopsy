mod decode;
mod errorlog;
mod mdmp;
mod stackdump;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

const USAGE: &str = "usage: sqlautopsy [--format text|json] <folder-or-file>...

Reads the SQLDumpNNNN.txt, .log and .mdmp files SQL Server writes into its LOG
folder and prints, for each dump: build, cause, exception, input buffer, stack
signature, short stack, threads and loaded modules. Frames are module+offset:
symbols are not resolved yet.

Exit codes: 0 success, 1 no dump could be read, 2 usage error.";

/// Bumped when a field is renamed or removed; adding one keeps it.
const SCHEMA: u32 = 1;

/// A `.txt` or `.log` larger than this is not a `SQLDumper` file.
const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;

/// Frames printed in text mode; JSON carries them all.
const TEXT_FRAMES: usize = 12;

#[derive(Serialize)]
struct Report {
    schema: u32,
    dumps: Vec<Dump>,
}

#[derive(Serialize)]
struct Dump {
    /// `SQLDump0008`.
    name: String,
    files: Vec<String>,
    text: Option<stackdump::StackDump>,
    errorlog: Option<errorlog::ErrorLog>,
    minidump: Option<mdmp::MiniDump>,
    /// Files that could not be read, with the reason.
    problems: Vec<String>,
}

fn main() -> ExitCode {
    let mut json = false;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => {
                println!("sqlautopsy {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--format" => match args.next().as_deref() {
                Some("json") => json = true,
                Some("text") => json = false,
                _ => return usage_error("--format takes text or json"),
            },
            s if s.starts_with('-') => return usage_error(&format!("unknown option {s}")),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.is_empty() {
        return usage_error("no folder or file given");
    }

    let groups = match collect(&paths) {
        Ok(groups) => groups,
        Err(e) => {
            eprintln!("sqlautopsy: {e}");
            return ExitCode::FAILURE;
        }
    };
    if groups.is_empty() {
        eprintln!("sqlautopsy: no SQLDumpNNNN.txt, .log or .mdmp file found");
        return ExitCode::FAILURE;
    }
    let dumps: Vec<Dump> = groups
        .into_iter()
        .map(|(name, files)| analyze(name, &files))
        .collect();
    let analyzed = dumps
        .iter()
        .any(|d| d.text.is_some() || d.errorlog.is_some() || d.minidump.is_some());
    let report = Report {
        schema: SCHEMA,
        dumps,
    };
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("sqlautopsy: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        print!("{}", render(&report));
    }
    for dump in &report.dumps {
        for problem in &dump.problems {
            eprintln!("sqlautopsy: {}: {problem}", dump.name);
        }
    }
    if analyzed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("sqlautopsy: {message}\n\n{USAGE}");
    ExitCode::from(2)
}

/// Dump files by dump name (`SQLDump0008`), from folders (not recursive) and
/// files named on the command line.
fn collect(paths: &[PathBuf]) -> Result<BTreeMap<String, Vec<PathBuf>>, String> {
    let mut groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in paths {
        let files = if path.is_dir() {
            let entries =
                std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
            entries.filter_map(Result::ok).map(|e| e.path()).collect()
        } else if path.is_file() {
            vec![path.clone()]
        } else {
            return Err(format!("{}: no such file or folder", path.display()));
        };
        for file in files {
            if let Some(name) = dump_name(&file) {
                groups.entry(name).or_default().push(file);
            }
        }
    }
    for files in groups.values_mut() {
        files.sort();
        files.dedup();
    }
    Ok(groups)
}

/// `SQLDump0008` for `.../SQLDump0008.txt`, `None` for anything else.
fn dump_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let digits = stem.get(7..)?;
    let is_dump = stem.get(..7)?.eq_ignore_ascii_case("SQLDump")
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && matches!(ext.as_str(), "txt" | "log" | "mdmp");
    is_dump.then(|| format!("SQLDump{digits}"))
}

fn analyze(name: String, files: &[PathBuf]) -> Dump {
    let with_ext = |ext: &str| {
        files.iter().find(|f| {
            f.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
    };
    let mut dump = Dump {
        files: files
            .iter()
            .filter_map(|f| f.file_name()?.to_str().map(str::to_string))
            .collect(),
        text: None,
        errorlog: None,
        minidump: None,
        problems: Vec::new(),
        name,
    };
    if let Some(txt) = with_ext("txt") {
        match read_lines(txt) {
            Ok(lines) => dump.text = Some(stackdump::parse(&lines)),
            Err(e) => dump.problems.push(e),
        }
    }
    if let Some(log) = with_ext("log") {
        let txt_name = format!("{}.txt", dump.name);
        match read_lines(log) {
            Ok(lines) => dump.errorlog = Some(errorlog::parse(&lines, &txt_name)),
            Err(e) => dump.problems.push(e),
        }
    }
    if let Some(mdmp) = with_ext("mdmp") {
        match mdmp::read(mdmp) {
            Ok(m) => dump.minidump = Some(m),
            Err(e) => dump.problems.push(format!("{}.mdmp: {e}", dump.name)),
        }
    }
    dump
}

fn read_lines(path: &Path) -> Result<Vec<String>, String> {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{name}: {e}"))?
        .len();
    if size > MAX_TEXT_BYTES {
        return Err(format!(
            "{name}: {size} bytes, larger than a SQLDumper text file"
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    Ok(decode::lines(&decode::decode(&bytes)))
}

fn render(report: &Report) -> String {
    let mut out = String::new();
    for (i, dump) in report.dumps.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Writing to a String cannot fail.
        let _ = render_dump(&mut out, dump);
    }
    out
}

fn render_dump(out: &mut String, dump: &Dump) -> std::fmt::Result {
    let t = dump.text.as_ref();
    let m = dump.minidump.as_ref();
    let field = |v: Option<&str>| v.unwrap_or("?").to_string();
    writeln!(
        out,
        "{}  {}  {} {}",
        dump.name,
        field(
            t.and_then(|t| t.time.as_deref())
                .or_else(|| m.and_then(|m| m.time.as_deref()))
        ),
        field(t.and_then(|t| t.product.as_deref())),
        field(t.and_then(|t| t.version.as_deref())),
    )?;
    if t.is_none_or(|t| t.version.is_none())
        && let Some(v) = m.and_then(|m| {
            let exe = m
                .modules
                .iter()
                .find(|x| x.name.eq_ignore_ascii_case("sqlservr.exe"));
            exe?.version.as_deref()
        })
    {
        // The file version does not map to the build: 2008 R2 10.50.1600.1 is 2009.100.1600.1.
        writeln!(out, "  sqlservr   file version {v}")?;
    }
    writeln!(out, "  files      {}", dump.files.join(", "))?;
    if let Some(t) = t {
        if let Some(spid) = t.spid {
            writeln!(out, "  spid       {spid}")?;
        }
        if let Some(c) = &t.condition {
            writeln!(out, "  cause      {c}")?;
        }
        if let Some(e) = &t.exception {
            write!(out, "  exception  {}", e.code)?;
            if let Some(n) = &e.name {
                write!(out, " {n}")?;
            }
            if let (Some(m), Some(o)) = (&e.module, &e.offset) {
                write!(out, " in {m}+{}", short_hex(o))?;
            }
            writeln!(out)?;
        }
    }
    if let Some(sig) = dump
        .errorlog
        .as_ref()
        .and_then(|l| l.stack_signature.as_deref())
    {
        writeln!(out, "  signature  {sig}")?;
    }
    if let Some(t) = t {
        if let Some(buffer) = &t.input_buffer {
            let one_line = buffer.split_whitespace().collect::<Vec<_>>().join(" ");
            let shown: String = one_line.chars().take(200).collect();
            let more = if shown.len() < one_line.len() {
                " ..."
            } else {
                ""
            };
            writeln!(out, "  input      {shown}{more}")?;
        }
        if !t.stack.is_empty() {
            writeln!(out, "  stack      {} frames", t.stack.len())?;
            for frame in t.stack.iter().take(TEXT_FRAMES) {
                match (&frame.module, &frame.offset) {
                    (Some(m), Some(o)) => writeln!(out, "             {m}+{}", short_hex(o))?,
                    _ => writeln!(out, "             {}", frame.address)?,
                }
            }
            if t.stack.len() > TEXT_FRAMES {
                writeln!(out, "             ...")?;
            }
        }
        if !t.modules.is_empty() {
            writeln!(out, "  modules    {} loaded", t.modules.len())?;
        }
    }
    if let Some(m) = m {
        render_minidump(out, m)?;
    }
    if let Some(log) = &dump.errorlog
        && !log.before_dump.is_empty()
    {
        writeln!(out, "  errorlog before the dump")?;
        for entry in &log.before_dump {
            writeln!(out, "             {entry}")?;
        }
    }
    Ok(())
}

fn render_minidump(out: &mut String, m: &mdmp::MiniDump) -> std::fmt::Result {
    writeln!(
        out,
        "  minidump   {}, {} threads, {} modules",
        m.os.as_deref().unwrap_or("?"),
        m.threads,
        m.modules.len()
    )?;
    if let Some(e) = &m.exception {
        write!(out, "             exception {} {}", e.code, e.reason)?;
        if let (Some(module), Some(offset)) = (&e.at.module, &e.at.offset) {
            write!(out, " in {module}+{offset}")?;
        }
        writeln!(out, ", thread {}", e.at.thread_id)?;
    }
    if let Some(d) = &m.dump_thread {
        write!(out, "             no exception; thread {}", d.thread_id)?;
        if let (Some(module), Some(offset)) = (&d.module, &d.offset) {
            write!(out, " at {module}+{offset}")?;
        }
        writeln!(out)?;
    }
    Ok(())
}

/// `00000000018853A6` as `18853A6`, the form `WinDbg` and KB articles use.
fn short_hex(s: &str) -> &str {
    let trimmed = s.trim_start_matches('0');
    if trimmed.is_empty() { "0" } else { trimmed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_dump_file_names() {
        let name = |p: &str| dump_name(Path::new(p));
        assert_eq!(name("LOG/SQLDump0008.txt").as_deref(), Some("SQLDump0008"));
        assert_eq!(name("sqldump0012.MDMP").as_deref(), Some("SQLDump0012"));
        assert_eq!(name("SQLDUMPER_ERRORLOG.log"), None);
        assert_eq!(name("SQLDump.txt"), None);
        assert_eq!(name("SQLDump0008.zip"), None);
    }
}
