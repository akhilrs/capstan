//! Reads files named on the command line and prints `accept` or `reject` for each, as the loader's TOML step sees them
//! (BOM and CRLF handled as `parse_config` does). A probe for the differential run's minimiser.
fn main() {
    for file in std::env::args().skip(1) {
        let bytes = std::fs::read(&file).unwrap();
        let verdict =
            match capstan_config::parse_config(&bytes, std::path::Path::new("/nonexistent")) {
                Err(capstan_config::ConfigError::Parse(_)) => "reject",
                _ => "accept",
            };
        println!("{verdict}");
    }
}
