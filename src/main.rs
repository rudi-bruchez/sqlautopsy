mod decode;
mod errorlog;
mod mdmp;
mod stackdump;
mod symbols;
mod xestack;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

const USAGE: &str = "usage: sqlautopsy [options] <folder-or-file>...
       sqlautopsy [options] stack [file]

Reads the SQLDumpNNNN.txt, .log and .mdmp files SQL Server writes into its LOG
folder and prints, for each dump: build, cause, exception, input buffer, stack
signature, short stack, threads and loaded modules.

When the .mdmp is there, the frames and the exception are named with
Microsoft's public symbols, downloaded once per build from
msdl.microsoft.com into a local cache.

stack reads Extended Events output captured with package0.callstack_rva
(SQL Server 2022 and later): a histogram or ring buffer target, an event_file
read back as XML, or the frames alone, from a file or standard input. It merges
identical stacks, counts them and names their frames.

Options:
  --format text|json  output format (default: text)
  --symbols <dir>     symbol cache (default: $SQLAUTOPSY_SYMBOLS, else
                      ~/.cache/sqlautopsy/symbols or %LOCALAPPDATA%)
  --offline           use the symbol cache only, never the network
  --no-symbols        do not resolve symbols

Exit codes: 0 success, 1 nothing could be read, 2 usage error.";

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

/// Options shared by both commands.
struct Options {
    json: bool,
    symbols: bool,
    offline: bool,
    cache: Option<PathBuf>,
}

impl Options {
    /// The symbol store, unless symbols are off or no cache folder is known.
    fn store(&self) -> Option<symbols::Symbols> {
        let cache = self.cache.clone().or_else(symbols::default_cache)?;
        self.symbols
            .then(|| symbols::Symbols::new(cache, self.offline))
    }

    fn print<T: Serialize>(&self, value: &T, text: impl FnOnce() -> String) -> Result<(), String> {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
            );
        } else {
            print!("{}", text());
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let mut options = Options {
        json: false,
        symbols: true,
        offline: false,
        cache: None,
    };
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
                Some("json") => options.json = true,
                Some("text") => options.json = false,
                _ => return usage_error("--format takes text or json"),
            },
            "--offline" => options.offline = true,
            "--no-symbols" => options.symbols = false,
            "--symbols" => match args.next() {
                Some(dir) => options.cache = Some(PathBuf::from(dir)),
                None => return usage_error("--symbols takes a folder"),
            },
            "-" => paths.push(PathBuf::from(arg)),
            s if s.starts_with('-') => return usage_error(&format!("unknown option {s}")),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let result = if paths.first().is_some_and(|p| p.as_os_str() == "stack") {
        match &paths[1..] {
            [] => stack_command(&options, None),
            [file] => stack_command(&options, Some(file)),
            _ => return usage_error("stack reads one file, or standard input"),
        }
    } else if paths.is_empty() {
        return usage_error("no folder or file given");
    } else {
        dumps_command(&options, &paths)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sqlautopsy: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Serialize)]
struct StackReport<'a> {
    schema: u32,
    stacks: &'a [xestack::Stack],
}

/// `sqlautopsy stack [file]`: names the frames of `callstack_rva` stacks.
fn stack_command(options: &Options, file: Option<&PathBuf>) -> Result<(), String> {
    let bytes = if let Some(f) = file.filter(|f| f.as_os_str() != "-") {
        std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?
    } else {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf).map_err(|e| e.to_string())?;
        buf
    };
    let mut stacks = xestack::parse(&decode::decode(&bytes));
    if stacks.is_empty() {
        return Err("no callstack_rva frame found (<frame ... rva=\"...\" />)".to_string());
    }
    if let Some(mut store) = options.store() {
        xestack::symbolize(&mut stacks, &mut store);
        for problem in &store.problems {
            eprintln!("sqlautopsy: symbols: {problem}");
        }
    }
    let report = StackReport {
        schema: SCHEMA,
        stacks: &stacks,
    };
    options.print(&report, || xestack::render(&stacks))
}

/// `sqlautopsy <folder-or-file>...`: reads `SQLDumper` files.
fn dumps_command(options: &Options, paths: &[PathBuf]) -> Result<(), String> {
    let groups = collect(paths)?;
    if groups.is_empty() {
        return Err("no SQLDumpNNNN.txt, .log or .mdmp file found".to_string());
    }
    let mut dumps: Vec<Dump> = groups
        .into_iter()
        .map(|(name, files)| analyze(name, &files))
        .collect();
    if let Some(mut store) = options.store() {
        for dump in &mut dumps {
            symbolize(dump, &mut store);
        }
        for problem in &store.problems {
            eprintln!("sqlautopsy: symbols: {problem}");
        }
    }
    let analyzed = dumps
        .iter()
        .any(|d| d.text.is_some() || d.errorlog.is_some() || d.minidump.is_some());
    let report = Report {
        schema: SCHEMA,
        dumps,
    };
    options.print(&report, || render(&report))?;
    for dump in &report.dumps {
        for problem in &dump.problems {
            eprintln!("sqlautopsy: {}: {problem}", dump.name);
        }
    }
    if analyzed {
        Ok(())
    } else {
        Err("no dump could be read".to_string())
    }
}

/// Names the frames and exception addresses of a dump with the public PDBs of
/// its modules, whose identifiers only the `.mdmp` carries, except in the
/// thread stacks of 2025, whose frames carry their own.
fn symbolize(dump: &mut Dump, store: &mut symbols::Symbols) {
    if let Some(text) = &mut dump.text {
        for frame in text.threads.iter_mut().flat_map(|t| t.frames.iter_mut()) {
            xestack::name(frame, store);
        }
    }
    let Some(minidump) = &mut dump.minidump else {
        return;
    };
    // By lowercase name and by stem: the .txt says `sqllang`, the .mdmp `sqllang.dll`.
    let mut pdbs: BTreeMap<String, (String, String)> = BTreeMap::new();
    for m in &minidump.modules {
        if let (Some(file), Some(id)) = (&m.debug_file, &m.debug_id) {
            let name = m.name.to_ascii_lowercase();
            let stem = name
                .rsplit_once('.')
                .map_or(name.as_str(), |(s, _)| s)
                .to_string();
            pdbs.insert(stem, (file.clone(), id.clone()));
            pdbs.insert(name, (file.clone(), id.clone()));
        }
    }
    let mut resolve = |module: Option<&String>, offset: Option<&String>| {
        let (file, id) = pdbs.get(&module?.to_ascii_lowercase())?;
        let rva = u32::try_from(u64::from_str_radix(offset?, 16).ok()?).ok()?;
        store.resolve(file, id, rva)
    };
    for location in minidump
        .exception
        .iter_mut()
        .map(|e| &mut e.at)
        .chain(minidump.dump_thread.iter_mut())
    {
        location.function = resolve(location.module.as_ref(), location.offset.as_ref());
    }
    if let Some(text) = &mut dump.text {
        if let Some(e) = &mut text.exception {
            e.function = resolve(e.module.as_ref(), e.offset.as_ref());
        }
        for frame in &mut text.stack {
            frame.function = resolve(frame.module.as_ref(), frame.offset.as_ref());
        }
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
            if let Some(f) = &e.function {
                write!(out, " {f}")?;
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
                let place = match (&frame.module, &frame.offset) {
                    (Some(m), Some(o)) => format!("{m}+{}", short_hex(o)),
                    _ => frame.address.clone(),
                };
                match &frame.function {
                    Some(f) => writeln!(out, "             {place:<20} {f}")?,
                    None => writeln!(out, "             {place}")?,
                }
            }
            if t.stack.len() > TEXT_FRAMES {
                writeln!(out, "             ...")?;
            }
        }
        if !t.modules.is_empty() {
            writeln!(out, "  modules    {} loaded", t.modules.len())?;
        }
        render_threads(out, &t.threads)?;
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

/// The count of 2025 thread stacks, then those of the threads that ran a
/// session: the others are the server's own workers, waiting.
fn render_threads(out: &mut String, threads: &[stackdump::Thread]) -> std::fmt::Result {
    if threads.is_empty() {
        return Ok(());
    }
    let sessions: Vec<_> = threads
        .iter()
        .filter(|t| t.spid.unwrap_or(0) != 0)
        .collect();
    writeln!(
        out,
        "  threads    {} stacks, {} in a session",
        threads.len(),
        sessions.len()
    )?;
    for thread in sessions {
        writeln!(
            out,
            "  spid {:<5} {} frames",
            thread.spid.unwrap_or(0),
            thread.frames.len()
        )?;
        for f in thread.frames.iter().take(TEXT_FRAMES) {
            let place = format!("{}+{}", f.module, f.rva.trim_start_matches("0x"));
            match &f.function {
                Some(function) => writeln!(out, "             {place:<20} {function}")?,
                None => writeln!(out, "             {place}")?,
            }
        }
        if thread.frames.len() > TEXT_FRAMES {
            writeln!(out, "             ...")?;
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
        if let Some(f) = &e.at.function {
            write!(out, " {f}")?;
        }
        writeln!(out, ", thread {}", e.at.thread_id)?;
    }
    if let Some(d) = &m.dump_thread {
        write!(out, "             no exception; thread {}", d.thread_id)?;
        if let (Some(module), Some(offset)) = (&d.module, &d.offset) {
            write!(out, " at {module}+{offset}")?;
        }
        if let Some(f) = &d.function {
            write!(out, " {f}")?;
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
