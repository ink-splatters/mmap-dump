use std::error::Error;
use std::io::Write;

use anstyle::{AnsiColor, Style};

const ERROR_STYLE: Style = AnsiColor::BrightRed.on_default().bold();
const CAUSE_STYLE: Style = AnsiColor::BrightBlack.on_default();

pub fn report(error: &(dyn Error + 'static)) {
    let mut stderr = anstream::stderr().lock();
    let _ = write_report(&mut stderr, error);
}

fn write_report<W: Write>(output: &mut W, error: &(dyn Error + 'static)) -> std::io::Result<()> {
    writeln!(output, "{ERROR_STYLE}error{ERROR_STYLE:#}: {error}")?;

    let mut source = error.source();
    while let Some(cause) = source {
        writeln!(output, "  {CAUSE_STYLE}caused by{CAUSE_STYLE:#}: {cause}")?;
        source = cause.source();
    }

    Ok(())
}
