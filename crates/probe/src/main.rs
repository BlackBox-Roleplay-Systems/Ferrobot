//! ferro-probe: Ferrobot's test platform. It logs a test account in to real regions (a local
//! OpenSim-NGC grid: AGENTS.md rule 1) and runs scenarios of teleports and crossings, each timed
//! and judged by where the avatar ends up, as the Tranquillity teleport rig did with a
//! LibreMetaverse client. Faults (a lost CrossedRegion) are put in on purpose (ferro-client's
//! `faults` feature). One JSON line per step on stdout, then a table.
//!
//! Exit codes: 0 every step went right, 1 one or more did not, 2 wrong options.

mod judge;
mod options;
mod report;
mod scenarios;

use options::Options;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    // the client's own log goes to stderr, quiet unless asked (RUST_LOG)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let opts = match Options::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprint!("{}", options::USAGE);
            return ExitCode::from(2);
        }
    };
    let password = match std::fs::read_to_string(&opts.password_file) {
        Ok(p) => p.trim_end_matches(['\r', '\n']).to_owned(),
        Err(e) => {
            eprintln!("--password-file: {e}");
            return ExitCode::from(2);
        }
    };
    let json = match &opts.json {
        Some(path) => match std::fs::File::create(path) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("--json: {e}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    let mut probe = scenarios::Probe::new(&opts, password, json);
    for round in 1..=opts.rounds {
        probe.round(round).await;
        if round < opts.rounds {
            // the grid forgets the last session before the next login
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
    println!();
    print!("{}", report::summary(&probe.records));
    if probe.records.iter().all(|r| r.ok) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
