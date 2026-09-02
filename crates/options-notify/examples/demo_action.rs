//! Post a notification (as a client) then invoke its action, to verify the
//! outbound ActionInvoked + NotificationClosed path. Run under a private bus:
//!   dbus-run-session -- cargo run -p options-notify --example demo_action

use std::collections::HashMap;
use std::time::Duration;

use zbus::zvariant::Value;

#[tokio::main]
async fn main() -> zbus::Result<()> {
    let svc = options_notify::NotificationService::start().await?;

    // A second connection stands in for a client app posting a notification.
    let client = zbus::Connection::session().await?;
    let reply = client
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "demo",
                0u32,
                "",
                "Summary",
                "Body",
                Vec::<String>::new(),
                HashMap::<String, Value>::new(),
                0i32,
            ),
        )
        .await?;
    let id: u32 = reply.body().deserialize()?;
    println!("posted notification id={id}");

    tokio::time::sleep(Duration::from_millis(300)).await;
    svc.invoke_action(id, "default").await?;
    println!(
        "invoke_action(id={id}, \"default\") done — ActionInvoked + NotificationClosed emitted"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}
