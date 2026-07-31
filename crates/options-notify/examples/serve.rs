//! Run the notification server and print the live list as it changes.
//!
//! On the real session bus (mako must be stopped):
//! ```sh
//! cargo run -p options-notify --example serve
//! ```

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("options_notify=debug").init();
    let svc = match options_notify::NotificationService::start().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not start notification server: {e}");
            eprintln!("(is another daemon — mako — holding org.freedesktop.Notifications?)");
            return;
        }
    };
    let mut rx = svc.subscribe();
    eprintln!("options-notify serving; waiting for notifications…");
    loop {
        if rx.changed().await.is_err() {
            break;
        }
        let list = rx.borrow().clone();
        println!("== {} active ==", list.len());
        for n in &list {
            println!(
                "  #{} [{}] {} — {} | urgency={:?} actions={} inline={} img={}",
                n.id,
                n.app_name,
                n.summary,
                n.body,
                n.urgency,
                n.actions.len(),
                n.supports_inline_reply,
                n.image_dims.map_or("no".into(), |(w, h)| format!("{w}x{h}")),
            );
        }
    }
}
