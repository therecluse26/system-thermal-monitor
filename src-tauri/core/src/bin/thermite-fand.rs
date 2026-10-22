//! `thermite-fand <user>`: the fan daemon. See `thermite_core::daemon`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [user] = args.as_slice() else {
        eprintln!("usage: thermite-fand <user>");
        return ExitCode::from(2);
    };
    thermite_core::daemon::main(user)
}
