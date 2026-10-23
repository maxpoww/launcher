//! THE WEB'S OWN (Max, 2026-10-22: "podríamos hacer lo mismo con las webapps?
//! … instagram → chats, youtube → suscripciones, recientes en youtube, amazon
//! → recientes en amazon"; then "quiero que funcione en las 40 apps más usadas
//! del mundo"). What a lone app's row (`app_actions.rs`) knows of the web:
//!
//! - SITES: the most used sites of the world, each with the pages Golem knows
//!   it has (`Site::links`: Instagram's Messages, YouTube's Subscriptions,
//!   Amazon's orders…) — the right side of its webapp's row, opened in the
//!   webapp's own window (`seam -golem-app <slug> <url>` loads it there) —,
//!   which of its pages are worth showing as RECENT (`Site::pages`: a video, a
//!   product, a chat), what its titles carry that is not the page's own
//!   (`Site::strip`), and how to SEARCH it (`Site::search`, asked by typing one
//!   of `Site::words` and then the words: "amazon zapatillas").
//! - HISTORY, read where the browsers keep it, never over the network: the
//!   Firefox family (Seam — where the webapps live —, Firefox, LibreWolf:
//!   `places.sqlite`) and the Chromium one (Chrome, Chromium, Brave, Edge,
//!   Vivaldi: `History`), each a COPY read off the loop (the browser holds the
//!   file); and each page's FAVICON as the browser saved it (`favicons.sqlite`,
//!   `Favicons`), written once into the cache as a picture for the thumbnailer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One of a site's pages Golem knows: its name (English, said in the shell's
/// language), where it is (a path from the site's own address, or a whole
/// address) and its symbol (`app_actions::SYMBOLS`).
pub(crate) struct Link {
    pub name: &'static str,
    pub url: &'static str,
    pub symbol: &'static str,
}

const fn link(name: &'static str, url: &'static str, symbol: &'static str) -> Link {
    Link { name, url, symbol }
}

/// One of the sites.
pub(crate) struct Site {
    /// Its webapp's slug in the catalog (`webapps.list`).
    pub slug: &'static str,
    /// Its name, as a search names it ("Search … in YouTube").
    pub name: &'static str,
    /// The hosts its pages are on: `www.youtube.com` exactly, `*.wikipedia.org`
    /// any under it, `www.amazon.*` any ending.
    pub hosts: &'static [&'static str],
    pub links: &'static [Link],
    /// What a page's path holds for it to be shown as recent (none: no
    /// recents; `"/"` alone: any page but the first).
    pub pages: &'static [&'static str],
    /// What its titles carry at their ends that is not the page's own.
    pub strip: &'static [&'static str],
    /// Its search's address, `{q}` the words (`{lang}` the shell's language).
    pub search: Option<&'static str>,
    /// What typed first asks it to search ("youtube gatos").
    pub words: &'static [&'static str],
}

/// THE SITES (most used first, roughly as the world uses them).
pub(crate) const SITES: &[Site] = &[
    Site {
        slug: "youtube",
        name: "YouTube",
        hosts: &["www.youtube.com", "youtube.com", "m.youtube.com"],
        links: &[
            link("Subscriptions", "/feed/subscriptions", "list"),
            link("History", "/feed/history", "clock"),
            link("Watch later", "/playlist?list=WL", "bookmark"),
            link("Shorts", "/shorts", "play"),
        ],
        pages: &["/watch", "/shorts/"],
        strip: &[" - YouTube"],
        search: Some("https://www.youtube.com/results?search_query={q}"),
        words: &["youtube", "yt"],
    },
    Site {
        slug: "google",
        name: "Google",
        hosts: &["www.google.com", "google.com", "www.google.*"],
        links: &[],
        pages: &["/search"],
        strip: &[" - Buscar con Google", " - Google Search", " - Google Suche", " - Recherche Google"],
        search: Some("https://www.google.com/search?q={q}"),
        words: &["google", "g"],
    },
    Site {
        slug: "facebook",
        name: "Facebook",
        hosts: &["www.facebook.com", "facebook.com", "m.facebook.com"],
        links: &[
            link("Messages", "/messages/", "chat"),
            link("Marketplace", "/marketplace/", "cart"),
            link("Groups", "/groups/feed/", "list"),
            link("Notifications", "/notifications", "bell"),
        ],
        pages: &["/groups/", "/marketplace/item/", "/events/"],
        strip: &[" | Facebook"],
        search: Some("https://www.facebook.com/search/top?q={q}"),
        words: &["facebook", "fb"],
    },
    Site {
        slug: "instagram",
        name: "Instagram",
        hosts: &["www.instagram.com", "instagram.com"],
        links: &[
            link("Messages", "/direct/inbox/", "chat"),
            link("Reels", "/reels/", "play"),
            link("Explore", "/explore/", "compass"),
        ],
        pages: &["/p/", "/reel/"],
        strip: &[" • Instagram photos and videos", " • Fotos y videos de Instagram", " • Instagram"],
        search: Some("https://www.instagram.com/explore/search/keyword/?q={q}"),
        words: &["instagram", "ig"],
    },
    Site {
        slug: "whatsapp",
        name: "WhatsApp",
        hosts: &["web.whatsapp.com"],
        links: &[],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "tiktok",
        name: "TikTok",
        hosts: &["www.tiktok.com", "tiktok.com"],
        links: &[
            link("Following", "/following", "list"),
            link("Explore", "/explore", "compass"),
            link("Messages", "/messages", "chat"),
        ],
        pages: &["/video/"],
        strip: &[" | TikTok"],
        search: Some("https://www.tiktok.com/search?q={q}"),
        words: &["tiktok"],
    },
    Site {
        slug: "x",
        name: "X",
        hosts: &["x.com", "twitter.com", "mobile.x.com"],
        links: &[
            link("Messages", "/messages", "chat"),
            link("Notifications", "/notifications", "bell"),
            link("Bookmarks", "/i/bookmarks", "bookmark"),
        ],
        pages: &["/status/"],
        strip: &[" / X"],
        search: Some("https://x.com/search?q={q}"),
        words: &["x", "twitter"],
    },
    Site {
        slug: "reddit",
        name: "Reddit",
        hosts: &["www.reddit.com", "reddit.com", "old.reddit.com"],
        links: &[
            link("Popular", "/r/popular/", "compass"),
            link("Messages", "/message/inbox/", "chat"),
            link("Saved", "/user/me/saved/", "bookmark"),
        ],
        pages: &["/comments/"],
        strip: &[" : reddit", " - Reddit"],
        search: Some("https://www.reddit.com/search/?q={q}"),
        words: &["reddit"],
    },
    Site {
        slug: "wikipedia",
        name: "Wikipedia",
        hosts: &["*.wikipedia.org"],
        links: &[link("Random article", "https://{lang}.wikipedia.org/wiki/Special:Random", "compass")],
        pages: &["/wiki/"],
        strip: &[" - Wikipedia, la enciclopedia libre", " - Wikipedia"],
        search: Some("https://{lang}.wikipedia.org/w/index.php?search={q}"),
        words: &["wikipedia", "wiki"],
    },
    Site {
        slug: "amazon",
        name: "Amazon",
        hosts: &["www.amazon.*", "amazon.*", "smile.amazon.*"],
        links: &[
            link("Your orders", "/gp/css/order-history", "package"),
            link("Cart", "/gp/cart/view.html", "cart"),
            link("Wish list", "/hz/wishlist/ls", "heart"),
        ],
        pages: &["/dp/", "/gp/product/"],
        strip: &["Amazon.com: ", "Amazon.es: ", "Amazon.com.mx: ", "Amazon.de: ", " : Amazon.com", " : Amazon.es", ": Amazon.com.mx"],
        search: Some("https://www.amazon.com/s?k={q}"),
        words: &["amazon"],
    },
    Site {
        slug: "chatgpt",
        name: "ChatGPT",
        hosts: &["chatgpt.com", "chat.openai.com"],
        links: &[link("New chat", "/", "plus")],
        pages: &["/c/"],
        strip: &["ChatGPT - "],
        search: Some("https://chatgpt.com/?q={q}"),
        words: &["chatgpt", "gpt"],
    },
    Site {
        slug: "netflix",
        name: "Netflix",
        hosts: &["www.netflix.com", "netflix.com"],
        links: &[
            link("My list", "/browse/my-list", "bookmark"),
            link("New & popular", "/latest", "star"),
        ],
        pages: &["/title/"],
        strip: &[" | Netflix Official Site", " | Sitio oficial de Netflix", " | Netflix"],
        search: Some("https://www.netflix.com/search?q={q}"),
        words: &["netflix"],
    },
    Site {
        slug: "linkedin",
        name: "LinkedIn",
        hosts: &["www.linkedin.com", "linkedin.com"],
        links: &[
            link("Messages", "/messaging/", "chat"),
            link("My network", "/mynetwork/", "person"),
            link("Jobs", "/jobs/", "package"),
            link("Notifications", "/notifications/", "bell"),
        ],
        pages: &["/in/", "/jobs/view/", "/company/"],
        strip: &[" | LinkedIn"],
        search: Some("https://www.linkedin.com/search/results/all/?keywords={q}"),
        words: &["linkedin"],
    },
    Site {
        slug: "gmail",
        name: "Gmail",
        hosts: &["mail.google.com"],
        links: &[
            link("Compose", "/mail/?view=cm&fs=1", "pen"),
            link("Starred", "/mail/u/0/#starred", "star"),
            link("Sent", "/mail/u/0/#sent", "send"),
        ],
        pages: &[],
        strip: &[],
        search: Some("https://mail.google.com/mail/u/0/#search/{q}"),
        words: &["gmail"],
    },
    Site {
        slug: "outlook",
        name: "Outlook",
        hosts: &["outlook.live.com", "outlook.office.com"],
        links: &[
            link("Compose", "/mail/0/deeplink/compose", "pen"),
            link("Calendar", "/calendar/0/", "clock"),
        ],
        pages: &[],
        strip: &[],
        search: None,
        words: &["outlook"],
    },
    Site {
        slug: "spotify",
        name: "Spotify",
        hosts: &["open.spotify.com"],
        links: &[
            link("Your library", "/collection", "list"),
            link("Liked songs", "/collection/tracks", "heart"),
        ],
        pages: &["/playlist/", "/album/", "/artist/", "/track/", "/show/", "/episode/"],
        strip: &[" | Spotify"],
        search: Some("https://open.spotify.com/search/{q}"),
        words: &["spotify"],
    },
    Site {
        slug: "twitch",
        name: "Twitch",
        hosts: &["www.twitch.tv", "twitch.tv"],
        links: &[
            link("Following", "/directory/following", "list"),
            link("Explore", "/directory", "compass"),
        ],
        pages: &["/videos/"],
        strip: &[" - Twitch"],
        search: Some("https://www.twitch.tv/search?term={q}"),
        words: &["twitch"],
    },
    Site {
        slug: "pinterest",
        name: "Pinterest",
        hosts: &["www.pinterest.com", "pinterest.com", "*.pinterest.com"],
        links: &[],
        pages: &["/pin/"],
        strip: &[" | Pinterest"],
        search: Some("https://www.pinterest.com/search/pins/?q={q}"),
        words: &["pinterest"],
    },
    Site {
        slug: "discord",
        name: "Discord",
        hosts: &["discord.com"],
        links: &[link("Direct messages", "/channels/@me", "chat")],
        pages: &["/channels/"],
        strip: &["Discord | "],
        search: None,
        words: &[],
    },
    Site {
        slug: "telegram",
        name: "Telegram",
        hosts: &["web.telegram.org"],
        links: &[],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "google-maps",
        name: "Google Maps",
        hosts: &["maps.google.com", "www.google.com"],
        links: &[],
        pages: &["/maps/place/"],
        strip: &[" - Google Maps"],
        search: Some("https://www.google.com/maps/search/{q}"),
        words: &["maps", "mapa"],
    },
    Site {
        slug: "google-drive",
        name: "Google Drive",
        hosts: &["drive.google.com"],
        links: &[
            link("Recent", "/drive/recent", "clock"),
            link("Shared with me", "/drive/shared-with-me", "person"),
            link("Starred", "/drive/starred", "star"),
        ],
        pages: &["/file/d/", "/drive/folders/"],
        strip: &[" - Google Drive"],
        search: Some("https://drive.google.com/drive/search?q={q}"),
        words: &["drive"],
    },
    Site {
        slug: "google-docs",
        name: "Google Docs",
        hosts: &["docs.google.com"],
        links: &[
            link("New document", "https://docs.new", "plus"),
            link("New spreadsheet", "https://sheets.new", "plus"),
            link("New presentation", "https://slides.new", "plus"),
        ],
        pages: &["/document/d/", "/spreadsheets/d/", "/presentation/d/", "/forms/d/"],
        strip: &[
            " - Documentos de Google",
            " - Hojas de cálculo de Google",
            " - Presentaciones de Google",
            " - Google Docs",
            " - Google Sheets",
            " - Google Slides",
            " - Google Forms",
        ],
        search: None,
        words: &["docs"],
    },
    Site {
        slug: "google-calendar",
        name: "Google Calendar",
        hosts: &["calendar.google.com"],
        links: &[
            link("New event", "/calendar/r/eventedit", "plus"),
            link("Today", "/calendar/r/day", "clock"),
        ],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "google-photos",
        name: "Google Photos",
        hosts: &["photos.google.com"],
        links: &[link("Albums", "/albums", "picture")],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "google-meet",
        name: "Google Meet",
        hosts: &["meet.google.com"],
        links: &[link("New meeting", "/new", "plus")],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "google-translate",
        name: "Google Translate",
        hosts: &["translate.google.com"],
        links: &[],
        pages: &[],
        strip: &[],
        search: Some("https://translate.google.com/?sl=auto&text={q}"),
        words: &["translate", "traducir"],
    },
    Site {
        slug: "claude",
        name: "Claude",
        hosts: &["claude.ai"],
        links: &[link("New chat", "/new", "plus"), link("Projects", "/projects", "list")],
        pages: &["/chat/"],
        strip: &[" - Claude"],
        search: Some("https://claude.ai/new?q={q}"),
        words: &["claude"],
    },
    Site {
        slug: "gemini",
        name: "Gemini",
        hosts: &["gemini.google.com"],
        links: &[link("New chat", "/app", "plus")],
        pages: &["/app/"],
        strip: &[" - Gemini", "Gemini - "],
        search: None,
        words: &["gemini"],
    },
    Site {
        slug: "ebay",
        name: "eBay",
        hosts: &["www.ebay.com", "www.ebay.*", "ebay.com"],
        links: &[
            link("Purchases", "/mye/myebay/purchase", "package"),
            link("Watchlist", "/mye/myebay/watchlist", "heart"),
        ],
        pages: &["/itm/"],
        strip: &[" | eBay"],
        search: Some("https://www.ebay.com/sch/i.html?_nkw={q}"),
        words: &["ebay"],
    },
    Site {
        slug: "github",
        name: "GitHub",
        hosts: &["github.com"],
        links: &[
            link("Notifications", "/notifications", "bell"),
            link("Pull requests", "/pulls", "send"),
            link("Issues", "/issues", "list"),
        ],
        pages: &["/"],
        strip: &[" · GitHub"],
        search: Some("https://github.com/search?q={q}"),
        words: &["github", "gh"],
    },
    Site {
        slug: "zoom",
        name: "Zoom",
        hosts: &["app.zoom.us", "zoom.us"],
        links: &[],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "canva",
        name: "Canva",
        hosts: &["www.canva.com", "canva.com"],
        links: &[link("Your designs", "/projects", "list")],
        pages: &["/design/"],
        strip: &[" - Canva"],
        search: Some("https://www.canva.com/search/templates?q={q}"),
        words: &["canva"],
    },
    Site {
        slug: "duolingo",
        name: "Duolingo",
        hosts: &["www.duolingo.com", "duolingo.com"],
        links: &[link("Learn", "/learn", "play")],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "aliexpress",
        name: "AliExpress",
        hosts: &["www.aliexpress.com", "*.aliexpress.com", "www.aliexpress.*"],
        links: &[
            link("Your orders", "/p/order/index.html", "package"),
            link("Cart", "/p/shoppingcart/index.html", "cart"),
        ],
        pages: &["/item/"],
        strip: &[" - AliExpress"],
        search: Some("https://www.aliexpress.com/wholesale?SearchText={q}"),
        words: &["aliexpress", "ali"],
    },
    Site {
        slug: "booking",
        name: "Booking.com",
        hosts: &["www.booking.com", "booking.com"],
        links: &[],
        pages: &["/hotel/"],
        strip: &[],
        search: Some("https://www.booking.com/searchresults.html?ss={q}"),
        words: &["booking"],
    },
    Site {
        slug: "airbnb",
        name: "Airbnb",
        hosts: &["www.airbnb.com", "www.airbnb.*", "airbnb.com"],
        links: &[
            link("Trips", "/trips/v1", "package"),
            link("Wishlists", "/wishlists", "heart"),
            link("Messages", "/guest/messages", "chat"),
        ],
        pages: &["/rooms/"],
        strip: &[" - Airbnb"],
        search: Some("https://www.airbnb.com/s/{q}/homes"),
        words: &["airbnb"],
    },
    Site {
        slug: "youtube-music",
        name: "YouTube Music",
        hosts: &["music.youtube.com"],
        links: &[
            link("Library", "/library", "list"),
            link("Liked music", "/playlist?list=LM", "heart"),
            link("Explore", "/explore", "compass"),
        ],
        pages: &["/watch", "/playlist", "/browse/"],
        strip: &[" - YouTube Music"],
        search: Some("https://music.youtube.com/search?q={q}"),
        words: &["ytmusic"],
    },
    Site {
        slug: "messenger",
        name: "Messenger",
        hosts: &["www.messenger.com", "messenger.com"],
        links: &[],
        pages: &[],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "steam",
        name: "Steam",
        hosts: &["store.steampowered.com"],
        links: &[link("Wishlist", "/wishlist/", "heart"), link("Cart", "/cart/", "cart")],
        pages: &["/app/"],
        strip: &[" on Steam", " en Steam"],
        search: Some("https://store.steampowered.com/search/?term={q}"),
        words: &[],
    },
    Site {
        slug: "stackoverflow",
        name: "Stack Overflow",
        hosts: &["stackoverflow.com"],
        links: &[],
        pages: &["/questions/"],
        strip: &[" - Stack Overflow"],
        search: Some("https://stackoverflow.com/search?q={q}"),
        words: &["stackoverflow", "so"],
    },
    Site {
        slug: "notion",
        name: "Notion",
        hosts: &["www.notion.so", "notion.so"],
        links: &[],
        pages: &["/"],
        strip: &[],
        search: None,
        words: &[],
    },
    Site {
        slug: "figma",
        name: "Figma",
        hosts: &["www.figma.com", "figma.com"],
        links: &[],
        pages: &["/file/", "/design/"],
        strip: &[" – Figma", " - Figma"],
        search: None,
        words: &[],
    },
];

/// The site of webapp `slug`.
pub(crate) fn site(slug: &str) -> Option<&'static Site> {
    SITES.iter().find(|s| s.slug == slug)
}

/// The site whose pages are on `host`.
pub(crate) fn site_of_host(host: &str) -> Option<&'static Site> {
    SITES.iter().find(|s| host_is(host, s.hosts))
}

/// A search asked by what was typed: one of a site's words, then the words
/// to search ("amazon zapatillas" → Amazon, "zapatillas").
pub(crate) fn asked(query: &str) -> Option<(&'static Site, String)> {
    let (first, rest) = query.trim_start().split_once(char::is_whitespace)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let first = first.to_lowercase();
    let site = SITES.iter().find(|s| s.search.is_some() && s.words.contains(&first.as_str()))?;
    Some((site, rest.to_owned()))
}

/// `s` made safe in a URL's query.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The shell's language as Wikipedia names it ("es", "en").
fn lang() -> String {
    let l = crate::i18n::lang().unwrap_or_default();
    l.split(['-', '_']).next().filter(|l| !l.is_empty()).unwrap_or("en").to_lowercase()
}

/// Where a search of `words` on `site` goes.
pub(crate) fn search_url(site: &Site, words: &str) -> Option<String> {
    Some(site.search?.replace("{lang}", &lang()).replace("{q}", &encode(words)))
}

/// Where a search of `words` on `site` goes for a webapp that opens at
/// `start`: on that webapp's own country of the site (amazon.com.mx), when
/// the site is one of many endings.
pub(crate) fn search_url_from(site: &Site, words: &str, start: Option<&str>) -> Option<String> {
    let url = search_url(site, words)?;
    let ends = |host: &str| site.hosts.iter().any(|p| p.ends_with(".*") && host_is(host, &[p]));
    match start.and_then(origin).zip(origin(&url)) {
        Some(((theirs, host), (own, _))) if ends(&host) && theirs != own => Some(format!("{theirs}{}", &url[own.len()..])),
        _ => Some(url),
    }
}

/// Where `link` goes, on a site whose webapp opens at `origin`
/// (`https://www.youtube.com`).
pub(crate) fn link_url(link: &Link, origin: &str) -> String {
    let url = link.url.replace("{lang}", &lang());
    if url.starts_with("https://") || url.starts_with("http://") {
        url
    } else {
        format!("{}{url}", origin.trim_end_matches('/'))
    }
}

/// An address's scheme and host (`https://www.youtube.com`), and its host.
pub(crate) fn origin(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_owned();
    Some((format!("{scheme}://{host}"), host))
}

/// Whether `host` is one of `patterns` (`www.youtube.com`, `*.wikipedia.org`,
/// `www.amazon.*`).
fn host_is(host: &str, patterns: &[&str]) -> bool {
    let host = host.to_lowercase();
    patterns.iter().any(|p| {
        if let Some(tail) = p.strip_prefix("*.") {
            host == tail || host.ends_with(&format!(".{tail}"))
        } else if let Some(head) = p.strip_suffix(".*") {
            host.strip_prefix(head).is_some_and(|rest| rest.starts_with('.') && !rest[1..].contains('/'))
        } else {
            host == *p
        }
    })
}

/// Whether the page at `url` is one of `site`'s worth showing as recent.
fn is_page(site: &Site, url: &str) -> bool {
    let Some((_, host)) = origin(url) else {
        return false;
    };
    if !host_is(&host, site.hosts) || site.pages.is_empty() {
        return false;
    }
    let path = url.split_once("://").map_or("", |(_, r)| r.find('/').map_or("", |i| &r[i..]));
    if site.pages == ["/"] {
        // Any page but the first, and none of the site's own known pages.
        let bare = path.split(['?', '#']).next().unwrap_or("");
        return bare.len() > 1 && !site.links.iter().any(|l| l.url.starts_with(bare));
    }
    site.pages.iter().any(|p| path.contains(p))
}

/// A page's title, without what the site adds to every one.
pub(crate) fn clean_title(site: Option<&Site>, title: &str) -> String {
    let mut t = title.trim().to_owned();
    // A count of what is new, "(3) Inbox": not the page's.
    if t.starts_with('(') {
        if let Some(end) = t.find(") ") {
            if t[1..end].chars().all(|c| c.is_ascii_digit() || c == '+') {
                t = t[end + 2..].to_owned();
            }
        }
    }
    for s in site.map_or(&[][..], |s| s.strip) {
        if let Some(rest) = t.strip_suffix(s) {
            t = rest.trim().to_owned();
        }
        if let Some(rest) = t.strip_prefix(s) {
            t = rest.trim().to_owned();
        }
    }
    t
}

// ---- the browsers -------------------------------------------------------------

/// Where a browser keeps its history.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Browser {
    /// A Firefox profile's folder (`places.sqlite`, `favicons.sqlite`;
    /// Seam's own also its disk cache, `cache2`).
    Firefox(PathBuf),
    /// A Chromium profile's folder (`History`, `Favicons`).
    Chromium(PathBuf),
}

fn home() -> PathBuf {
    crate::filer::home()
}

/// A folder of the dock's cache (`~/.cache/waverunner/<what>`), made.
fn cache(what: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache"));
    let dir = base.join("waverunner").join(what);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Seam's profile: where every webapp's pages are.
pub(crate) fn seam() -> Browser {
    Browser::Firefox(home().join(".local/share/seam"))
}

/// The newest profile of a Firefox-like browser under `dir` (by its history's age).
fn newest_profile(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let at = std::fs::metadata(p.join("places.sqlite")).and_then(|m| m.modified()).ok()?;
            Some((at, p))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, p)| p)
}

/// The browser an app's program is, and where its history is.
pub(crate) fn browser_for(program: &str) -> Option<Browser> {
    let config = home().join(".config");
    let chromium = |dir: &str| Some(Browser::Chromium(config.join(dir).join("Default")));
    match program.to_lowercase().as_str() {
        "seam" => Some(seam()),
        "firefox" | "firefox-esr" => newest_profile(&home().join(".mozilla/firefox")).map(Browser::Firefox),
        "librewolf" => newest_profile(&home().join(".librewolf")).map(Browser::Firefox),
        "google-chrome" | "google-chrome-stable" => chromium("google-chrome"),
        "chromium" | "chromium-browser" => chromium("chromium"),
        "brave" | "brave-browser" => chromium("BraveSoftware/Brave-Browser"),
        "microsoft-edge" | "microsoft-edge-stable" => chromium("microsoft-edge"),
        "vivaldi" | "vivaldi-stable" => chromium("vivaldi"),
        _ => None,
    }
}

/// One page of a history.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Visit {
    pub url: String,
    pub title: String,
    /// When it was last opened (unix seconds).
    pub last: f64,
    /// How much it is in use: its visits, each forgetting as the dock's
    /// launches do (`usage::decayed`).
    pub score: f64,
    /// Its picture as Seam kept it, when it is one Seam heard play.
    pub pic: Option<String>,
}

/// A short, steady name for some bytes (a cache file's).
fn hash_of(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// A copy of a browser's database, open, for ONE reading: gone (with its
/// log and shared memory) when it is dropped. Each reading has its own —
/// two at once (YouTube's and YouTube Music's recents asked a key apart)
/// shared one name, and each copied over and deleted the log of the file
/// the other was reading.
struct Copied {
    conn: Option<rusqlite::Connection>,
    path: PathBuf,
}

impl std::ops::Deref for Copied {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &rusqlite::Connection {
        self.conn.as_ref().expect("open until dropped")
    }
}

impl Drop for Copied {
    fn drop(&mut self) {
        drop(self.conn.take());
        for end in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{end}", self.path.display()));
        }
    }
}

/// A copy of a browser's database (and its write-ahead log) in the cache,
/// opened: the browser holds the file and keeps writing it.
fn copy_open(path: &Path) -> Option<Copied> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    static SWEPT: std::sync::Once = std::sync::Once::new();
    let dir = cache("history")?;
    // Copies a dock that died left behind (and the old shared ones).
    SWEPT.call_once(|| sweep(&dir, Duration::from_secs(600)));
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let to = dir.join(format!("{}-{}-{n}", hash_of(path.as_os_str().as_encoded_bytes()), std::process::id()));
    let wal = |p: &Path| PathBuf::from(format!("{}-wal", p.display()));
    // (Made first: a failed copy or open leaves nothing behind either.)
    let mut copied = Copied { conn: None, path: to.clone() };
    std::fs::copy(path, &to).ok()?;
    let _ = std::fs::copy(wal(path), wal(&to));
    copied.conn = Some(rusqlite::Connection::open(&to).ok()?);
    Some(copied)
}

/// Files of `dir` older than `age`, removed.
fn sweep(dir: &Path, age: Duration) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > age);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A cache folder of pictures kept under `keep` files: past it, the oldest
/// go down to three quarters of it (each is made again if it is wanted).
fn trim_dir(dir: &Path, keep: usize) {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    let extra = files.len() - keep * 3 / 4;
    for (_, path) in files.into_iter().take(extra) {
        let _ = std::fs::remove_file(path);
    }
}

/// How many pictures each cache folder keeps.
const PICTURES_KEPT: usize = 1500;

/// A picture from bytes a page had a hand in, decoded within bounds: no
/// side over 4096, no more than 64 MB held (a small file can claim a huge
/// picture).
fn decode(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

/// A browser's newest pages, newest first (`max` read).
fn history(browser: &Browser, max: usize) -> Vec<Visit> {
    // Firefox keeps microseconds since 1970; Chromium since 1601.
    const CHROMIUM_EPOCH: f64 = 11_644_473_600.0;
    let (db, pages, visits, epoch) = match browser {
        Browser::Firefox(p) => (
            p.join("places.sqlite"),
            "SELECT id, url, COALESCE(title, ''), last_visit_date FROM moz_places WHERE last_visit_date IS NOT NULL \
             AND url LIKE 'http%' ORDER BY last_visit_date DESC LIMIT ?1",
            "SELECT place_id, visit_date FROM moz_historyvisits WHERE visit_date > ?1",
            0.0,
        ),
        Browser::Chromium(p) => (
            p.join("History"),
            "SELECT id, url, COALESCE(title, ''), last_visit_time FROM urls WHERE url LIKE 'http%' \
             ORDER BY last_visit_time DESC LIMIT ?1",
            "SELECT url, visit_time FROM visits WHERE visit_time > ?1",
            CHROMIUM_EPOCH,
        ),
    };
    let Some(conn) = copy_open(&db) else {
        return Vec::new();
    };
    let secs = |t: i64| t as f64 / 1e6 - epoch;
    let now = crate::usage::now_secs();
    // The visits of the last weeks, each worth what is left of it (older
    // ones are worth next to nothing); a history without them: none.
    let mut score: HashMap<i64, f64> = HashMap::new();
    let since = ((now - 30.0 * 24.0 * 3600.0 + epoch) * 1e6) as i64;
    if let Ok(mut stmt) = conn.prepare(visits) {
        if let Ok(rows) = stmt.query_map([since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))) {
            for (id, at) in rows.flatten() {
                *score.entry(id).or_default() += crate::usage::decayed(now - secs(at));
            }
        }
    }
    let Ok(mut stmt) = conn.prepare(pages) else {
        return Vec::new();
    };
    let rows = stmt.query_map([max as i64], |r| {
        let id: i64 = r.get(0)?;
        Ok(Visit {
            url: r.get(1)?,
            title: r.get(2)?,
            last: secs(r.get::<_, Option<i64>>(3)?.unwrap_or(0)),
            score: score.get(&id).copied().unwrap_or(0.0),
            pic: None,
        })
    });
    rows.map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// What the owner picked lately from a lone app's recents, by its address
/// or path: how much (as `usage` scores) and when last (unix seconds).
pub(crate) type Picks = HashMap<String, (f64, f64)>;

/// How many of the newest come first in a lone app's recents, by time
/// alone (Max, 2026-10-22: "los primeros tres ítems sí sean recientes
/// puros… la idea es que pueda retomar ahí lo que dejé ayer, o esta mañana").
pub(crate) const NEWEST_FIRST: usize = 3;
/// What a pick from the box is worth, in visits: a choice of one's own.
const PICK_WEIGHT: f64 = 3.0;

/// THE ORDER of a lone app's recents: the `NEWEST_FIRST` last opened, by
/// time — to take up where one left off —, then the rest by how much they
/// are in use (their score; at equal use the newer first); at most `max`.
/// Each item: what it is, when last opened, its score.
pub(crate) fn rank<T>(mut items: Vec<(T, f64, f64)>, max: usize) -> Vec<T> {
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    let rest = items.split_off(NEWEST_FIRST.min(items.len()));
    let mut rest = rest;
    rest.sort_by(|a, b| b.2.total_cmp(&a.2).then(b.1.total_cmp(&a.1)));
    items.into_iter().chain(rest).take(max).map(|(t, _, _)| t).collect()
}

/// `pages` with the owner's picks counted in, in THE ORDER, at most `max`.
fn ranked_pages(pages: Vec<(Visit, String)>, picks: &Picks, max: usize) -> Vec<(Visit, String)> {
    let items = pages
        .into_iter()
        .map(|(v, title)| {
            let (pick, picked) = picks.get(&v.url).copied().unwrap_or((0.0, 0.0));
            let (last, score) = (v.last.max(picked), v.score + PICK_WEIGHT * pick);
            ((v, title), last, score)
        })
        .collect();
    rank(items, max)
}

/// The browser's favicons database, copied to read.
fn favicons_db(browser: &Browser) -> Option<Copied> {
    copy_open(&match browser {
        Browser::Firefox(p) => p.join("favicons.sqlite"),
        Browser::Chromium(p) => p.join("Favicons"),
    })
}

/// The favicon a browser keeps of the page at `url` (the biggest; a
/// drawing first), or of its site when the page has none of its own.
fn favicon(browser: &Browser, conn: &rusqlite::Connection, url: &str) -> Option<Vec<u8>> {
    match browser {
        Browser::Firefox(_) => conn
            .query_row(
                "SELECT i.data FROM moz_pages_w_icons p JOIN moz_icons_to_pages ip ON ip.page_id = p.id \
                 JOIN moz_icons i ON i.id = ip.icon_id WHERE p.page_url = ?1 AND i.data IS NOT NULL \
                 ORDER BY i.width DESC LIMIT 1",
                [url],
                |r| r.get(0),
            )
            .or_else(|_| {
                let (origin, _) = origin(url).ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                conn.query_row(
                    "SELECT data FROM moz_icons WHERE icon_url LIKE ?1 AND data IS NOT NULL \
                     ORDER BY root DESC, width DESC LIMIT 1",
                    [format!("{origin}/%")],
                    |r| r.get(0),
                )
            })
            .ok(),
        Browser::Chromium(_) => conn
            .query_row(
                "SELECT b.image_data FROM icon_mapping m JOIN favicon_bitmaps b ON b.icon_id = m.icon_id \
                 WHERE m.page_url = ?1 ORDER BY b.width DESC LIMIT 1",
                [url],
                |r| r.get(0),
            )
            .ok(),
    }
}

/// How big a favicon stands on its tile, of the tile.
const FAVICON_ON_TILE: f32 = 0.5;
/// The tile's colour under a favicon (a light card: favicons are drawn for
/// light pages).
const TILE_GROUND: [u8; 4] = [243, 243, 243, 255];

/// A favicon (PNG, JPEG, WebP or a drawing) as a TILE: centred on a light
/// card the size of an icon, written once into `dir` (named by its bytes)
/// for the thumbnailer, which rounds its corners as a file's picture.
fn tile_in(dir: &Path, data: &[u8]) -> Option<PathBuf> {
    let path = dir.join(format!("{}-t1.png", hash_of(data)));
    if path.exists() {
        return Some(path);
    }
    let side = crate::apps::ICON_SIZE;
    let room = (side as f32 * FAVICON_ON_TILE) as u32;
    let svg = data[..data.len().min(512)].windows(4).any(|w| w == b"<svg");
    let fav = if svg { svg_rgba(data, room)? } else { decode(data)?.to_rgba8() };
    let fav = if fav.width() == room || fav.height() == room {
        fav
    } else {
        let k = room as f32 / fav.width().max(fav.height()).max(1) as f32;
        let (w, h) = (((fav.width() as f32 * k).round() as u32).max(1), ((fav.height() as f32 * k).round() as u32).max(1));
        image::imageops::resize(&fav, w, h, image::imageops::FilterType::CatmullRom)
    };
    let mut tile = image::RgbaImage::from_pixel(side, side, image::Rgba(TILE_GROUND));
    let (x, y) = ((side - fav.width()) / 2, (side - fav.height()) / 2);
    image::imageops::overlay(&mut tile, &fav, i64::from(x), i64::from(y));
    let tmp = path.with_extension("part");
    tile.save_with_format(&tmp, image::ImageFormat::Png).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// A drawing's pixels, `side` on its longer side (straight alpha).
fn svg_rgba(data: &[u8], side: u32) -> Option<image::RgbaImage> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let k = side as f32 / size.width().max(size.height()).max(1.0);
    let (w, h) = (((size.width() * k).round() as u32).max(1), ((size.height() * k).round() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    let mut px = pixmap.take();
    for p in px.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(w, h, px)
}

// ---- YouTube's pictures, from Seam's disk cache -------------------------------

/// The video a YouTube page shows (`/watch?v=…`, `/shorts/…`), by its id.
fn video_id(url: &str) -> Option<&str> {
    let (_, host) = origin(url)?;
    if !host.ends_with("youtube.com") {
        return None;
    }
    let by_query = url.split_once('?').and_then(|(_, q)| q.split(['&', '#']).find_map(|kv| kv.strip_prefix("v=")));
    let id = by_query.or_else(|| url.split_once("/shorts/").map(|(_, r)| r))?;
    let id = id.split(['&', '#', '?', '/']).next()?;
    (id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then_some(id)
}

/// A cache entry's key — its address. Firefox's cache file is the body,
/// then its metadata, whose place is the file's last four bytes; the key
/// follows a few numbers there (`a,:https://…`, maybe after a partition).
fn cache_key(path: &Path) -> Option<(String, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len < 8 {
        return None;
    }
    f.seek(SeekFrom::End(-4)).ok()?;
    let mut b = [0u8; 4];
    f.read_exact(&mut b).ok()?;
    let body = u64::from(u32::from_be_bytes(b));
    if body >= len {
        return None;
    }
    f.seek(SeekFrom::Start(body)).ok()?;
    let mut meta = vec![0u8; ((len - body) as usize).min(4096)];
    f.read_exact(&mut meta).ok()?;
    let at = meta.windows(5).position(|w| w == b":http")? + 1;
    let end = meta[at..].iter().position(|&c| c == 0).map_or(meta.len(), |e| at + e);
    Some((String::from_utf8_lossy(&meta[at..end]).into_owned(), body))
}

/// How good a picture of a video its file's name says it is (none: not one).
fn picture_rank(name: &str) -> u8 {
    match name.split(['.', '?']).next().unwrap_or("") {
        n if n.starts_with("hq720") => 4,
        "mqdefault" => 3,
        "hqdefault" | "sddefault" => 2,
        _ => 0,
    }
}

/// The pictures of videos in a cache, by video id: how good, the entry, its
/// body's length.
type Pictures = HashMap<String, (u8, PathBuf, u64)>;

/// Where Seam's cache keeps a picture of each video, by its id (the best
/// it has): an index made once each time the cache's folder changes.
fn video_pictures(cache2: &Path) -> Pictures {
    static INDEX: std::sync::Mutex<Option<(PathBuf, std::time::SystemTime, Pictures)>> = std::sync::Mutex::new(None);
    let entries = cache2.join("entries");
    let Ok(changed) = std::fs::metadata(&entries).and_then(|m| m.modified()) else {
        return HashMap::new();
    };
    if let Ok(index) = INDEX.lock() {
        if let Some((_, _, found)) = index.as_ref().filter(|(d, at, _)| *d == entries && *at == changed) {
            return found.clone();
        }
    }
    let mut found = Pictures::new();
    for e in std::fs::read_dir(&entries).into_iter().flatten().flatten() {
        let path = e.path();
        let Some((key, body)) = cache_key(&path) else {
            continue;
        };
        let Some(rest) = key.split_once("i.ytimg.com/").map(|(_, r)| r) else {
            continue;
        };
        let mut parts = rest.split('/');
        let (Some(kind), Some(id), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let rank = picture_rank(name);
        if !kind.starts_with("vi") || rank == 0 {
            continue;
        }
        if found.get(id).is_none_or(|(r, _, _)| *r < rank) {
            found.insert(id.to_owned(), (rank, path, body));
        }
    }
    if let Ok(mut index) = INDEX.lock() {
        *index = Some((entries, changed, found.clone()));
    }
    found
}

/// A video's picture from a cache entry (`body` bytes long), as a file of
/// ours: its middle, SQUARE, as big as a tile — the tile is square, and a
/// wide picture in it is a thin strip (the black bands of a 4:3 one are
/// inside what is cut away).
fn video_picture(dir: &Path, id: &str, entry: &Path, body: u64) -> Option<PathBuf> {
    let path = dir.join(format!("{id}-s1.png"));
    if path.exists() {
        return Some(path);
    }
    let bytes = std::fs::read(entry).ok()?;
    squared(bytes.get(..body as usize)?, &path)
}

/// A picture's middle, SQUARE, as big as a tile, written to `path` (a
/// PNG): the tile is square, and a wide picture in it is a thin strip (the
/// black bands of a 4:3 video picture are inside what is cut away).
fn squared(bytes: &[u8], path: &Path) -> Option<PathBuf> {
    let img = decode(bytes)?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    // (A 4:3 one is a 16:9 picture between bands: its square within that.)
    let side = if w * 3 == h * 4 { w * 9 / 16 } else { w.min(h) };
    let img = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let tile = crate::apps::ICON_SIZE;
    let img = if side > tile { img.resize(tile, tile, image::imageops::FilterType::Triangle) } else { img };
    let tmp = path.with_extension("part");
    img.to_rgba8().save_with_format(&tmp, image::ImageFormat::Png).ok()?;
    std::fs::rename(&tmp, path).ok()?;
    Some(path.to_owned())
}

// ---- Seam's kitchen ---------------------------------------------------------------

/// WHAT SEAM COOKED (Max, 2026-10-22: "can we make Seam leave the files 'cooked' and pick
/// them up with the shell from a dir?" — the dock never goes on the network): Seam's chrome
/// script (Golem's `seam/golem-chrome.js`, THE RECENTS' KITCHEN) keeps what the pages it
/// shows really are — their title as it is now, their preview picture — and what they
/// play (song, artist, cover), in `$XDG_STATE_HOME/golem/seam/recents/`. Read here as
/// DATA a page had a hand in: the file at most `KITCHEN_MAX`, addresses only http(s),
/// words clipped, pictures only files of Seam's own naming in its `pics/`, no link.
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub(crate) struct Kitchen {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    pages: HashMap<String, KitchenPage>,
    #[serde(default)]
    plays: Vec<KitchenPlay>,
    #[serde(skip)]
    dir: PathBuf,
    /// Pictures more than one page wears: a site's logo, or the first
    /// page's of an app that changes page without loading — no page's own.
    #[serde(skip)]
    shared: HashSet<String>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
struct KitchenPage {
    #[serde(default)]
    title: String,
    #[serde(default)]
    pic: Option<String>,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
struct KitchenPlay {
    #[serde(default)]
    host: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    at: f64,
    #[serde(default)]
    n: u32,
    #[serde(default)]
    pic: Option<String>,
}

/// The most of Seam's file read.
const KITCHEN_MAX: u64 = 4 * 1024 * 1024;
/// The most of one of its pictures read.
const KITCHEN_PIC_MAX: u64 = 2 * 1024 * 1024;

/// Where Seam keeps what it cooked.
fn kitchen_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"));
    state.join("golem/seam/recents")
}

/// What Seam cooked, as it stands (none when it has not, or is not ours).
pub(crate) fn kitchen() -> Kitchen {
    kitchen_in(&kitchen_dir())
}

fn kitchen_in(dir: &Path) -> Kitchen {
    let path = dir.join("recents.json");
    let small = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= KITCHEN_MAX);
    let read = small.then(|| std::fs::read(&path).ok()).flatten();
    match read.and_then(|b| serde_json::from_slice::<Kitchen>(&b).ok()).filter(|k| k.v == 1) {
        Some(mut k) => {
            k.dir = dir.to_owned();
            let mut seen = HashSet::new();
            for pic in k.pages.values().filter_map(|p| p.pic.clone()) {
                if !seen.insert(pic.clone()) {
                    k.shared.insert(pic);
                }
            }
            k
        }
        None => Kitchen::default(),
    }
}

/// An http(s) address of a sane length.
fn is_web(url: &str) -> bool {
    url.len() <= 2000 && (url.starts_with("https://") || url.starts_with("http://"))
}

impl Kitchen {
    /// The title Seam saw last on the page at `url`.
    fn title_of(&self, url: &str) -> Option<String> {
        let t = self.pages.get(url)?.title.trim();
        (!t.is_empty()).then(|| t.chars().take(300).collect())
    }

    /// One of Seam's pictures, by its name — only a file of its own naming
    /// (`<16 hex>.<jpg|png|webp>`), in its `pics/`, a plain small file.
    fn picture(&self, name: &str) -> Option<PathBuf> {
        let (stem, ext) = name.split_once('.')?;
        let ours = stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit()) && matches!(ext, "jpg" | "png" | "webp");
        let path = self.dir.join("pics").join(name);
        let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= KITCHEN_PIC_MAX);
        (ours && plain && !self.dir.as_os_str().is_empty()).then_some(path)
    }

    /// The preview picture Seam kept of the page at `url` — when it is the
    /// page's own, not one several pages wear.
    fn page_picture(&self, url: &str) -> Option<PathBuf> {
        let pic = self.pages.get(url)?.pic.as_deref()?;
        (!self.shared.contains(pic)).then(|| self.picture(pic)).flatten()
    }
}

/// What a music site's webapp played, by its slug: "song • artist".
const MUSIC: [&str; 4] = ["spotify", "youtube-music", "soundcloud", "deezer"];

/// What the webapp of `site` (opening at `start`) PLAYED, as Seam heard it
/// from the page: each a page to show — a song of a music site named with
/// its artist ("In the End • Linkin Park"), opened where it played when that
/// is a page of the site's own (a YouTube Music song), else as a search of
/// the site for it (Spotify plays from a search page); with its cover.
fn played(k: &Kitchen, site: Option<&Site>, start: &str, now: f64) -> Vec<(Visit, String)> {
    let host = origin(start).map(|(_, h)| h.to_lowercase()).unwrap_or_default();
    let hosts: Vec<&str> = site.map_or_else(|| vec![host.as_str()], |s| s.hosts.to_vec());
    let music = site.is_some_and(|s| MUSIC.contains(&s.slug));
    k.plays
        .iter()
        .filter(|p| host_is(&p.host, &hosts) && is_web(&p.url) && !p.title.trim().is_empty())
        // (A YouTube play away from its video's page was heard in a mini player on
        // another page: it would open that page, not the video.)
        .filter(|p| !p.host.ends_with("youtube.com") || video_id(&p.url).is_some())
        .map(|p| {
            let (song, artist): (String, String) = (p.title.trim().chars().take(200).collect(), p.artist.trim().chars().take(120).collect());
            let title = if music && !artist.is_empty() { format!("{song} • {artist}") } else { song.clone() };
            let own = site.is_some_and(|s| is_page(s, &p.url));
            let url = match site.filter(|_| !own).and_then(|s| search_url(s, &format!("{song} {artist}"))) {
                Some(search) => search,
                None => p.url.clone(),
            };
            let pic = p.pic.as_deref().and_then(|n| k.picture(n)).map(|p| p.to_string_lossy().into_owned());
            let visit = Visit { url, title: title.clone(), last: p.at, score: f64::from(p.n.max(1)) * crate::usage::decayed(now - p.at), pic };
            (visit, title)
        })
        .collect()
}

// ---- the recents ----------------------------------------------------------------

/// A recent page as a row of a lone app shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recent {
    /// Its address — or, an app's file, its path.
    pub url: String,
    pub title: String,
    /// Its picture as a file: the video's own (YouTube, from Seam's cache),
    /// else its favicon on a tile; none without either.
    pub picture: Option<PathBuf>,
    /// A folder (an app's recent file that is one).
    pub dir: bool,
}

/// `pages` made recents: each one's picture found.
fn pictured(browser: &Browser, kitchen: &Kitchen, pages: Vec<(Visit, String)>) -> Vec<Recent> {
    let icons = favicons_db(browser);
    let tiles = cache("web-tiles");
    let videos = match browser {
        Browser::Firefox(p) if p.join("cache2").is_dir() => video_pictures(&p.join("cache2")),
        _ => HashMap::new(),
    };
    let shots = cache("web-pictures");
    let out: Vec<Recent> = pages
        .into_iter()
        .map(|(v, title)| {
            // What Seam heard it play, then the video's own from Seam's cache
            // (by its id: it cannot be another's), then the preview Seam kept,
            // then the favicon.
            let cooked = |src: PathBuf| {
                let out = shots.as_deref()?.join(format!("k-{}-s1.png", src.file_stem()?.to_string_lossy()));
                if out.exists() {
                    return Some(out);
                }
                let bytes = std::fs::read(&src).ok()?;
                squared(&bytes, &out)
            };
            let video = || {
                video_id(&v.url)
                    .and_then(|id| Some((id, videos.get(id)?)))
                    .and_then(|(id, (_, entry, body))| video_picture(shots.as_deref()?, id, entry, *body))
            };
            let played = v.pic.as_deref().map(PathBuf::from).and_then(cooked);
            let picture = played.or_else(video).or_else(|| kitchen.page_picture(&v.url).and_then(cooked)).or_else(|| {
                let data = favicon(browser, icons.as_deref()?, &v.url)?;
                tile_in(tiles.as_deref()?, &data)
            });
            Recent { url: v.url, title, picture, dir: false }
        })
        .collect();
    // (Now and then: the picture caches kept under their bound.)
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    if CALLS.fetch_add(1, Ordering::Relaxed).is_multiple_of(16) {
        for dir in [&tiles, &shots].into_iter().flatten() {
            trim_dir(dir, PICTURES_KEPT);
        }
    }
    out
}

/// A path that is not a place one went to (signing in or out, paying).
fn passing_path(url: &str) -> bool {
    let path = url.split_once("://").map_or("", |(_, r)| r.find('/').map_or("", |i| &r[i..])).to_lowercase();
    const WORDS: [&str; 10] = ["login", "signin", "sign-in", "signup", "sign-up", "logout", "oauth", "auth", "register", "checkout"];
    path.split(['/', '?', '#', '&', '=']).any(|seg| WORDS.iter().any(|w| seg == *w || seg.starts_with(&format!("{w}."))))
}

/// `title` without the app's own name at either end ("R2 | Cloudflare").
fn without_name(title: &str, name: &str) -> String {
    let (lower, n) = (title.to_lowercase(), name.to_lowercase());
    for sep in [" | ", " - ", " · ", " – ", " — ", ": "] {
        // (`get`: a letter whose lower case is longer leaves the title whole.)
        if let Some(cut) = lower.strip_suffix(&format!("{sep}{n}")).and_then(|c| title.get(..c.len())) {
            return cut.trim().to_owned();
        }
        if let Some(rest) = lower.starts_with(&format!("{n}{sep}")).then(|| title.get(n.len() + sep.len()..)).flatten() {
            return rest.trim().to_owned();
        }
    }
    title.trim().to_owned()
}

/// `pages` once each by `group` (a video, a title, a site): the newest of a
/// group stands for it, with the whole group's use (`pages` come newest first).
fn once_each(pages: impl Iterator<Item = (Visit, String)>, group: impl Fn(&Visit, &str) -> String) -> Vec<(Visit, String)> {
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<(Visit, String)> = Vec::new();
    for (v, title) in pages {
        match at.get(&group(&v, &title)) {
            Some(&i) => {
                out[i].0.score += v.score;
                // (A song heard and its page: the one kept wears the cover.)
                if out[i].0.pic.is_none() {
                    out[i].0.pic = v.pic;
                }
            }
            None => {
                at.insert(group(&v, &title), out.len());
                out.push((v, title));
            }
        }
    }
    out
}

/// Of a history, the pages of a webapp worth showing — of its `site` when
/// Golem knows it (its kinds of page: a video, a product), else any page of
/// its own host but its start, a sign-in or one titled only with its name —,
/// newest first, each once (a video or a title), at most `max`, with their
/// titles cleaned.
fn webapp_pages(visits: Vec<Visit>, site: Option<&Site>, start: &str, name: &str, max: usize) -> Vec<(Visit, String)> {
    // (A site Golem knows with no kind of page worth it: none, on purpose.)
    if site.is_some_and(|s| s.pages.is_empty()) {
        return Vec::new();
    }
    let host = origin(start).map(|(_, h)| h.to_lowercase()).unwrap_or_default();
    let start = start.trim_end_matches('/');
    let worth = |url: &str| match site {
        Some(site) => is_page(site, url),
        None => {
            let path = url.split_once("://").map_or("", |(_, r)| r.find('/').map_or("", |i| &r[i..]));
            origin(url).is_some_and(|(_, h)| h.to_lowercase() == host)
                && url.trim_end_matches('/') != start
                && path.trim_matches('/').split(['?', '#']).next().is_some_and(|p| !p.is_empty())
        }
    };
    let names: Vec<String> = [Some(name), site.map(|s| s.name)].into_iter().flatten().map(str::to_lowercase).collect();
    let pages = visits
        .into_iter()
        .filter(|v| worth(&v.url) && !passing_path(&v.url))
        .map(|v| {
            let title = without_name(&clean_title(site, &v.title), name);
            (v, title)
        })
        .filter(|(_, title)| !title.is_empty() && !names.contains(&title.to_lowercase()));
    let mut pages = once_each(pages, |v, title| video_id(&v.url).map_or_else(|| title.to_lowercase(), str::to_owned));
    pages.truncate(max);
    pages
}

/// How many pages a lone app's recents are chosen from.
const CANDIDATES: usize = 400;

/// What a webapp (named `name`, opening at `start`; of `site` when Golem
/// knows it) showed lately: from Seam's history, where every webapp lives —
/// in THE ORDER (`rank`, the owner's `picks` counted in), at most `max`.
pub(crate) fn webapp_recents(site: Option<&Site>, start: &str, name: &str, picks: &Picks, max: usize) -> Vec<Recent> {
    let (browser, k) = (seam(), kitchen());
    // (Seam's title only when it says more than the app's name: an app that
    // keeps "YouTube Music" over a song must not hide the history's name of it.)
    let names: Vec<String> = [Some(name), site.map(|s| s.name)].into_iter().flatten().map(str::to_lowercase).collect();
    let says = |t: &str| {
        let t = without_name(&clean_title(site, t), name).to_lowercase();
        !t.is_empty() && !names.contains(&t)
    };
    let pages = webapp_pages(retitled(history(&browser, 4000), &k, says), site, start, name, CANDIDATES);
    let pages = with_plays(pages, played(&k, site, start, crate::usage::now_secs()));
    pictured(&browser, &k, ranked_pages(pages, picks, max))
}

/// A history with each page's title as Seam saw it last, where it kept one
/// (an app that names the song after the history wrote "YouTube Music").
fn retitled(mut visits: Vec<Visit>, k: &Kitchen, says: impl Fn(&str) -> bool) -> Vec<Visit> {
    for v in &mut visits {
        if let Some(t) = k.title_of(&v.url).filter(|t| says(t)) {
            v.title = t;
        }
    }
    visits
}

/// A webapp's pages and what it played, newest first, once each (a video,
/// a title): a song heard and its page are one.
fn with_plays(pages: Vec<(Visit, String)>, plays: Vec<(Visit, String)>) -> Vec<(Visit, String)> {
    let mut all: Vec<(Visit, String)> = plays.into_iter().chain(pages).collect();
    all.sort_by(|a, b| b.0.last.total_cmp(&a.0.last));
    once_each(all.into_iter(), |v, title| video_id(&v.url).map_or_else(|| title.to_lowercase(), str::to_owned))
}

/// Hosts that are not a place one went to (sign-ins, this computer).
fn passing(host: &str) -> bool {
    host.starts_with("accounts.") || host.starts_with("login.") || host == "localhost" || host.starts_with("127.")
}

/// Of a history, the newest page of each site (with the whole site's use),
/// newest first, at most `max` — none of the sites in `skip` (patterns as a
/// site's hosts: the installed webapps', whose pages are theirs, not the
/// browser's).
fn browser_pages(visits: Vec<Visit>, skip: &[String], max: usize) -> Vec<(Visit, String)> {
    let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
    let pages = visits.into_iter().filter_map(|v| {
        let (_, host) = origin(&v.url)?;
        let title = clean_title(site_of_host(&host), &v.title);
        let mine = !host_is(&host, &skip) && !passing(&host) && !passing_path(&v.url);
        (mine && !title.is_empty()).then_some((v, title))
    });
    let mut pages = once_each(pages, |v, _| origin(&v.url).map(|(_, h)| h).unwrap_or_default());
    pages.truncate(max);
    pages
}

/// What a browser showed lately: a page a site but those in `skip`, in THE
/// ORDER (`rank`, the owner's `picks` counted in), at most `max`.
pub(crate) fn browser_recents(browser: &Browser, skip: &[String], picks: &Picks, max: usize) -> Vec<Recent> {
    // (Seam's: as Seam saw its pages; another browser's as it wrote them.)
    let k = if *browser == seam() { kitchen() } else { Kitchen::default() };
    let pages = browser_pages(retitled(history(browser, 3000), &k, |t| !t.trim().is_empty()), skip, CANDIDATES);
    pictured(browser, &k, ranked_pages(pages, picks, max))
}

/// The hosts a webapp's pages are on, as patterns: its site's when Golem
/// knows it, else its own.
pub(crate) fn webapp_hosts(site: Option<&Site>, start: &str) -> Vec<String> {
    match site {
        Some(site) => site.hosts.iter().map(|h| (*h).to_owned()).collect(),
        None => origin(start).map(|(_, h)| vec![h.to_lowercase()]).unwrap_or_default(),
    }
}

/// The recents read lately, by app (its desktop id), so typing over a lone
/// app does not read a history on every letter (`app_actions.rs` asks;
/// read off the loop).
#[derive(Debug, Default)]
pub(crate) struct Recents {
    pub got: HashMap<String, (Instant, Vec<Recent>)>,
    pub asking: HashSet<String>,
}

/// How long recents read stay good.
pub(crate) const FRESH: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_exactly_below_or_with_any_ending() {
        assert!(host_is("www.youtube.com", &["www.youtube.com"]));
        assert!(!host_is("music.youtube.com", &["www.youtube.com", "youtube.com"]));
        assert!(host_is("es.wikipedia.org", &["*.wikipedia.org"]));
        assert!(host_is("www.amazon.es", &["www.amazon.*"]));
        assert!(host_is("www.amazon.com.mx", &["www.amazon.*"]));
        assert!(!host_is("www.amazonia.org", &["www.amazon.*"]));
    }

    #[test]
    fn a_sites_recent_pages_are_its_kind_of_page() {
        let yt = site("youtube").unwrap();
        assert!(is_page(yt, "https://www.youtube.com/watch?v=abc"));
        assert!(!is_page(yt, "https://www.youtube.com/feed/subscriptions"));
        assert!(!is_page(yt, "https://music.youtube.com/watch?v=abc"));
        let az = site("amazon").unwrap();
        assert!(is_page(az, "https://www.amazon.es/Zapatillas-Running/dp/B0C1234/ref=x"));
        let gh = site("github").unwrap();
        assert!(is_page(gh, "https://github.com/maxpoww/waveview"));
        assert!(!is_page(gh, "https://github.com/"));
        assert!(!is_page(gh, "https://github.com/notifications"));
        assert!(!is_page(site("gmail").unwrap(), "https://mail.google.com/mail/u/0/#inbox"));
    }

    #[test]
    fn titles_lose_what_the_site_adds() {
        let yt = site("youtube");
        assert_eq!(clean_title(yt, "(3) Gatos graciosos - YouTube"), "Gatos graciosos");
        assert_eq!(clean_title(site("amazon"), "Amazon.com: Running Shoes : Clothing"), "Running Shoes : Clothing");
        assert_eq!(clean_title(site("github"), "maxpoww/waveview · GitHub"), "maxpoww/waveview");
        assert_eq!(clean_title(None, "  Plain page "), "Plain page");
    }

    #[test]
    fn a_search_is_asked_by_a_sites_word_then_the_words() {
        let (s, q) = asked("amazon zapatillas running").unwrap();
        assert_eq!((s.slug, q.as_str()), ("amazon", "zapatillas running"));
        assert_eq!(asked("YouTube  gatos").map(|(s, q)| (s.slug, q)), Some(("youtube", "gatos".into())));
        assert!(asked("amazon").is_none());
        assert!(asked("amazon ").is_none());
        assert!(asked("vlc media").is_none());
        let s = site("youtube").unwrap();
        assert_eq!(search_url(s, "gatos & perros").unwrap(), "https://www.youtube.com/results?search_query=gatos%20%26%20perros");
        let az = site("amazon").unwrap();
        assert_eq!(search_url_from(az, "x", Some("https://www.amazon.com.mx")).unwrap(), "https://www.amazon.com.mx/s?k=x");
        assert_eq!(search_url_from(az, "x", None).unwrap(), "https://www.amazon.com/s?k=x");
        let wiki = site("wikipedia").unwrap();
        assert!(search_url_from(wiki, "x", Some("https://www.wikipedia.org")).unwrap().ends_with(".wikipedia.org/w/index.php?search=x"));
    }

    #[test]
    fn links_open_on_the_webapps_own_address() {
        let yt = site("youtube").unwrap();
        assert_eq!(link_url(&yt.links[0], "https://www.youtube.com/"), "https://www.youtube.com/feed/subscriptions");
        let docs = site("google-docs").unwrap();
        assert_eq!(link_url(&docs.links[0], "https://docs.google.com"), "https://docs.new");
        assert_eq!(origin("https://www.amazon.es/dp/x?y=1"), Some(("https://www.amazon.es".into(), "www.amazon.es".into())));
    }

    #[test]
    fn every_sites_slug_is_one_and_its_symbols_exist() {
        let mut seen = HashSet::new();
        for s in SITES {
            assert!(seen.insert(s.slug), "{} twice", s.slug);
            for l in s.links {
                assert!(crate::app_actions::has_symbol(l.symbol), "{}'s {} has no symbol {}", s.slug, l.name, l.symbol);
            }
        }
        assert!(SITES.len() >= 40, "{} sites", SITES.len());
    }

    #[test]
    fn a_videos_id_is_read_from_its_page() {
        assert_eq!(video_id("https://www.youtube.com/watch?v=9d9PQ1hrhBk&t=10s"), Some("9d9PQ1hrhBk"));
        assert_eq!(video_id("https://www.youtube.com/watch?feature=x&v=9d9PQ1hrhBk"), Some("9d9PQ1hrhBk"));
        assert_eq!(video_id("https://www.youtube.com/shorts/9d9PQ1hrhBk"), Some("9d9PQ1hrhBk"));
        assert_eq!(video_id("https://music.youtube.com/watch?v=9d9PQ1hrhBk&list=x"), Some("9d9PQ1hrhBk"));
        assert_eq!(video_id("https://example.com/watch?v=9d9PQ1hrhBk"), None);
        assert_eq!(video_id("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(picture_rank("hq720_custom_3.jpg?sqp=x"), 4);
        assert_eq!(picture_rank("mqdefault.jpg"), 3);
        assert_eq!(picture_rank("maxresdefault.jpg"), 0);
    }

    fn visit(url: &str, title: &str) -> Visit {
        Visit { url: url.into(), title: title.into(), last: 0.0, score: 0.0, pic: None }
    }

    #[test]
    fn the_newest_three_come_first_then_the_most_in_use() {
        // (what, when last opened, how much in use)
        let items = vec![
            ("daily", 10.0, 9.0),
            ("this morning", 90.0, 1.0),
            ("yesterday", 70.0, 1.0),
            ("an hour ago", 95.0, 0.5),
            ("weekly", 20.0, 3.0),
            ("once, long ago", 5.0, 0.1),
            ("once, last week", 30.0, 0.1),
        ];
        assert_eq!(
            rank(items, 6),
            ["an hour ago", "this morning", "yesterday", "daily", "weekly", "once, last week"]
        );
        // A pick from the box counts more than a visit, and as the newest.
        let pages = vec![
            (Visit { url: "a".into(), title: "A".into(), last: 50.0, score: 2.0, pic: None }, "A".to_string()),
            (Visit { url: "b".into(), title: "B".into(), last: 40.0, score: 1.0, pic: None }, "B".to_string()),
        ];
        let picks: Picks = [("b".to_string(), (1.0, 99.0))].into();
        let got = ranked_pages(pages, &picks, 2);
        assert_eq!(got.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["B", "A"]);
    }

    #[test]
    fn a_webapp_golem_does_not_know_shows_its_own_pages_only() {
        let history = vec![
            visit("https://dash.cloudflare.com/abc/r2/overview", "R2 Object Storage | Cloudflare"),
            visit("https://dash.cloudflare.com/abc/r2/overview", "R2 Object Storage | Cloudflare"),
            visit("https://dash.cloudflare.com/login", "Cloudflare Dashboard"),
            visit("https://dash.cloudflare.com/oauth2/auth?x=1", ""),
            visit("https://dash.cloudflare.com/", "Cloudflare | Web Performance"),
            visit("https://www.youtube.com/watch?v=9d9PQ1hrhBk", "A video - YouTube"),
            visit("https://dash.cloudflare.com/abc/home", "Account home"),
        ];
        let got = webapp_pages(history, None, "https://dash.cloudflare.com", "Cloudflare", 8);
        let titles: Vec<&str> = got.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(titles, ["R2 Object Storage", "Account home"]);
        // Pages titled only with the app's name say nothing: none.
        let keep = vec![
            visit("https://keep.google.com/", "Google Keep"),
            visit("https://keep.google.com/#NOTE/1idy", "Google Keep"),
        ];
        assert!(webapp_pages(keep, None, "https://keep.google.com", "Google Keep", 8).is_empty());
        // A site Golem knows with no kind of page worth it: none, on purpose.
        let mail = vec![visit("https://mail.google.com/mail/u/0/#inbox/abc", "Hello - Gmail")];
        assert!(webapp_pages(mail, site("gmail"), "https://mail.google.com", "Gmail", 8).is_empty());
    }

    #[test]
    fn a_browser_leaves_out_the_installed_webapps_sites() {
        let history = vec![
            visit("https://www.youtube.com/watch?v=9d9PQ1hrhBk", "A video - YouTube"),
            visit("https://m.youtube.com/", "YouTube"),
            visit("https://www.nytimes.com/a", "News"),
            visit("https://keep.google.com/", "Google Keep"),
            visit("https://www.nytimes.com/b", "Older news"),
            visit("https://accounts.google.com/signin", "Sign in"),
        ];
        let mut skip = webapp_hosts(site("youtube"), "https://www.youtube.com");
        skip.extend(webapp_hosts(None, "https://keep.google.com"));
        let got = browser_pages(history, &skip, 8);
        assert_eq!(got.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["News"]);
        assert_eq!(without_name("Cloudflare - R2", "cloudflare"), "R2");
        assert!(passing_path("https://x.com/i/flow/login") && !passing_path("https://x.com/author/alice"));
    }

    #[test]
    fn what_seam_cooked_is_read_as_data() {
        let dir = scratch("kitchen");
        std::fs::create_dir_all(dir.join("pics")).unwrap();
        std::fs::write(dir.join("pics/0123456789abcdef.png"), b"png").unwrap();
        std::fs::write(dir.join("pics/fedcba9876543210.jpg"), vec![0u8; (KITCHEN_PIC_MAX + 1) as usize]).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.join("pics/aaaaaaaaaaaaaaaa.png")).unwrap();
        std::fs::write(
            dir.join("recents.json"),
            r#"{"v":1,"pages":{"https://music.youtube.com/watch?v=9d9PQ1hrhBk":{"title":"Robot Rock - YouTube Music","at":5,"pic":"0123456789abcdef.png"}},
               "plays":[{"host":"open.spotify.com","url":"https://open.spotify.com/search/linkin","title":"In the End","artist":"Linkin Park","at":20,"n":2,"pic":"0123456789abcdef.png"},
                        {"host":"music.youtube.com","url":"https://music.youtube.com/watch?v=9d9PQ1hrhBk","title":"Robot Rock","artist":"Daft Punk","at":10,"n":1},
                        {"host":"evil.example","url":"javascript:alert(1)","title":"x","artist":"","at":30,"n":1}]}"#,
        )
        .unwrap();
        let k = kitchen_in(&dir);
        assert_eq!(k.title_of("https://music.youtube.com/watch?v=9d9PQ1hrhBk").as_deref(), Some("Robot Rock - YouTube Music"));
        assert!(k.picture("0123456789abcdef.png").is_some());
        // One page wears it alone: its own.
        assert!(k.page_picture("https://music.youtube.com/watch?v=9d9PQ1hrhBk").is_some());
        assert!(k.picture("fedcba9876543210.jpg").is_none(), "too big");
        assert!(k.picture("aaaaaaaaaaaaaaaa.png").is_none(), "a link");
        assert!(k.picture("../../etc/passwd").is_none() && k.picture("0123456789abcdef.svg").is_none());
        // Spotify played from a search page: a search of Spotify for the song, named with its artist.
        let sp = played(&k, site("spotify"), "https://open.spotify.com", 20.0);
        assert_eq!(sp.len(), 1);
        assert_eq!(sp[0].1, "In the End • Linkin Park");
        assert_eq!(sp[0].0.url, "https://open.spotify.com/search/In%20the%20End%20Linkin%20Park");
        assert!(sp[0].0.pic.is_some());
        // A YouTube play heard away from its video (a mini player on the home page): none.
        let mut away = k.clone();
        away.plays = vec![KitchenPlay { host: "www.youtube.com".into(), url: "https://www.youtube.com/".into(), title: "x".into(), ..Default::default() }];
        assert!(played(&away, site("youtube"), "https://www.youtube.com", 20.0).is_empty());
        // YouTube Music played on its own page: that page; one with it in the history.
        let ym = played(&k, site("youtube-music"), "https://music.youtube.com", 20.0);
        assert_eq!(ym[0].0.url, "https://music.youtube.com/watch?v=9d9PQ1hrhBk");
        let page = (Visit { url: "https://music.youtube.com/watch?v=9d9PQ1hrhBk".into(), title: "Robot Rock".into(), last: 5.0, score: 1.0, pic: None }, "Robot Rock".to_string());
        let both = with_plays(vec![page], ym);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].1, "Robot Rock • Daft Punk");
        // Not a file of its own, or not ours at all: nothing.
        assert_eq!(kitchen_in(&dir.join("nowhere")).plays.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_several_pages_wear_is_none_of_theirs() {
        let dir = scratch("shared-pic");
        std::fs::create_dir_all(dir.join("pics")).unwrap();
        std::fs::write(dir.join("pics/0123456789abcdef.png"), b"png").unwrap();
        std::fs::write(dir.join("pics/fedcba9876543210.png"), b"png").unwrap();
        std::fs::write(
            dir.join("recents.json"),
            r#"{"v":1,"pages":{"https://www.youtube.com/":{"title":"YouTube","pic":"0123456789abcdef.png"},
                              "https://www.youtube.com/watch?v=a":{"title":"A","pic":"0123456789abcdef.png"},
                              "https://www.youtube.com/watch?v=b":{"title":"B","pic":"fedcba9876543210.png"}},"plays":[]}"#,
        )
        .unwrap();
        let k = kitchen_in(&dir);
        assert!(k.page_picture("https://www.youtube.com/watch?v=a").is_none(), "the first page's picture on a later page");
        assert!(k.page_picture("https://www.youtube.com/watch?v=b").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readings_at_once_each_read_a_copy_of_their_own_and_leave_none() {
        let dir = scratch("together");
        let places = rusqlite::Connection::open(dir.join("places.sqlite")).unwrap();
        places.execute_batch("CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, last_visit_date INTEGER);").unwrap();
        for i in 0..300 {
            places
                .execute("INSERT INTO moz_places VALUES (?1, ?2, 't', ?3)", rusqlite::params![i, format!("https://x.test/{i}"), i])
                .unwrap();
        }
        drop(places);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || (0..10).map(|_| history(&Browser::Firefox(d.clone()), 1000).len()).collect::<Vec<_>>())
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap().iter().all(|&n| n == 300), "a reading saw another's copy");
        }
        let tag = hash_of(dir.join("places.sqlite").as_os_str().as_encoded_bytes());
        let left = std::fs::read_dir(cache("history").unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&tag)).count();
        assert_eq!(left, 0, "copies left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_claiming_a_huge_size_is_not_decoded() {
        // A PNG's header saying 60000 × 60000 (its data never comes).
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let ihdr = [&60000u32.to_be_bytes()[..], &60000u32.to_be_bytes()[..], &[8, 6, 0, 0, 0]].concat();
        png.extend_from_slice(&(ihdr.len() as u32).to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&[0, 0, 0, 0]);
        assert!(decode(&png).is_none());
        let mut ok = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])).write_to(&mut std::io::Cursor::new(&mut ok), image::ImageFormat::Png).unwrap();
        assert!(decode(&ok).is_some());
    }

    #[test]
    fn a_picture_cache_is_kept_under_its_bound() {
        let dir = scratch("trim");
        for i in 0..20 {
            std::fs::write(dir.join(format!("{i}.png")), b"x").unwrap();
        }
        trim_dir(&dir, 30);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 20);
        trim_dir(&dir, 16);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 12);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seams_title_wins_only_when_it_says_more_than_the_apps_name() {
        let dir = scratch("retitle");
        std::fs::write(
            dir.join("recents.json"),
            r#"{"v":1,"pages":{"https://music.youtube.com/watch?v=a":{"title":"YouTube Music"},"https://music.youtube.com/watch?v=b":{"title":"Robot Rock - YouTube Music"}},"plays":[]}"#,
        )
        .unwrap();
        let k = kitchen_in(&dir);
        let visits = vec![
            visit("https://music.youtube.com/watch?v=a", "Song A | YouTube Music"),
            visit("https://music.youtube.com/watch?v=b", "YouTube Music"),
        ];
        let says = |t: &str| {
            let t = without_name(&clean_title(site("youtube-music"), t), "YouTube Music").to_lowercase();
            !t.is_empty() && t != "youtube music"
        };
        let got = retitled(visits, &k, says);
        assert_eq!(got[0].title, "Song A | YouTube Music", "a generic title of Seam's hid the history's");
        assert_eq!(got[1].title, "Robot Rock - YouTube Music");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wr-web-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_cache_entrys_key_and_body_are_found() {
        let dir = scratch("cache");
        let body = b"\xFF\xD8picture bytes";
        let mut file = body.to_vec();
        file.extend_from_slice(&[0, 0, 0, 7, 1, 2]);
        file.extend_from_slice(b"O^partitionKey=%28https%2Cyoutube.com%29,a,:https://i.ytimg.com/vi/9d9PQ1hrhBk/mqdefault.jpg\0");
        file.extend_from_slice(b"response-head\0HTTP/2 200\0");
        file.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let entry = dir.join("ENTRY");
        std::fs::write(&entry, &file).unwrap();
        let (key, len) = cache_key(&entry).unwrap();
        assert_eq!(key, "https://i.ytimg.com/vi/9d9PQ1hrhBk/mqdefault.jpg");
        assert_eq!(len, body.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_favicon_becomes_a_tile() {
        let dir = scratch("tile");
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(32, 32, image::Rgba([200, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let tile = image::open(tile_in(&dir, &png).unwrap()).unwrap().to_rgba8();
        let side = crate::apps::ICON_SIZE;
        assert_eq!((tile.width(), tile.height()), (side, side));
        assert_eq!(tile.get_pixel(side / 2, side / 2).0, [200, 0, 0, 255]);
        assert_eq!(tile.get_pixel(4, 4).0, TILE_GROUND);
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" fill="#00f"/></svg>"##;
        let tile = image::open(tile_in(&dir, svg).unwrap()).unwrap().to_rgba8();
        assert_eq!(tile.get_pixel(side / 2, side / 2).0, [0, 0, 255, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the rows would show from this computer's own histories:
    /// `cargo test -- --ignored real_recents --nocapture`.
    #[test]
    #[ignore]
    fn real_recents() {
        for slug in ["youtube", "claude", "amazon", "github", "google-docs", "chatgpt"] {
            let at = Instant::now();
            let s = site(slug).unwrap();
            let got = webapp_recents(Some(s), &format!("https://{}", s.hosts[0]), s.name, &Picks::new(), 8);
            println!("{slug}: {} in {:?}", got.len(), at.elapsed());
            for r in got {
                println!("   {:60} {}", r.title.chars().take(60).collect::<String>(), r.picture.map(|p| p.display().to_string()).unwrap_or_default());
            }
        }
        for (name, start) in [("Google Keep", "https://keep.google.com"), ("Cloudflare", "https://dash.cloudflare.com")] {
            let got = webapp_recents(None, start, name, &Picks::new(), 8);
            println!("{name}: {}", got.len());
            for r in got {
                println!("   {:60} {}", r.title.chars().take(60).collect::<String>(), r.url);
            }
        }
        // The installed webapps' sites, as the dock leaves them out of Seam's.
        let home = crate::filer::home();
        let installed: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(home.join(".local/share/waverunner/managed-webapps.json")).unwrap_or_default())
                .unwrap_or_default();
        let skip: Vec<String> = installed
            .iter()
            .filter_map(|slug| {
                let text = std::fs::read_to_string(home.join(format!(".local/share/applications/webapp-{slug}.desktop"))).ok()?;
                let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?.to_owned();
                let start = crate::webapps::exec_app_url(&exec)?.to_owned();
                Some(webapp_hosts(site(slug).or_else(|| site_of_host(&origin(&start)?.1)), &start))
            })
            .flatten()
            .collect();
        for prog in ["seam", "google-chrome-stable", "firefox"] {
            let Some(b) = browser_for(prog) else { continue };
            let at = Instant::now();
            let got = browser_recents(&b, if prog == "seam" { &skip } else { &[] }, &Picks::new(), 8);
            println!("{prog}: {} in {:?}", got.len(), at.elapsed());
            for r in got {
                println!("   {:60} {}", r.title.chars().take(60).collect::<String>(), r.url);
            }
        }
    }

    #[test]
    fn a_firefox_and_a_chromium_history_are_read() {
        let dir = scratch("history");
        let places = rusqlite::Connection::open(dir.join("places.sqlite")).unwrap();
        places
            .execute_batch(
                "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, last_visit_date INTEGER);
                 INSERT INTO moz_places VALUES (1, 'https://www.youtube.com/watch?v=a', 'Old - YouTube', 10);
                 INSERT INTO moz_places VALUES (2, 'https://www.youtube.com/watch?v=b', 'New - YouTube', 20);
                 INSERT INTO moz_places VALUES (3, 'https://www.youtube.com/feed/subscriptions', NULL, 30);
                 INSERT INTO moz_places VALUES (4, 'about:blank', 'x', 40);",
            )
            .unwrap();
        drop(places);
        let got = history(&Browser::Firefox(dir.clone()), 10);
        assert_eq!(got.iter().map(|v| v.title.as_str()).collect::<Vec<_>>(), ["", "New - YouTube", "Old - YouTube"]);
        let chrome = rusqlite::Connection::open(dir.join("History")).unwrap();
        chrome
            .execute_batch(
                "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, last_visit_time INTEGER);
                 INSERT INTO urls VALUES (1, 'https://github.com/a/b', 'a/b · GitHub', 5);
                 INSERT INTO urls VALUES (2, 'https://www.reddit.com/r/x/comments/1/y', 'y : reddit', 9);",
            )
            .unwrap();
        drop(chrome);
        let got = history(&Browser::Chromium(dir.clone()), 10);
        assert_eq!(got.iter().map(|v| v.url.as_str()).collect::<Vec<_>>(), ["https://www.reddit.com/r/x/comments/1/y", "https://github.com/a/b"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
