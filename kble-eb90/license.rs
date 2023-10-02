pub const NOTICE: &'static str = include_str!(concat!(env!("OUT_DIR"), "/LICENSE"));

pub trait ParseWithLicenseExt: clap::Parser {
    fn parse_with_license() -> Self {
        let command =
            Self::command_for_update().arg(clap::arg!(--"legal-notice" "Show legal notices"));
        let matches = command.get_matches();
        if matches.get_flag("legal-notice") {
            println!("{}", NOTICE);
            std::process::exit(0);
        }
        Self::parse()
    }
}

impl<T: clap::Parser> ParseWithLicenseExt for T {}
