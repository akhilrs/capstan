use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("cstan-dash {}", env!("CSTAN_DASH_VERSION"));
        return ExitCode::SUCCESS;
    }
    cstan_dash::app::run(args)
}
