#![forbid(unsafe_op_in_unsafe_fn)]

mod app;
mod diagnostics;
mod dump;
mod source;

use std::process::ExitCode;

fn main() -> ExitCode {
    match app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if app::is_broken_pipe(&error) => ExitCode::SUCCESS,
        Err(error) => {
            diagnostics::report(&error);
            ExitCode::FAILURE
        }
    }
}
