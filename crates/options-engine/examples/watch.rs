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
            "#{:>3} | compositor={} fresh={:?}ms | win='{}' [{}] ws{} fs={} float={} | submap='{}' layout='{}' cast={} | focusvel={:.2}/s",
            ctx.generation,
            ctx.health.compositor.alive,
            ctx.health.compositor.last_update_ms,
            ctx.window.title,
            ctx.window.class,
            ctx.window.workspace_id,
            ctx.window.is_fullscreen,
            ctx.window.is_floating,
            ctx.hypr_submap,
            ctx.active_layout,
            ctx.is_screencasting,
            ctx.behavior.focus_switch_velocity,
        );
    }
}
