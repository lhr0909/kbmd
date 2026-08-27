fn main() {
    if let Err(error) = kbmd::cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
