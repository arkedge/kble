pub const NOTICE: &'static str = include_str!(concat!(env!("OUT_DIR"), "/LICENSE"));

pub trait ParseWithLicenseExt: clap::Parser {
    fn parse_with_license() -> Self {
        let mut command =
            Self::command_for_update().arg(clap::arg!(--"legal-notice" "Show legal notices"));
        let mut matches = command.clone().get_matches();
        if matches.get_flag("legal-notice") {
            println!("{}", NOTICE);
            std::process::exit(0);
        }
        Self::from_arg_matches_mut(&mut matches)
            .map_err(|err| err.format(&mut command))
            .unwrap_or_else(|err| err.exit())
    }
}

impl<T: clap::Parser> ParseWithLicenseExt for T {}
