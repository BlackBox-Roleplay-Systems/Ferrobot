//! ferro-probe: Ferrobot's test platform. It logs test accounts in to real regions (a local
//! OpenSim-NGC grid: AGENTS.md rule 1) and runs scenarios of teleports and crossings, each timed
//! and judged by where the avatar ends up, as the Tranquillity teleport rig did with a
//! LibreMetaverse client. Faults (a lost CrossedRegion) are put in on purpose (ferro-client's
//! `faults` feature); regions are restarted through OpenSim's RemoteAdmin. One JSON line per step
//! on stdout, then a table.
//!
//! Exit codes: 0 every step went right, 1 one or more did not, 2 wrong options.

mod admin;
mod judge;
mod options;
mod report;
mod scenarios;

use options::Options;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> ExitCode {
    // the client's own log goes to stderr, quiet unless asked (RUST_LOG)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let opts = match Options::parse(std::env::args().skip(1)) {
        Ok(o) => Arc::new(o),
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprint!("{}", options::USAGE);
            return ExitCode::from(2);
        }
    };
    let Some(password) = secret(&opts.password_file, "--password-file") else {
        return ExitCode::from(2);
    };
    let http = reqwest::Client::builder()
        .redirect(ferro_client::login::redirect_policy())
        .build()
        .unwrap_or_default();
    let admin = match &opts.admin_password_file {
        Some(file) => match secret(file, "--remote-admin-password-file") {
            Some(pw) => Some(Arc::new(admin::Admin::new(
                http.clone(),
                opts.login_uri.clone(),
                pw,
            ))),
            None => return ExitCode::from(2),
        },
        None => None,
    };
    let json = match &opts.json {
        Some(path) => match std::fs::File::create(path) {
            Ok(f) => Some(Arc::new(Mutex::new(f))),
            Err(e) => {
                eprintln!("--json: {e}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    let users = match accounts(&opts, &password, admin.as_deref()).await {
        Ok(users) => users,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let mut bots = tokio::task::JoinSet::new();
    for (k, user) in users.into_iter().enumerate() {
        let mut probe = scenarios::Probe::new(
            opts.clone(),
            user,
            password.clone(),
            http.clone(),
            admin.clone(),
            json.clone(),
        );
        let rounds = opts.rounds;
        let start = std::time::Duration::from_millis(opts.stagger * k as u64);
        bots.spawn(async move {
            tokio::time::sleep(start).await;
            for round in 1..=rounds {
                probe.round(round).await;
                if round < rounds {
                    // the grid forgets the last session before the next login
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
            probe.records
        });
    }
    let mut records = Vec::new();
    while let Some(done) = bots.join_next().await {
        match done {
            Ok(r) => records.extend(r),
            Err(e) => {
                eprintln!("a bot's task: {e}");
                return ExitCode::from(1);
            }
        }
    }
    println!();
    print!("{}", report::summary(&records));
    if records.iter().all(|r| r.ok) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// A password from its file, without the line end.
fn secret(path: &std::path::Path, option: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(p) => Some(p.trim_end_matches(['\r', '\n']).to_owned()),
        Err(e) => {
            eprintln!("{option}: {e}");
            None
        }
    }
}

/// The bots' accounts: `--user`, then the same name with 2, 3… after the last name, created
/// through RemoteAdmin when they are missing.
async fn accounts(
    opts: &Options,
    password: &str,
    admin: Option<&admin::Admin>,
) -> Result<Vec<String>, String> {
    let mut users = vec![opts.user.clone()];
    if opts.bots == 1 {
        return Ok(users);
    }
    let (first, last) = opts
        .user
        .split_once(' ')
        .ok_or("--bots: --user as \"First Last\"")?;
    let Some(admin) = admin else {
        return Err("--bots needs --remote-admin-password-file, to create the accounts".into());
    };
    for k in 2..=opts.bots {
        let last = format!("{last}{k}");
        if !admin.exists_user(first, &last).await? {
            admin.create_user(first, &last, password).await?;
            eprintln!("account {first} {last} created");
        }
        users.push(format!("{first} {last}"));
    }
    Ok(users)
}
