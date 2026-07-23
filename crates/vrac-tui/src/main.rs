use std::process::ExitCode;

fn main() -> ExitCode {
    match vrac_tui::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
