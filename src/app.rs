use std::error::Error;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::PathBuf;

use clap::Parser;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use thiserror::Error;

use crate::dump::{self, DumpError, MappingMode};
use crate::source::{LockedFile, SourceError};

const OUTPUT_BUFFER_CAPACITY: usize = 64 * 1024;

/// Write a file's contents to standard output, starting at a byte offset.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// File to read
    #[arg(value_name = "FILE")]
    file_path: PathBuf,

    /// Byte offset at which to start
    #[arg(value_name = "OFFSET")]
    offset: u64,

    /// Try one mapping for the remaining range before falling back to smaller mappings
    #[arg(long = "map-whole-file", short = 'w')]
    map_whole_file: bool,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Source(#[from] SourceError),

    #[error(transparent)]
    Dump(#[from] DumpError),

    #[error("failed to flush standard output")]
    Flush(#[source] io::Error),
}

pub fn run() -> Result<(), AppError> {
    let args = Args::parse();
    let file = LockedFile::open(&args.file_path)?;
    let progress = progress_bar(file.len().saturating_sub(args.offset), &args.file_path);
    let stdout = io::stdout();
    let tracked_stdout = progress.wrap_write(stdout.lock());
    let mut output = BufWriter::with_capacity(OUTPUT_BUFFER_CAPACITY, tracked_stdout);

    let result = (|| {
        dump_file(&args, &file, &mut output)?;
        output.flush().map_err(AppError::Flush)
    })();

    drop(output);
    progress.finish_and_clear();
    result
}

pub fn is_broken_pipe(error: &AppError) -> bool {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);

    while let Some(cause) = current {
        if cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
        {
            return true;
        }
        current = cause.source();
    }

    false
}

fn dump_file<W: Write>(args: &Args, file: &LockedFile, output: &mut W) -> Result<(), DumpError> {
    let mode = if args.map_whole_file {
        MappingMode::WholeFile
    } else {
        MappingMode::Chunked
    };

    dump::file_range(file, args.offset, mode, output)
}

fn progress_bar(total_bytes: u64, path: &std::path::Path) -> ProgressBar {
    // Raw stdout and terminal rendering cannot safely share the same screen.
    // The stderr target also hides itself when stderr is not user-attended.
    let draw_target = if io::stdout().is_terminal() {
        ProgressDrawTarget::hidden()
    } else {
        ProgressDrawTarget::stderr()
    };
    let style = ProgressStyle::with_template(
        "{spinner:.cyan} {msg} [{wide_bar:.cyan/blue}] \
         {bytes}/{total_bytes} {bytes_per_sec} ETA {eta}",
    )
    .expect("the static progress template is valid")
    .progress_chars("━╸─");
    let progress = ProgressBar::with_draw_target(Some(total_bytes), draw_target).with_style(style);
    let display_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    progress.set_message(display_name);
    progress
}

#[cfg(test)]
mod tests {
    use std::io;

    use clap::Parser;

    use super::{AppError, Args, is_broken_pipe};

    #[test]
    fn parses_the_whole_file_options() {
        for option in ["--map-whole-file", "-w"] {
            let args = Args::try_parse_from(["mmap-dump", "input", "12", option])
                .expect("arguments should parse");

            assert!(args.map_whole_file);
            assert_eq!(args.offset, 12);
        }
    }

    #[test]
    fn finds_a_broken_pipe_below_context() {
        let error = AppError::Flush(io::Error::new(io::ErrorKind::BrokenPipe, "reader closed"));

        assert!(is_broken_pipe(&error));
    }

    #[test]
    fn does_not_misclassify_other_io_errors() {
        let error = AppError::Flush(io::Error::new(io::ErrorKind::WriteZero, "no progress"));

        assert!(!is_broken_pipe(&error));
    }
}
