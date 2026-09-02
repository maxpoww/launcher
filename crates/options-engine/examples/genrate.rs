//! Measure the engine's aggregate update rate: `generation` increments once
//! per collector update, so its growth over a window is the exact number of
//! snapshot publishes (and thus brain wakeups) the engine causes.
//!
//! Run: `cargo run -p options-engine --example genrate`

use options_engine::Engine;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let engine = Engine::start();
    let rx = engine.subscribe();
    // Let the collectors settle (first connects, seeds).
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let g0 = rx.borrow().generation;
    let t0 = std::time::Instant::now();
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    let g1 = rx.borrow().generation;
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "generation: {g0} -> {g1} (+{}) over {dt:.1}s = {:.1} updates/sec",
        g1 - g0,
        (g1 - g0) as f64 / dt
    );
}
