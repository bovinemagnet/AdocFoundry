use std::process::ExitCode;

fn main() -> ExitCode {
    match adoc_ls::server::run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("adoc-ls: {error}");
            ExitCode::FAILURE
        }
    }
}
