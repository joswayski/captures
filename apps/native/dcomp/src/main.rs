#[cfg(windows)]
mod host;
#[path = "../../common/options.rs"]
#[cfg_attr(not(windows), allow(dead_code))]
mod options;
#[cfg(windows)]
mod renderer;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod scene;

fn main() {
    #[cfg(windows)]
    if let Err(error) = host::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        // Parse on every platform so the shared CLI contract is testable here.
        let _ = options::Options::parse(std::env::args().skip(1));
        eprintln!(
            "DirectComposition requires Windows; no browser fallback.\n{}",
            options::USAGE
        );
        std::process::exit(3);
    }
}
