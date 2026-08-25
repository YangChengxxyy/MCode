//! `mcode` binary — composition root. The real CLI wiring lives in
//! [`mcode_cli`] (M1 T6); this thin forwarder only provides the
//! executable.

use std::process::ExitCode;

fn main() -> ExitCode {
    mcode_cli::main()
}
