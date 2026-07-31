//! Print OPTIONS context snapshots as they change. A quick way to eyeball the
//! engine against a live session:
//!
//! ```sh
//! cargo run -p options-engine --example watch
//! ```

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "options_engine=debug".into()),
        )
        .init();

    let engine = options_engine::Engine::start();
    let mut rx = engine.subscribe();
    println!("watching OPTIONS context (Ctrl-C to stop)…\n");
    loop {
        if rx.changed().await.is_err() {
            break;
        }
        let ctx = rx.borrow();
        println!(
            "#{:>3} | win='{}' [{}] ws{} fs={} float={} | focusvel={:.2}/s | git={}@{} | cpu={:.0}% ram={:.0}% batt={:?}{} | submap='{}' cast={}",
            ctx.generation,
            ctx.window.title,
            ctx.window.class,
            ctx.window.workspace_id,
            ctx.window.is_fullscreen,
            ctx.window.is_floating,
            ctx.behavior.focus_switch_velocity,
            ctx.git.branch.as_deref().unwrap_or("-"),
            ctx.git
                .repo_root
                .as_deref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".into()),
            ctx.metrics.cpu_usage_pct,
            ctx.metrics.ram_usage_pct,
            ctx.metrics.battery_pct,
            if ctx.metrics.is_charging { "+" } else { "" },
            ctx.hypr_submap,
            ctx.is_screencasting,
        );
    }
}
