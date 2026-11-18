//! The probe's command line. The password is read from a file, never from the command line, so
//! that it stays out of the shell's history and the process list.

use std::path::PathBuf;

pub const USAGE: &str = "\
ferro-probe: teleports and crossings against real regions, timed and judged

  ferro-probe --user \"First Last\" --password-file FILE [options]

  --login-uri URI       the grid's login address (http://127.0.0.1:9000/)
  --user NAME           the test account: \"First Last\" or a username
  --password-file FILE  a file holding its password
  --scenarios LIST      comma-separated: all (local, near, far, cross), local, near, far, cross,
                        bounce, lost-crossing, unreachable, restart, sit-cross (all)
  --rounds N            rounds, each a fresh login (1)
  --bots N              bots at once, each running the scenarios (1): the first is --user, the
                        others the same name with 2, 3… after the last name, the same password
  --stagger MS          milliseconds between the bots' logins (250); 0 logs them in at once
  --remote-admin-password-file FILE
                        the grid's RemoteAdmin password (OpenSim, at the login address): creates
                        the other bots' accounts, restarts regions (the restart scenario)
  --home REGION         where the bot starts and comes back to (Alpha)
  --near REGION         the home region's neighbour to the east (Beta)
  --far REGION          a region far from home (Gamma)
  --unreachable REGION  a region whose address the bot cannot reach (Delta)
  --no-neighbours       keep no circuits to neighbour regions (a viewer keeps them)
  --json FILE           also write the lines to this file
";

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub login_uri: String,
    pub user: String,
    pub password_file: PathBuf,
    pub scenarios: Vec<Scenario>,
    pub rounds: u32,
    pub home: String,
    pub near: String,
    pub far: String,
    pub unreachable: String,
    pub neighbours: bool,
    pub json: Option<PathBuf>,
    pub bots: u32,
    pub stagger: u64,
    pub admin_password_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Teleports within the home region.
    Local,
    /// To the neighbour and back.
    Near,
    /// To a far region and back.
    Far,
    /// Flying over the border to the neighbour and back.
    Cross,
    /// Local teleports while standing, walking, flying and falling: does the avatar snap back?
    Bounce,
    /// A crossing whose CrossedRegion the bot drops, then what still works.
    LostCrossing,
    /// A teleport to a region the bot cannot reach, then what still works.
    Unreachable,
    /// The neighbour region restarts, then the home region with the bot in it (RemoteAdmin).
    Restart,
    /// Sitting on a box moved over the border and back, as on a vehicle.
    SitCross,
}

impl Scenario {
    fn parse(s: &str) -> Result<Vec<Scenario>, String> {
        Ok(match s {
            "all" => vec![
                Scenario::Local,
                Scenario::Near,
                Scenario::Far,
                Scenario::Cross,
            ],
            "local" => vec![Scenario::Local],
            "near" => vec![Scenario::Near],
            "far" => vec![Scenario::Far],
            "cross" => vec![Scenario::Cross],
            "bounce" => vec![Scenario::Bounce],
            "lost-crossing" => vec![Scenario::LostCrossing],
            "unreachable" => vec![Scenario::Unreachable],
            "restart" => vec![Scenario::Restart],
            "sit-cross" => vec![Scenario::SitCross],
            _ => return Err(format!("unknown scenario: {s}")),
        })
    }
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut o = Options {
            login_uri: "http://127.0.0.1:9000/".into(),
            user: String::new(),
            password_file: PathBuf::new(),
            scenarios: Scenario::parse("all")?,
            rounds: 1,
            home: "Alpha".into(),
            near: "Beta".into(),
            far: "Gamma".into(),
            unreachable: "Delta".into(),
            neighbours: true,
            json: None,
            bots: 1,
            stagger: 250,
            admin_password_file: None,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--login-uri" => o.login_uri = value()?,
                "--user" => o.user = value()?,
                "--password-file" => o.password_file = value()?.into(),
                "--scenarios" => {
                    let list = value()?;
                    o.scenarios = Vec::new();
                    for s in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        for sc in Scenario::parse(s)? {
                            if !o.scenarios.contains(&sc) {
                                o.scenarios.push(sc);
                            }
                        }
                    }
                }
                "--rounds" => {
                    o.rounds = value()?
                        .parse()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("--rounds: a number, at least 1")?
                }
                "--home" => o.home = value()?,
                "--near" => o.near = value()?,
                "--far" => o.far = value()?,
                "--unreachable" => o.unreachable = value()?,
                "--bots" => {
                    o.bots = value()?
                        .parse()
                        .ok()
                        .filter(|n| (1..=100).contains(n))
                        .ok_or("--bots: a number from 1 to 100")?
                }
                "--stagger" => {
                    o.stagger = value()?
                        .parse()
                        .map_err(|_| "--stagger: a number of milliseconds")?
                }
                "--remote-admin-password-file" => o.admin_password_file = Some(value()?.into()),
                "--no-neighbours" => o.neighbours = false,
                "--json" => o.json = Some(value()?.into()),
                "-h" | "--help" => return Err(String::new()),
                _ => return Err(format!("unknown option: {arg}")),
            }
        }
        if o.user.is_empty() {
            return Err("--user is required".into());
        }
        if o.password_file.as_os_str().is_empty() {
            return Err("--password-file is required".into());
        }
        if o.scenarios.is_empty() {
            return Err("--scenarios: none given".into());
        }
        if o.scenarios.contains(&Scenario::Restart) && o.admin_password_file.is_none() {
            return Err("--scenarios restart needs --remote-admin-password-file".into());
        }
        if o.bots > 1 && o.scenarios.contains(&Scenario::Restart) {
            return Err("--scenarios restart runs with one bot".into());
        }
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn defaults_are_the_rigs() {
        let o = parse(&["--user", "Test User", "--password-file", "pw"]).expect("options");
        assert_eq!(o.user, "Test User");
        assert_eq!(
            o.scenarios,
            vec![
                Scenario::Local,
                Scenario::Near,
                Scenario::Far,
                Scenario::Cross
            ]
        );
        assert_eq!((o.rounds, o.neighbours), (1, true));
        assert_eq!(
            (o.home.as_str(), o.near.as_str(), o.far.as_str()),
            ("Alpha", "Beta", "Gamma")
        );
    }

    #[test]
    fn scenarios_listed_once_each_in_order() {
        let o = parse(&[
            "--user",
            "u",
            "--password-file",
            "pw",
            "--scenarios",
            "bounce, local,all",
            "--rounds",
            "3",
            "--no-neighbours",
        ])
        .expect("options");
        assert_eq!(
            o.scenarios,
            vec![
                Scenario::Bounce,
                Scenario::Local,
                Scenario::Near,
                Scenario::Far,
                Scenario::Cross
            ]
        );
        assert_eq!((o.rounds, o.neighbours), (3, false));
    }

    #[test]
    fn bots_and_restarts() {
        let o = parse(&[
            "--user",
            "u",
            "--password-file",
            "pw",
            "--bots",
            "4",
            "--remote-admin-password-file",
            "ra",
        ])
        .expect("options");
        assert_eq!(o.bots, 4);
        assert_eq!(o.admin_password_file, Some(PathBuf::from("ra")));
        let base = ["--user", "u", "--password-file", "pw"];
        let with = |more: &[&str]| parse(&[&base[..], more].concat());
        assert!(with(&["--bots", "0"]).is_err());
        assert!(with(&["--bots", "101"]).is_err());
        assert!(with(&["--scenarios", "restart"]).is_err(), "no RemoteAdmin");
        assert!(
            with(&[
                "--scenarios",
                "restart",
                "--remote-admin-password-file",
                "ra",
                "--bots",
                "2"
            ])
            .is_err(),
            "restarts with one bot"
        );
        assert!(
            with(&[
                "--scenarios",
                "restart",
                "--remote-admin-password-file",
                "ra"
            ])
            .is_ok()
        );
    }

    #[test]
    fn wrong_options_are_refused() {
        assert!(parse(&["--password-file", "pw"]).is_err());
        assert!(parse(&["--user", "u"]).is_err());
        assert!(parse(&["--user", "u", "--password-file", "pw", "--rounds", "0"]).is_err());
        assert!(parse(&["--user", "u", "--password-file", "pw", "--scenarios", "fly"]).is_err());
        assert!(parse(&["--user", "u", "--password-file", "pw", "--wat"]).is_err());
        assert!(parse(&["--user"]).is_err());
    }
}
