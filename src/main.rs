use std::process::ExitCode;

const USAGE: &str = "usage: sqlautopsy <dump-folder>

Reads the SQLDump files SQL Server writes into its LOG folder.
Nothing is implemented yet: see https://github.com/rudi-bruchez/sqlautopsy";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("sqlautopsy {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
