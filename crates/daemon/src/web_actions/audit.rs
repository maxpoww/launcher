//! THE AUDIT of every installed webapp's row on the owner's REAL data
//! (Max, 2026-10-28: "check all the apps so they all respect their recents,
//! functions…"): `cargo test -- --ignored real_every_webapp --nocapture`,
//! with `XDG_CACHE_HOME` a scratch folder (the pictures it makes go there).
//! Each finding is one line, "<app>: <what>"; none is the pass.

use super::*;

fn installed() -> Vec<(String, String, String)> {
    let home = crate::filer::home();
    let slugs: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(home.join(".local/share/waverunner/managed-webapps.json")).unwrap_or_default())
            .unwrap_or_default();
    slugs
        .into_iter()
        .filter_map(|slug| {
            let text = std::fs::read_to_string(home.join(format!(".local/share/applications/webapp-{slug}.desktop"))).ok()?;
            let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?.to_owned();
            let name = text.lines().find_map(|l| l.strip_prefix("Name="))?.to_owned();
            let start = crate::webapps::exec_app_url(&exec)?.to_owned();
            Some((slug, name, start))
        })
        .collect()
}

/// What stands for one page whatever its address says besides.
fn identity(url: &str) -> String {
    if let Some(id) = video_id(url) {
        return format!("v:{id}");
    }
    if let Some(id) = list_id(url) {
        return format!("list:{id}");
    }
    // (A search is its words.)
    let q = url.split_once('?').and_then(|(_, q)| q.split(['&', '#']).find_map(|kv| kv.strip_prefix("q=")));
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    format!("{}{}", rest.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').to_lowercase(), q.map(|q| format!("?q={q}")).unwrap_or_default())
}

/// What a title carries that is not the page's: a count, the app's name at an end.
fn title_faults(title: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let t = title.trim();
    if t.is_empty() {
        out.push("empty title".into());
    }
    if t.starts_with('(') && t.find(") ").is_some_and(|e| t[1..e].chars().all(|c| c.is_ascii_digit() || c == '+')) {
        out.push("a count in front".into());
    }
    let (lt, ln) = (t.to_lowercase(), name.to_lowercase());
    for sep in [" | ", " - ", " · ", " – ", " — ", ": ", " / ", " • "] {
        if lt.ends_with(&format!("{sep}{ln}")) || lt.starts_with(&format!("{ln}{sep}")) {
            out.push(format!("the app's name at an end ({sep:?})"));
        }
    }
    if lt == ln {
        out.push("only the app's name".into());
    }
    if t.contains('\u{FFFD}') {
        out.push("a broken letter".into());
    }
    if t.chars().any(|c| c.is_control()) {
        out.push("a control character".into());
    }
    out
}

#[test]
#[ignore]
fn real_every_webapp() {
    let apps = installed();
    assert!(!apps.is_empty(), "no installed webapps found");
    let mut faults: Vec<String> = Vec::new();
    for (slug, name, start) in &apps {
        let host = origin(start).map(|(_, h)| h);
        let site = site(slug).or_else(|| site_of_host(host.as_deref().unwrap_or("")));
        let hosts = webapp_hosts(site, start);
        let at = Instant::now();
        let got = webapp_recents(site, start, name, &Wanted { max: 24, ..Default::default() });
        println!("\n== {name} ({slug}) {start} — site {:?}: {} recents in {:?}", site.map(|s| s.slug), got.len(), at.elapsed());
        let mut f = |what: String| faults.push(format!("{name}: {what}"));
        let (mut urls, mut ids, mut titles) = (HashSet::new(), HashMap::new(), HashMap::new());
        for r in &got {
            println!(
                "   {:58} {:7} {} {}",
                r.title.chars().take(58).collect::<String>(),
                r.aspect.map(|a| format!("{a:.2}")).unwrap_or_else(|| "tile".into()),
                if r.played { "♪" } else { " " },
                r.url.chars().take(110).collect::<String>()
            );
            if !urls.insert(r.url.clone()) {
                f(format!("the same address twice: {}", r.url));
            }
            if let Some(other) = ids.insert(identity(&r.url), r.url.clone()) {
                f(format!("one page twice: {} / {}", other, r.url));
            }
            if let Some(other) = titles.insert(r.title.to_lowercase(), r.url.clone()) {
                f(format!("one title twice \"{}\": {} / {}", r.title, other, r.url));
            }
            for fault in title_faults(&r.title, name).into_iter().chain(site.map(|s| title_faults(&r.title, s.name)).unwrap_or_default()) {
                f(format!("\"{}\": {fault}", r.title));
            }
            let searched = site.and_then(|s| s.search).and_then(origin).is_some_and(|(_, h)| hosts.iter().any(|p| host_is(&h, &[p.as_str()])));
            if !(on_hosts(&r.url, &hosts) || r.played && searched) {
                f(format!("not on its own hosts {hosts:?}: {}", r.url));
            }
            if passing_path(&r.url) {
                f(format!("a sign-in or paying page: {}", r.url));
            }
            match &r.picture {
                None => f(format!("no picture: \"{}\"", r.title)),
                Some(p) if !p.is_file() => f(format!("its picture is gone: {}", p.display())),
                Some(p) if image::image_dimensions(p).is_err() => f(format!("its picture does not read: {}", p.display())),
                Some(_) => {}
            }
            if r.aspect.is_some() {
                match &r.big {
                    None => f(format!("a preview without a big one for Quick Look: \"{}\"", r.title)),
                    Some((p, _)) if !p.is_file() => f(format!("its Quick Look picture is gone: {}", p.display())),
                    Some(_) => {}
                }
            }
            if r.last <= 0.0 {
                f(format!("no time seen: \"{}\"", r.title));
            }
        }
        // Pictures two pages wear (a site's tile, fine; a preview twice: not its own).
        let mut worn: HashMap<&Path, Vec<&str>> = HashMap::new();
        for r in got.iter().filter(|r| r.aspect.is_some()) {
            if let Some(p) = &r.picture {
                worn.entry(p.as_path()).or_default().push(&r.title);
            }
        }
        for (p, who) in worn.iter().filter(|(_, w)| w.len() > 1) {
            f(format!("one preview on {} pages {:?}: {}", who.len(), who, p.display()));
        }
        // A search in it finds what it shows.
        if let Some(first) = got.first() {
            if let Some(word) = first.title.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric())).find(|w| w.chars().count() >= 4) {
                let words = words_of(word);
                let found = webapp_recents(site, start, name, &Wanted { max: 24, words, ..Default::default() });
                if !found.iter().any(|r| r.url == first.url) {
                    f(format!("a search for \"{word}\" misses \"{}\"", first.title));
                }
                if let Some(r) = found.iter().find(|r| !says(&words_of(word), &r.url, &r.title)) {
                    f(format!("a search for \"{word}\" finds \"{}\", which does not say it", r.title));
                }
            }
        }
        // Its known pages and its search.
        if let Some(s) = site {
            let own = origin(start).map(|(o, _)| o).unwrap_or_default();
            for l in s.links {
                let url = link_url(l, &own);
                let at_home = origin(&url).is_some_and(|(_, h)| hosts.iter().any(|p| host_is(&h, &[p.as_str()])));
                println!("   → {:20} {url}{}", l.name, if at_home { "" } else { "   (another host)" });
                if !crate::app_actions::has_symbol(l.symbol) {
                    f(format!("link \"{}\" has no symbol \"{}\"", l.name, l.symbol));
                }
            }
            if let Some(q) = search_url_from(s, "daft punk", Some(start)) {
                println!("   ? {q}");
            }
        }
    }
    // Seam's own: none of the installed webapps' pages.
    let skip: Vec<String> = apps
        .iter()
        .flat_map(|(slug, _, start)| {
            let host = origin(start).map(|(_, h)| h);
            webapp_hosts(site(slug).or_else(|| site_of_host(host.as_deref().unwrap_or(""))), start)
        })
        .collect();
    let got = browser_recents(&seam(), &skip, &Wanted { max: 24, ..Default::default() });
    println!("\n== Seam: {} recents", got.len());
    let mut hosts_seen = HashSet::new();
    for r in &got {
        println!("   {:58} {}", r.title.chars().take(58).collect::<String>(), r.url.chars().take(100).collect::<String>());
        if on_hosts(&r.url, &skip) {
            faults.push(format!("Seam: an installed webapp's page: {}", r.url));
        }
        if !hosts_seen.insert(origin(&r.url).map(|(_, h)| h).unwrap_or_default()) {
            faults.push(format!("Seam: one site twice: {}", r.url));
        }
        if r.picture.is_none() {
            faults.push(format!("Seam: no picture: \"{}\"", r.title));
        }
    }
    println!("\n== {} findings", faults.len());
    for f in &faults {
        println!("!! {f}");
    }
}
