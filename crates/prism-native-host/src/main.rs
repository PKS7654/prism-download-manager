//! `prism-native-host` — started by browsers through native messaging, or by
//! the installer/app with `register` / `unregister` / `status`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use prism_native_host::args::{self, Invocation};
use prism_native_host::registration::{self, Locations, RegisterOptions};
use prism_native_host::{CHROME_EXTENSION_ID, FIREFOX_EXTENSION_ID, host};
use std::path::PathBuf;
use std::process::ExitCode;

fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(prism_native_host::HOST_BINARY_NAME))
}

fn print_json<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let inv = match args::classify(&argv) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    match inv {
        Invocation::Host { caller } => ExitCode::from(host::run(caller) as u8),
        Invocation::Help => {
            println!("{}", args::USAGE);
            ExitCode::SUCCESS
        }
        Invocation::Version => {
            println!("prism-native-host {}", prism_native_host::VERSION);
            ExitCode::SUCCESS
        }
        Invocation::Register(r) => {
            let mut opts = RegisterOptions::new(r.host_path.unwrap_or_else(current_exe));
            opts.browsers = r.browsers;
            if !r.chrome_extension_ids.is_empty() {
                opts.chrome_extension_ids = r.chrome_extension_ids;
            }
            if !r.firefox_extension_ids.is_empty() {
                opts.firefox_extension_ids = r.firefox_extension_ids;
            }
            debug_assert!(!CHROME_EXTENSION_ID.is_empty() && !FIREFOX_EXTENSION_ID.is_empty());
            opts.locations = Locations {
                manifest_dir: r.manifest_dir,
                ..Default::default()
            };
            match registration::register(&opts) {
                Ok(st) => {
                    print_json(&st);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("register failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Invocation::Unregister {
            browsers,
            manifest_dir,
        } => {
            let loc = Locations {
                manifest_dir,
                ..Default::default()
            };
            match registration::unregister(&browsers, &loc) {
                Ok(st) => {
                    print_json(&st);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("unregister failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Invocation::Status {
            browsers,
            manifest_dir,
            host_path,
        } => {
            let loc = Locations {
                manifest_dir,
                ..Default::default()
            };
            print_json(&registration::status(
                &browsers,
                &host_path.unwrap_or_else(current_exe),
                &loc,
            ));
            ExitCode::SUCCESS
        }
    }
}
