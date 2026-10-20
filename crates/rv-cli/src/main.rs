//! `rv`: command-line front end. See `rv --help`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = rv_cli::run(
        &args,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
