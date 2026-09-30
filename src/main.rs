mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    env_logger::builder()
        .is_test(false)
        .parse_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp(None)
        .format_module_path(false)
        .format_level(true)
        .format_target(false)
        .write_style(env_logger::WriteStyle::Auto)
        .init();

    match cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        },
    }
}
