//! Reads `SQLDumpNNNN.mdmp` with Mozilla's `minidump` crate: the system, the
//! exception and its thread, the threads, and the loaded modules with the
//! identifiers that name their public PDB on the symbol server.
//!
//! The stack is not unwound here: a minidump carries neither the code nor
//! the unwind tables of the modules, which come with the symbols.

use minidump::{
    Minidump, MinidumpException, MinidumpModuleList, MinidumpSystemInfo, MinidumpThreadList,
    Module as _, system_info::Os,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct MiniDump {
    /// When the dump was written, UTC: `2022-06-02T14:04:39Z`.
    pub time: Option<String>,
    /// `Windows 10.0.19044`.
    pub os: Option<String>,
    pub cpu: Option<String>,
    pub processors: Option<u8>,
    pub exception: Option<Exception>,
    /// When the dump has no exception (non-yielding scheduler, latch timeout,
    /// DBCC): the thread and instruction its exception stream records anyway.
    pub dump_thread: Option<Location>,
    pub threads: usize,
    pub modules: Vec<Module>,
}

#[derive(Debug, Serialize)]
pub struct Exception {
    /// `c0000005`.
    pub code: String,
    /// `EXCEPTION_ACCESS_VIOLATION_READ`, as the `minidump` crate names it.
    pub reason: String,
    /// Where the faulting instruction is.
    #[serde(flatten)]
    pub at: Location,
    /// For an access violation, the address that was read or written.
    pub accessed_address: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Location {
    pub thread_id: u32,
    pub address: String,
    pub module: Option<String>,
    pub offset: Option<String>,
    /// From the public symbols: `CXStmtQuery::ErsqExecuteQuery+0x1A4`.
    pub function: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Module {
    /// `sqllang.dll`.
    pub name: String,
    pub path: String,
    pub base: String,
    pub size: String,
    /// File version from the module's version resource: `2019.150.2080.9`.
    pub version: Option<String>,
    /// `sqllang.pdb`.
    pub debug_file: Option<String>,
    /// The PDB's GUID and age as the symbol server keys them:
    /// `https://msdl.microsoft.com/download/symbols/<debug_file>/<debug_id>/<debug_file>`.
    pub debug_id: Option<String>,
    /// The image's timestamp and size as the symbol server keys binaries:
    /// `<name>/<code_id>/<name>`.
    pub code_id: Option<String>,
}

pub fn read(path: &std::path::Path) -> Result<MiniDump, String> {
    let dump = Minidump::read_path(path).map_err(|e| e.to_string())?;
    let system: Option<MinidumpSystemInfo> = dump.get_stream().ok();
    let modules: Option<MinidumpModuleList> = dump.get_stream().ok();
    let threads: Option<MinidumpThreadList> = dump.get_stream().ok();
    let exception: Option<MinidumpException> = dump.get_stream().ok();

    let mut dump_thread = None;
    let exception = exception.zip(system.as_ref()).and_then(|(e, system)| {
        let record = &e.raw.exception_record;
        let address = record.exception_address;
        let module = modules.as_ref().and_then(|m| m.module_at_address(address));
        let at = Location {
            thread_id: e.get_crashing_thread_id(),
            address: format!("{address:016X}"),
            module: module.map(|m| file_name(&m.code_file()).to_string()),
            offset: module.map(|m| format!("{:X}", address.saturating_sub(m.base_address()))),
            function: None,
        };
        if !is_exception_code(record.exception_code) {
            dump_thread = Some(at);
            return None;
        }
        let accessed = e.get_crash_address(system.os, system.cpu);
        Some(Exception {
            code: format!("{:08x}", record.exception_code),
            reason: e.get_crash_reason(system.os, system.cpu).to_string(),
            at,
            accessed_address: (accessed != address).then(|| format!("{accessed:016X}")),
        })
    });

    Ok(MiniDump {
        time: iso_utc(dump.header.time_date_stamp),
        os: system.as_ref().map(|s| {
            let r = &s.raw;
            let os = if s.os == Os::Windows {
                "Windows".to_string()
            } else {
                s.os.to_string()
            };
            format!(
                "{os} {}.{}.{}",
                r.major_version, r.minor_version, r.build_number
            )
        }),
        cpu: system.as_ref().map(|s| s.cpu.to_string()),
        processors: system.as_ref().map(|s| s.raw.number_of_processors),
        exception,
        dump_thread,
        threads: threads.map_or(0, |t| t.threads.len()),
        modules: modules
            .iter()
            .flat_map(MinidumpModuleList::by_addr)
            .map(|m| {
                let path = m.code_file();
                Module {
                    name: file_name(&path).to_string(),
                    path: path.to_string(),
                    base: format!("{:016X}", m.base_address()),
                    size: format!("{:X}", m.size()),
                    version: m.version().map(|v| v.to_string()),
                    debug_file: m.debug_file().map(|f| file_name(&f).to_string()),
                    debug_id: m.debug_identifier().map(|id| id.breakpad().to_string()),
                    code_id: m.code_identifier().map(|id| id.to_string()),
                }
            })
            .collect(),
    })
}

/// An NTSTATUS of error or warning severity (`c0000005`, `80000003`). SQL
/// Server writes an exception stream into dumps that have no exception, with
/// a code of 0 or a value that is not a status (`000042ac`, `59df37e0`).
fn is_exception_code(code: u32) -> bool {
    code & 0x8000_0000 != 0
}

/// The file name of a Windows path, whatever the platform reading it.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Seconds since 1970 as `YYYY-MM-DDThh:mm:ssZ`; `None` for zero, which a
/// writer leaves when it has no time.
fn iso_utc(seconds: u32) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let days = i64::from(seconds / 86_400);
    let secs = seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(iso_utc(0), None);
        assert_eq!(iso_utc(1).as_deref(), Some("1970-01-01T00:00:01Z"));
        assert_eq!(
            iso_utc(951_782_400).as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(
            iso_utc(1_654_178_679).as_deref(),
            Some("2022-06-02T14:04:39Z")
        );
        assert_eq!(iso_utc(u32::MAX).as_deref(), Some("2106-02-07T06:28:15Z"));
    }

    #[test]
    fn only_error_and_warning_statuses_are_exceptions() {
        assert!(is_exception_code(0xC000_0005) && is_exception_code(0x8000_0003));
        assert!(
            !is_exception_code(0) && !is_exception_code(0x42AC) && !is_exception_code(0x59DF_37E0)
        );
    }

    #[test]
    fn windows_file_names() {
        assert_eq!(file_name(r"C:\Program Files\x\sqllang.dll"), "sqllang.dll");
        assert_eq!(file_name("sqllang.dll"), "sqllang.dll");
    }

    /// A minidump with one module, `sqllang.dll`, and an exception stream
    /// whose code is `code`, at `sqllang+6690A4`.
    fn synthetic(code: u32) -> MiniDump {
        use minidump::format::{PlatformId, ProcessorArchitecture};
        use minidump_synth::{
            DumpString, Exception, Module, STOCK_VERSION_INFO, SynthMinidump, SystemInfo,
        };
        use test_assembler::{Endian, Section};

        let le = Endian::Little;
        let name = DumpString::new(r"C:\Program Files\Binn\sqllang.dll", le);
        let cv_record = Section::with_endian(le)
            .D32(minidump::format::CvSignature::Pdb70 as u32)
            .D32(0x3782_1027)
            .D16(0x6FE0)
            .D16(0x4165)
            .append_bytes(b"\xAC\x76\x95\x5D\xCD\x7C\x5A\xFA")
            .D32(2)
            .append_bytes(b"sqllang.pdb\0");
        let base = 0x7FFF_6942_0000_u64;
        let module = Module::new(
            le,
            base,
            0x0100_0000,
            &name,
            0,
            0,
            Some(&STOCK_VERSION_INFO),
        )
        .cv_record(&cv_record);
        let system = SystemInfo::new(le)
            .set_processor_architecture(ProcessorArchitecture::PROCESSOR_ARCHITECTURE_AMD64 as u16)
            .set_platform_id(PlatformId::VER_PLATFORM_WIN32_NT as u32);
        let mut exception = Exception::new(le);
        exception.thread_id = 1204;
        exception.exception_record.exception_code = code;
        exception.exception_record.exception_address = base + 0x0066_90A4;
        let bytes = SynthMinidump::with_endian(le)
            .add_system_info(system)
            .add_module(module)
            .add(name)
            .add(cv_record)
            .add_exception(exception)
            .finish()
            .unwrap();
        let path =
            std::env::temp_dir().join(format!("sqlautopsy-synth-{code:x}-{}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let dump = read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        dump
    }

    #[test]
    fn reads_an_exception_and_its_module() {
        let dump = synthetic(0xC000_0005);
        assert_eq!(dump.os.as_deref(), Some("Windows 0.0.0"));
        assert!(dump.dump_thread.is_none());
        let e = dump.exception.unwrap();
        assert_eq!(e.code, "c0000005");
        assert_eq!(e.at.thread_id, 1204);
        assert_eq!(e.at.module.as_deref(), Some("sqllang.dll"));
        assert_eq!(e.at.offset.as_deref(), Some("6690A4"));
        let m = &dump.modules[0];
        assert_eq!(m.path, r"C:\Program Files\Binn\sqllang.dll");
        assert_eq!(m.debug_file.as_deref(), Some("sqllang.pdb"));
        assert_eq!(
            m.debug_id.as_deref(),
            Some("378210276FE04165AC76955DCD7C5AFA2")
        );
    }

    #[test]
    fn a_dump_without_exception_records_its_thread() {
        let dump = synthetic(0x59DF_37E0);
        assert!(dump.exception.is_none());
        let at = dump.dump_thread.unwrap();
        assert_eq!((at.thread_id, at.offset.as_deref()), (1204, Some("6690A4")));
    }

    #[test]
    fn a_file_that_is_not_a_minidump_is_an_error() {
        let path = std::env::temp_dir().join(format!("sqlautopsy-notmdmp-{}", std::process::id()));
        std::fs::write(&path, b"MDMP but not really").unwrap();
        assert!(read(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
