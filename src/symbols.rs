//! Microsoft's public symbols: download a module's PDB from the symbol server
//! into a SymStore-layout cache, then name the function an offset falls in.
//!
//! Public PDBs are stripped: they hold the public symbols (function names and
//! their start addresses) and nothing else, so a name comes with the offset
//! from the function's start, never a line number.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const SERVER: &str = "https://msdl.microsoft.com/download/symbols";

/// The public PDB of `sqllang.dll` 15.0.2080.9 is 86 MB; refuse anything far
/// beyond that rather than fill a disk.
const MAX_PDB_BYTES: u64 = 1024 * 1024 * 1024;

pub struct Symbols {
    cache: PathBuf,
    offline: bool,
    /// Loaded tables by `<pdb>/<id>`, `None` when the PDB could not be had.
    tables: HashMap<String, Option<Table>>,
    /// Why a PDB could not be had, reported once each.
    pub problems: Vec<String>,
}

/// Function start addresses (RVA), sorted, with their names.
struct Table(Vec<(u32, String)>);

impl Symbols {
    pub fn new(cache: PathBuf, offline: bool) -> Self {
        Symbols {
            cache,
            offline,
            tables: HashMap::new(),
            problems: Vec::new(),
        }
    }

    /// The function containing `rva` in the module whose PDB is `pdb`
    /// (`sqllang.pdb`) with identifier `id` (GUID and age), as
    /// `CXStmtQuery::ErsqExecuteQuery+0x1A4`.
    pub fn resolve(&mut self, pdb: &str, id: &str, rva: u32) -> Option<String> {
        let key = format!("{pdb}/{id}");
        if !self.tables.contains_key(&key) {
            let table = self
                .load(pdb, id)
                .map_err(|e| self.problems.push(format!("{key}: {e}")));
            self.tables.insert(key.clone(), table.ok());
        }
        self.tables.get(&key)?.as_ref()?.lookup(rva)
    }

    fn load(&self, pdb: &str, id: &str) -> Result<Table, String> {
        let path = self.fetch(pdb, id)?;
        Table::read(&path)
    }

    /// The cached PDB, downloaded first unless offline.
    fn fetch(&self, pdb: &str, id: &str) -> Result<PathBuf, String> {
        // Both come from the dump: refuse anything that is not a plain file
        // name and a hex identifier before they become a path or a URL.
        let plain = |s: &str| {
            !s.is_empty()
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        if !plain(pdb) || !id.bytes().all(|b| b.is_ascii_hexdigit()) || id.is_empty() {
            return Err("not a symbol server name".to_string());
        }
        let path = self.cache.join(pdb).join(id).join(pdb);
        if path.is_file() {
            return Ok(path);
        }
        if self.offline {
            return Err(format!("not in the cache {}", self.cache.display()));
        }
        let url = format!("{SERVER}/{pdb}/{id}/{pdb}");
        let mut response = ureq::get(&url).call().map_err(|e| e.to_string())?;
        let size = response.body().content_length();
        eprintln!(
            "sqlautopsy: downloading {pdb}{} from msdl.microsoft.com",
            size.map_or(String::new(), |s| format!(" ({} MB)", s / 1_000_000))
        );
        let dir = path.parent().ok_or("no cache directory")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // Write beside the target and rename, so an interrupted download never
        // leaves a truncated PDB in the cache.
        let partial = dir.join(format!("{pdb}.partial-{}", std::process::id()));
        let result = (|| {
            let mut file = File::create(&partial).map_err(|e| e.to_string())?;
            let mut body = response
                .body_mut()
                .with_config()
                .limit(MAX_PDB_BYTES)
                .reader();
            std::io::copy(&mut body, &mut file).map_err(|e| e.to_string())?;
            file.flush().map_err(|e| e.to_string())?;
            std::fs::rename(&partial, &path).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        result.map(|()| path)
    }
}

impl Table {
    fn read(path: &Path) -> Result<Table, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut pdb = pdb::PDB::open(file).map_err(|e| e.to_string())?;
        let symbols = pdb.global_symbols().map_err(|e| e.to_string())?;
        let address_map = pdb.address_map().map_err(|e| e.to_string())?;
        let mut functions = Vec::new();
        let mut iter = symbols.iter();
        while let Some(symbol) =
            pdb::FallibleIterator::next(&mut iter).map_err(|e| e.to_string())?
        {
            if let Ok(pdb::SymbolData::Public(public)) = symbol.parse()
                && public.function
                && let Some(rva) = public.offset.to_rva(&address_map)
            {
                functions.push((rva.0, public.name.to_string().into_owned()));
            }
        }
        functions.sort_unstable_by_key(|f| f.0);
        Ok(Table(functions))
    }

    fn lookup(&self, rva: u32) -> Option<String> {
        let i = self.0.partition_point(|f| f.0 <= rva).checked_sub(1)?;
        let (start, name) = &self.0[i];
        Some(format!("{}+0x{:X}", demangle(name), rva - start))
    }
}

/// `?ErsqExecuteQuery@CXStmtQuery@@...` as `CXStmtQuery::ErsqExecuteQuery`;
/// a name that does not demangle is kept as it is.
fn demangle(name: &str) -> String {
    msvc_demangler::demangle(name, msvc_demangler::DemangleFlags::NAME_ONLY)
        .unwrap_or_else(|_| name.to_string())
}

/// `$SQLAUTOPSY_SYMBOLS`, else the platform's cache directory:
/// `$XDG_CACHE_HOME` or `~/.cache` on Linux and macOS, `%LOCALAPPDATA%` on
/// Windows, followed by `sqlautopsy/symbols`.
pub fn default_cache() -> Option<PathBuf> {
    let env = |k| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = env("SQLAUTOPSY_SYMBOLS") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")?
    } else {
        env("XDG_CACHE_HOME").or_else(|| Some(env("HOME")?.join(".cache")))?
    };
    Some(base.join("sqlautopsy").join("symbols"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_the_enclosing_function() {
        let table = Table(vec![(0x1000, "?f@A@@QEAAXXZ".into()), (0x2000, "g".into())]);
        assert_eq!(table.lookup(0xFFF), None);
        assert_eq!(table.lookup(0x1000).as_deref(), Some("A::f+0x0"));
        assert_eq!(table.lookup(0x1FFF).as_deref(), Some("A::f+0xFFF"));
        assert_eq!(table.lookup(0x2010).as_deref(), Some("g+0x10"));
    }

    #[test]
    fn refuses_names_that_would_leave_the_cache() {
        let mut s = Symbols::new(std::env::temp_dir().join("sqlautopsy-never"), true);
        for (pdb, id) in [
            ("../x.pdb", "AB"),
            ("a/b.pdb", "AB"),
            ("..", "AB"),
            ("x.pdb", "../AB"),
            ("x.pdb", ""),
        ] {
            assert_eq!(
                s.fetch(pdb, id),
                Err("not a symbol server name".to_string()),
                "{pdb} {id}"
            );
        }
        assert!(s.resolve("x.pdb", "AB", 0).is_none());
        assert_eq!(s.problems.len(), 1);
    }
}
