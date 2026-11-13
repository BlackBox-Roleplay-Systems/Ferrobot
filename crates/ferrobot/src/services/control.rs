//! The services that can be stopped, started and restarted while the bot runs, and have their
//! settings changed: SecondBot's service control (Commands/Services.cs, and BotServices in
//! Services/Services.cs: enableAndStart, Stop, Restart, changeConfig).
//!
//! A service joins by being registered with a [`Spec`]: its name (SecondBot's, when SecondBot has
//! the service), the prefix and names of the settings it reads, how it reads them and how it
//! starts. Starting reads its settings as they are then (the environment, under the values
//! ChangeConfigValue changed) and spawns its tasks; stopping aborts them and waits until they are
//! gone. A changed setting takes effect at the service's next start or restart.
//!
//! What the operator reaches the bot through is not here, so that no command can lock them out:
//! the login and the session (SecondBot's BotClientService), the HTTP API (HttpService), the
//! commands and the commands by IM (CommandsService), the chat history (DataStoreService), the
//! health file. SecondBot lets BotClientService and HttpService be stopped; Ferrobot answers
//! "Unable to manage selected service" (one of SecondBot's own replies) for those (deliberate).

use crate::config::{ConfigError, Source};
use tokio::task::JoinHandle;

/// SecondBot's controllable services that are Ferrobot's core: never stopped nor changed.
const PROTECTED: &[&str] = &["BotClientService", "HttpService"];

/// Why a service command was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// FERRO_ALLOW_SERVICE_CONTROL is off.
    NotAllowed,
    /// No such service (or one Ferrobot does not have yet).
    Unknown,
    /// One of the bot's core services.
    Protected,
    /// A setting that holds a secret: never changed by command.
    Secret,
    /// A setting the service does not read.
    UnknownSetting,
    /// A value the service could not read.
    Invalid,
}

type Read = Box<dyn Fn(&Source) -> Result<(), ConfigError> + Send + Sync>;
type Start = Box<dyn Fn(&Source) -> Result<Vec<JoinHandle<()>>, ConfigError> + Send + Sync>;

/// A service as the control knows it.
pub struct Spec {
    name: &'static str,
    prefix: &'static str,
    settings: &'static [&'static str],
    check: Read,
    start: Start,
}

impl Spec {
    /// `name`: SecondBot's name for the service. `prefix` and `settings`: SecondBot's name for its
    /// config file and the settings in it that ChangeConfigValue may change (`relay` and `count`
    /// for `relay_count`; `<n>_` stands for a number, as in `relay_1_enabled`). `read`: its
    /// settings from a source, an error when one is wrong. `run`: spawns its tasks with them,
    /// none when they leave the service off.
    pub fn new<T: 'static>(
        name: &'static str,
        prefix: &'static str,
        settings: &'static [&'static str],
        read: fn(&Source) -> Result<T, ConfigError>,
        run: impl Fn(T) -> Vec<JoinHandle<()>> + Send + Sync + 'static,
    ) -> Spec {
        Spec {
            name,
            prefix,
            settings,
            check: Box::new(move |s| read(s).map(drop)),
            start: Box::new(move |s| read(s).map(&run)),
        }
    }
}

/// Settings that hold secrets (a password, a token, a key to an API): refused by
/// ChangeConfigValue, so that they never travel in a command, its answer or the command history.
fn is_secret(setting: &str) -> bool {
    let s = setting.to_ascii_lowercase();
    [
        "secret",
        "password",
        "passwd",
        "token",
        "apikey",
        "api_key",
        "credential",
    ]
    .iter()
    .any(|w| s.contains(w))
}

/// The setting as the service spells it, whatever the case asked; `None` when it reads no such
/// setting.
fn setting_name(settings: &[&str], asked: &str) -> Option<String> {
    let asked = asked.trim();
    settings.iter().find_map(|s| match s.strip_prefix("<n>_") {
        Some(rest) => {
            let (n, r) = asked.split_once('_')?;
            let n: u32 = n
                .parse()
                .ok()
                .filter(|_| n.bytes().all(|b| b.is_ascii_digit()))?;
            r.eq_ignore_ascii_case(rest).then(|| format!("{n}_{rest}"))
        }
        None => s.eq_ignore_ascii_case(asked).then(|| (*s).to_owned()),
    })
}

struct Entry {
    spec: Spec,
    tasks: Vec<JoinHandle<()>>,
}

impl Entry {
    fn running(&self) -> bool {
        self.tasks.iter().any(|t| !t.is_finished())
    }

    fn start(&mut self, source: &Source) {
        let name = self.spec.name;
        match (self.spec.start)(source) {
            Ok(tasks) if tasks.is_empty() => log::info!("{name}: off by its settings"),
            Ok(tasks) => {
                log::info!("{name}: started");
                self.tasks = tasks;
            }
            Err(e) => log::warn!("{name}: not started: {e}"),
        }
    }

    /// Aborts its tasks and waits until they are dropped, so that what they hold is let go of
    /// before the service starts again.
    async fn stop(&mut self) {
        let was_running = self.running();
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks.drain(..) {
            let _ = t.await;
        }
        if was_running {
            log::info!("{}: stopped", self.spec.name);
        }
    }
}

#[derive(Default)]
struct Inner {
    source: Source,
    entries: Vec<Entry>,
}

/// The services, by name. One command at a time: a start never races a stop.
pub struct Services {
    allowed: bool,
    inner: tokio::sync::Mutex<Inner>,
}

impl Services {
    /// `allowed`: the commands are on (FERRO_ALLOW_SERVICE_CONTROL, SecondBot's
    /// AllowServiceControl); the services run either way.
    pub fn new(allowed: bool) -> Self {
        Services {
            allowed,
            inner: tokio::sync::Mutex::default(),
        }
    }

    /// Adds a service, not started yet.
    pub async fn register(&self, spec: Spec) {
        self.inner.lock().await.entries.push(Entry {
            spec,
            tasks: Vec::new(),
        });
    }

    /// Starts every service, at the bot's start: before the login, so that each one's
    /// subscription to the client's events sees the first ones.
    pub async fn start_all(&self) {
        let mut inner = self.inner.lock().await;
        let Inner { source, entries } = &mut *inner;
        for e in entries {
            e.start(source);
        }
    }

    /// The service's place, after the checks SecondBot makes, in its order.
    fn find(&self, inner: &Inner, name: &str) -> Result<usize, Refused> {
        if !self.allowed {
            return Err(Refused::NotAllowed);
        }
        let name = name.trim();
        if PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            return Err(Refused::Protected);
        }
        inner
            .entries
            .iter()
            .position(|e| e.spec.name.eq_ignore_ascii_case(name))
            .ok_or(Refused::Unknown)
    }

    /// EnableAndStartService: its Enabled setting (when it has one) set to true, then restarted.
    pub async fn enable_and_start(&self, name: &str) -> Result<(), Refused> {
        let mut inner = self.inner.lock().await;
        let i = self.find(&inner, name)?;
        let Inner { source, entries } = &mut *inner;
        let e = &mut entries[i];
        if e.spec.settings.contains(&"Enabled") {
            source.change(format!("{}_Enabled", e.spec.prefix), "true".into());
        }
        e.stop().await;
        e.start(source);
        Ok(())
    }

    /// StopService.
    pub async fn stop(&self, name: &str) -> Result<(), Refused> {
        let mut inner = self.inner.lock().await;
        let i = self.find(&inner, name)?;
        inner.entries[i].stop().await;
        Ok(())
    }

    /// RestartService: stopped when it runs, then started (SecondBot's Restart starts it either
    /// way).
    pub async fn restart(&self, name: &str) -> Result<(), Refused> {
        let mut inner = self.inner.lock().await;
        let i = self.find(&inner, name)?;
        let Inner { source, entries } = &mut *inner;
        entries[i].stop().await;
        entries[i].start(source);
        Ok(())
    }

    /// ChangeConfigValue: the setting kept for the service's next start. The value is never
    /// logged.
    pub async fn change(&self, name: &str, setting: &str, value: &str) -> Result<(), Refused> {
        let mut inner = self.inner.lock().await;
        let i = self.find(&inner, name)?;
        if is_secret(setting) {
            log::warn!("ChangeConfigValue: {setting} refused: a secret");
            return Err(Refused::Secret);
        }
        let spec = &inner.entries[i].spec;
        let setting = setting_name(spec.settings, setting).ok_or(Refused::UnknownSetting)?;
        let full = format!("{}_{setting}", spec.prefix);
        let mut changed = inner.source.clone();
        changed.change(full.clone(), value.to_owned());
        // a setting missing is fine (another may follow); a wrong value is not
        if let Err(ConfigError::Invalid(..)) = (spec.check)(&changed) {
            log::warn!("ChangeConfigValue: {full} refused: not a value it reads");
            return Err(Refused::Invalid);
        }
        log::info!("{}: {full} changed, for its next start", spec.name);
        inner.source = changed;
        Ok(())
    }

    /// Whether the service runs; `None` when there is no such service.
    #[cfg(test)]
    pub async fn running(&self, name: &str) -> Option<bool> {
        let inner = self.inner.lock().await;
        inner
            .entries
            .iter()
            .find(|e| e.spec.name == name)
            .map(Entry::running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts the tasks alive.
    struct Probe {
        alive: Arc<AtomicUsize>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.alive.fetch_sub(1, Ordering::SeqCst);
        }
    }

    async fn services(allowed: bool) -> (Services, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let services = Services::new(allowed);
        let (alive, relays) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (a, r) = (alive.clone(), relays.clone());
        services
            .register(Spec::new(
                "RelayService",
                "relay",
                &["count", "UseShortEncoder", "<n>_enabled", "<n>_source_type"],
                crate::config::relays,
                move |found| {
                    a.fetch_add(1, Ordering::SeqCst);
                    r.store(found.len(), Ordering::SeqCst);
                    let probe = Probe { alive: a.clone() };
                    vec![tokio::spawn(async move {
                        let _probe = probe;
                        std::future::pending::<()>().await;
                    })]
                },
            ))
            .await;
        (services, alive, relays)
    }

    #[tokio::test]
    async fn services_stop_start_and_restart_by_name() {
        let (s, alive, _) = services(true).await;
        s.start_all().await;
        assert_eq!(s.running("RelayService").await, Some(true));
        assert_eq!(alive.load(Ordering::SeqCst), 1);
        // the name in any case, as SecondBot spells it or not
        assert_eq!(s.stop("relayservice").await, Ok(()));
        assert_eq!(s.running("RelayService").await, Some(false));
        assert_eq!(alive.load(Ordering::SeqCst), 0, "its task dropped");
        assert_eq!(s.stop("RelayService").await, Ok(()), "stopped twice");
        assert_eq!(s.restart("RelayService").await, Ok(()));
        assert_eq!(s.running("RelayService").await, Some(true));
        assert_eq!(s.enable_and_start("RelayService").await, Ok(()));
        assert_eq!(alive.load(Ordering::SeqCst), 1, "the old task gone first");
    }

    #[tokio::test]
    async fn refusals_in_secondbots_order() {
        let (s, _, _) = services(false).await;
        assert_eq!(s.stop("RelayService").await, Err(Refused::NotAllowed));
        assert_eq!(s.stop("NoSuchService").await, Err(Refused::NotAllowed));
        let (s, _, _) = services(true).await;
        assert_eq!(s.stop("NoSuchService").await, Err(Refused::Unknown));
        // SecondBot's names that Ferrobot has no service for yet
        assert_eq!(s.restart("RLVService").await, Err(Refused::Unknown));
        for core in ["HttpService", "BotClientService", "httpservice"] {
            assert_eq!(s.stop(core).await, Err(Refused::Protected));
            assert_eq!(s.enable_and_start(core).await, Err(Refused::Protected));
            assert_eq!(s.restart(core).await, Err(Refused::Protected));
            assert_eq!(
                s.change(core, "Port", "8080").await,
                Err(Refused::Protected)
            );
        }
    }

    #[tokio::test]
    async fn settings_change_for_the_next_start() {
        let (s, _, relays) = services(true).await;
        s.start_all().await;
        assert_eq!(relays.load(Ordering::SeqCst), 0);
        for (k, v) in [
            ("COUNT", "1"),
            ("1_Enabled", "true"),
            ("1_source_type", "localchat"),
        ] {
            assert_eq!(s.change("RelayService", k, v).await, Ok(()), "{k}");
        }
        assert_eq!(relays.load(Ordering::SeqCst), 0, "not before a restart");
        s.restart("RelayService").await.expect("restarted");
        assert_eq!(relays.load(Ordering::SeqCst), 1);
        // back off: a changed value wins over the environment, "" unsets it
        s.change("RelayService", "count", "")
            .await
            .expect("changed");
        s.restart("RelayService").await.expect("restarted");
        assert_eq!(relays.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn secrets_unknown_settings_and_wrong_values_are_refused() {
        let (s, _, _) = services(true).await;
        for secret in [
            "ClientToken",
            "SharedSecret",
            "Password",
            "OpenAiApiKey",
            "api_key",
        ] {
            assert_eq!(
                s.change("RelayService", secret, "x").await,
                Err(Refused::Secret),
                "{secret}"
            );
        }
        assert_eq!(
            s.change("RelayService", "HideStatusOutput", "true").await,
            Err(Refused::UnknownSetting)
        );
        assert_eq!(
            s.change("RelayService", "x_enabled", "true").await,
            Err(Refused::UnknownSetting)
        );
        assert_eq!(
            s.change("RelayService", "count", "many").await,
            Err(Refused::Invalid)
        );
        assert_eq!(
            s.change("RelayService", "UseShortEncoder", "maybe").await,
            Err(Refused::Invalid)
        );
        assert_eq!(s.change("RelayService", "count", "2").await, Ok(()));
    }

    #[test]
    fn setting_names_as_the_service_spells_them() {
        let names = &["count", "<n>_source_type", "Enabled"];
        assert_eq!(setting_name(names, "Count").as_deref(), Some("count"));
        assert_eq!(setting_name(names, "enabled").as_deref(), Some("Enabled"));
        assert_eq!(
            setting_name(names, "02_SOURCE_TYPE").as_deref(),
            Some("2_source_type")
        );
        assert_eq!(setting_name(names, "_source_type"), None);
        assert_eq!(setting_name(names, "+1_source_type"), None);
        assert_eq!(setting_name(names, "1_target_type"), None);
    }
}
