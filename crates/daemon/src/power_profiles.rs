//! The computer's power modes, asked of `power-profiles-daemon` on the system bus.
//!
//! Its `powerprofilesctl` is a Python program, and the power mode asked it two or
//! three things at every start: 1258 file opens and 5 s of a spinning disk, just as
//! the dock's GPU was opening (ASUS X550LC, 2026-10-31). Off the loop, like every
//! question that may wait: a machine without the daemon answers `None` at once, a
//! daemon that hangs after `PATIENCE`.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy};

const SERVICE: &str = "org.freedesktop.UPower.PowerProfiles";
const OBJECT: &str = "/org/freedesktop/UPower/PowerProfiles";
/// What any program the settings ask is given.
const PATIENCE: Duration = Duration::from_secs(4);

fn answer<T>(question: impl Future<Output = zbus::Result<T>>) -> Option<T> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    runtime.block_on(async { tokio::time::timeout(PATIENCE, question).await.ok()?.ok() })
}

/// The modes this machine has, slowest first; `None` without the daemon.
pub(crate) fn names() -> Option<Vec<String>> {
    answer(async { names_on(&Connection::system().await?).await })
}

/// The mode in use.
pub(crate) fn active() -> Option<String> {
    answer(async { active_on(&Connection::system().await?).await })
}

/// Ask for a mode; whether the daemon took it.
pub(crate) fn set(mode: &str) -> bool {
    answer(async { set_on(&Connection::system().await?, mode).await }).is_some()
}

async fn daemon(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    zbus::proxy::Builder::new(conn)
        .destination(SERVICE)?
        .path(OBJECT)?
        .interface(SERVICE)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

async fn names_on(conn: &Connection) -> zbus::Result<Vec<String>> {
    let all: Vec<HashMap<String, OwnedValue>> = daemon(conn).await?.get_property("Profiles").await?;
    let named = all
        .iter()
        .filter_map(|profile| String::try_from(profile.get("Profile")?.try_clone().ok()?).ok());
    Ok(slowest_first(named.collect()))
}

async fn active_on(conn: &Connection) -> zbus::Result<String> {
    daemon(conn).await?.get_property("ActiveProfile").await
}

async fn set_on(conn: &Connection, mode: &str) -> zbus::Result<()> {
    daemon(conn).await?.set_property("ActiveProfile", mode).await.map_err(zbus::Error::from)
}

fn slowest_first(mut names: Vec<String>) -> Vec<String> {
    let rank = |n: &str| match n {
        "power-saver" => 0,
        "balanced" => 1,
        _ => 2,
    };
    names.sort_by_key(|n| rank(n));
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modes_come_slowest_first() {
        let names = ["performance", "power-saver", "balanced"].map(String::from).to_vec();
        assert_eq!(slowest_first(names), ["power-saver", "balanced", "performance"]);
    }

    /// The machine's real daemon, read only (from the toolbox:
    /// `DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/host/run/dbus/system_bus_socket`).
    #[test]
    #[ignore]
    fn the_real_daemon_answers() {
        let names = names().expect("the daemon's modes");
        let now = active().expect("the mode in use");
        eprintln!("{names:?}, now {now}");
        assert!(names.contains(&now));
        assert!(names.contains(&"balanced".to_owned()));
    }

    /// No daemon of that name on a bus: an answer, not a hang.
    #[test]
    #[ignore]
    fn no_daemon_is_no_answer() {
        let missing = answer(async {
            let conn = Connection::system().await?;
            let nobody: Proxy<'_> = zbus::proxy::Builder::new(&conn)
                .destination("org.freedesktop.UPower.PowerProfiles.Nobody")?
                .path(OBJECT)?
                .interface(SERVICE)?
                .cache_properties(CacheProperties::No)
                .build()
                .await?;
            nobody.get_property::<String>("ActiveProfile").await
        });
        assert_eq!(missing, None);
    }
}
