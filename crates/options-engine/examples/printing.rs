//! Follow printing as the engine sees it: every change of the printers and
//! the prints, one block each, with the time it arrived.
//!
//! ```sh
//! cargo run -p options-engine --example printing
//! cargo run -p options-engine --example printing cancel 14   # or restart <id>,
//!     # resume|pause|options <printer>, default <printer>, print <printer> <file>,
//!     # set <printer> colour|quality|media|sides <value>
//! ```
//!
//! From a container: `CUPS_SERVER=<the host's cups.sock>` and
//! `DBUS_SYSTEM_BUS_ADDRESS=unix:path=<the host's system_bus_socket>`.

use options_engine::{cups, Engine, PrintingCollector};

#[tokio::main]
async fn main() {
    // `printing <action> <arg>`: do one thing to CUPS and say how it went.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [action, arg, rest @ ..] = args.as_slice() {
        let done = match action.as_str() {
            "cancel" => cups::cancel_job(arg.parse().expect("a job id")).map(|_| String::new()),
            "restart" => cups::restart_job(arg.parse().expect("a job id")).map(|_| String::new()),
            "resume" => cups::resume_printer(arg).map(|_| String::new()),
            "default" => cups::set_default(arg).map(|_| String::new()),
            "print" => cups::print_file(arg, std::path::Path::new(&rest[0]), "Golem")
                .map(|id| format!("job {id}")),
            "options" => cups::defaults(arg).map(|d| format!("{d:?}")),
            "set" => {
                let value = match rest[0].as_str() {
                    "colour" => cups::Default::Colour(rest[1] == "on"),
                    "quality" => cups::Default::Quality(rest[1].parse().expect("3, 4 or 5")),
                    "media" => cups::Default::Media(rest[1].clone()),
                    "sides" => cups::Default::BothSides(rest[1] == "on"),
                    other => panic!("no such default: {other}"),
                };
                cups::set_default_option(arg, &value).map(|_| String::new())
            }
            "pause" => cups::pause_printer(arg).map(|_| String::new()),
            other => panic!("no such action: {other}"),
        };
        println!("{action} {arg}: {done:?}");
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "options_engine=debug".into()),
        )
        .init();

    let engine = Engine::start_with(vec![Box::new(PrintingCollector::new())]);
    let mut rx = engine.subscribe();
    let start = std::time::Instant::now();
    let mut last = None;
    loop {
        if rx.changed().await.is_err() {
            break;
        }
        let ctx = rx.borrow().clone();
        if last.as_ref() == Some(&ctx.printing) && ctx.health.printing.alive {
            continue;
        }
        last = Some(ctx.printing.clone());
        println!(
            "── {:>6.1}s  alive={}",
            start.elapsed().as_secs_f32(),
            ctx.health.printing.alive
        );
        for p in &ctx.printing.printers {
            let ink: Vec<String> = p
                .markers
                .iter()
                .map(|m| {
                    format!(
                        "{} {}",
                        m.name,
                        m.level.map(|l| format!("{l}%")).unwrap_or("?".into())
                    )
                })
                .collect();
            println!(
                "  printer {}{} \"{}\" {:?} accepting={} problem={:?} reasons={:?} msg={:?} ink={:?}",
                p.name,
                if p.default { " (default)" } else { "" },
                p.info,
                p.state,
                p.accepting,
                p.problem,
                p.reasons,
                p.message,
                ink
            );
        }
        for j in &ctx.printing.jobs {
            println!(
                "  job {} on {} \"{}\" {:?} sheets={} pages={:?} reasons={:?} msg={:?}",
                j.id, j.printer, j.title, j.state, j.sheets_done, j.pages, j.reasons, j.message
            );
        }
    }
}
