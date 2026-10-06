//! The `verus-lint` command: the CLI with no Rust rules of its own.

fn main() -> std::process::ExitCode {
    verus_lint::run(&[])
}
