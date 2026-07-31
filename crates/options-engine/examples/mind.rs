//! Watch OPTIONS *decide*: print the ranked option set as context changes.
//!
//! ```sh
//! cargo run -p options-engine --example mind
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
    let mind = options_engine::Mind::new(&engine, options_engine::Tuning::default());
    let mut rx = mind.subscribe();
    println!("watching OPTIONS decisions (Ctrl-C to stop)…\n");
    let mut last = None;
    loop {
        if rx.changed().await.is_err() {
            break;
        }
        // Only print when the decision actually changes (ignore the generation).
        let opts = rx.borrow().clone();
        let key = (opts.activity, opts.items.clone());
        if last.as_ref() == Some(&key) {
            continue;
        }
        last = Some(key);
        println!("── {:?} ──", opts.activity);
        for a in &opts.items {
            println!(
                "  [{:.2}] {:?}  {} — {}   ({})",
                a.relevance, a.kind, a.title, a.detail, a.reason
            );
        }
    }
}
