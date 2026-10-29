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
    /// What its titles carry at their ends that is not the page's own (a
    /// last `*`: any one word there — " : r/*" is " : r/linux").
    pub strip: &'static [&'static str],
    /// Its search's address, `{q}` the words (`{lang}` the shell's language).
    pub search: Option<&'static str>,
    /// What typed first asks it to search ("youtube gatos") — never a word
    /// said every day, a letter or a name: "so long", "x men", "g major",
    /// "ali" asked Stack Overflow, X, Google, AliExpress, and Enter went
    /// there (2026-10-29). Its webapp's name, Shift-written, searches it.
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
        words: &["google"],
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
        pages: &["/groups/", "/marketplace/item/", "/events/", "/posts/", "/videos/", "/reel/"],
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
        pages: &["/p/", "/reel/", "/reels/"],
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
        words: &["twitter"],
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
        strip: &[" : reddit", " - Reddit", " : r/*"],
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
        slug: "google-keep",
        name: "Google Keep",
        hosts: &["keep.google.com"],
        links: &[
            link("New note", "https://keep.new", "plus"),
            link("Reminders", "/#reminders", "bell"),
            link("Archived", "/#archive", "package"),
            link("Bin", "/#trash", "trash"),
        ],
        // (Every page of it is titled "Google Keep": no recents.)
        pages: &[],
        strip: &[],
        search: Some("https://keep.google.com/#search/text%253D{q}"),
        // ("keep" is a word said every day.)
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
        words: &["aliexpress"],
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
        words: &["stackoverflow"],
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
    // (One of its known pages is an action of its row, not a recent: Discord's
    // Direct messages, YouTube Music's Liked music.)
    if site.links.iter().any(|l| is_link(l, path)) {
        return false;
    }
    // (A wiki's own pages — its searches, its random article, its front page
    // — are none of its articles.)
    if site.slug == "wikipedia" && path.strip_prefix("/wiki/").is_some_and(|t| WIKI_OWN.iter().any(|w| t.starts_with(w))) {
        return false;
    }
    if site.pages == ["/"] {
        // Any page but the first, and none of the site's own known pages.
        let bare = path.split(['?', '#']).next().unwrap_or("");
        return bare.len() > 1 && !site.links.iter().any(|l| l.url.starts_with(bare));
    }
    site.pages.iter().any(|p| path.contains(p))
}

/// What begins a Wikipedia page's title when the page is the wiki's own, not
/// an article: its namespaces for searches, the random article and the like
/// (in the languages it is read in most), and its front pages.
const WIKI_OWN: [&str; 10] = [
    "Special:", "Especial:", "Spezial:", "Sp%C3%A9cial:", "Speciale:", "Wikipedia:", "Portal:", "Main_Page", "Portada", "Accueil_principal",
];

/// The picture viewer over a wiki's article (`#/media/File:…`): the article,
/// though its title names the picture.
fn media_view(url: &str) -> bool {
    url.contains("#/media/")
}

/// Whether a page at `path` (from the site's address on) is `link`'s.
fn is_link(link: &Link, path: &str) -> bool {
    let own = match link.url.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |i| &rest[i..]),
        None => link.url,
    };
    let own = own.trim_end_matches('/');
    !own.is_empty() && own == path.trim_end_matches('/')
}

/// A page's title, without what the site adds to every one.
pub(crate) fn clean_title(site: Option<&Site>, title: &str) -> String {
    // (Marks that show nothing — the left-to-right one before "Apple Music" —
    // gone, and a run of spaces one.)
    let shown: String = title.chars().filter(|c| !INVISIBLE.contains(c)).collect();
    let mut t = shown.split_whitespace().collect::<Vec<_>>().join(" ");
    // A count of what is new, "(3) Inbox": not the page's.
    if t.starts_with('(') {
        if let Some(end) = t.find(") ") {
            if t[1..end].chars().all(|c| c.is_ascii_digit() || c == '+') {
                t = t[end + 2..].to_owned();
            }
        }
    }
    for s in site.map_or(&[][..], |s| s.strip) {
        if let Some(head) = s.strip_suffix('*') {
            if let Some(i) = t.rfind(head) {
                let word = &t[i + head.len()..];
                if !word.is_empty() && !word.contains(char::is_whitespace) {
                    t = t[..i].trim().to_owned();
                }
            }
            continue;
        }
        if let Some(rest) = t.strip_suffix(s) {
            t = rest.trim().to_owned();
        }
        if let Some(rest) = t.strip_prefix(s) {
            t = rest.trim().to_owned();
        }
    }
    t
}

/// Marks a title can carry that show nothing (direction marks and
/// embeddings, a zero-width space, a word joiner, a soft hyphen; not the
/// joiner of an emoji's parts).
const INVISIBLE: [char; 15] = [
    '\u{200B}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2060}', '\u{2066}', '\u{2067}',
    '\u{2068}', '\u{2069}', '\u{FEFF}', '\u{00AD}',
];

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
    /// How many times it was opened in the last weeks (a play: played).
    pub times: u32,
    /// Something Seam heard PLAY (a song, a video), not a page looked at.
    pub played: bool,
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
    history_with(browser, max, &[]).0
}

/// A browser's newest pages, newest first (`max` read), and which of
/// `ask` (addresses) it has at all — in one reading (none: it could not
/// be read).
fn history_with(browser: &Browser, max: usize, ask: &[String]) -> (Vec<Visit>, Option<HashSet<String>>) {
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
        return (Vec::new(), None);
    };
    let table = match browser {
        Browser::Firefox(_) => "moz_places",
        Browser::Chromium(_) => "urls",
    };
    let mut has = Some(HashSet::new());
    for some in ask.chunks(400) {
        let sql = format!("SELECT url FROM {table} WHERE url IN ({})", vec!["?"; some.len()].join(","));
        let read = conn.prepare(&sql).and_then(|mut stmt| {
            stmt.query_map(rusqlite::params_from_iter(some), |r| r.get::<_, String>(0)).map(|rows| rows.collect::<Result<Vec<_>, _>>())?
        });
        match (read, has.as_mut()) {
            (Ok(urls), Some(has)) => has.extend(urls),
            _ => has = None,
        }
    }
    let secs = |t: i64| t as f64 / 1e6 - epoch;
    let now = crate::usage::now_secs();
    // The visits of the last weeks, each worth what is left of it (older
    // ones are worth next to nothing); a history without them: none.
    let mut score: HashMap<i64, (f64, u32)> = HashMap::new();
    let since = ((now - 30.0 * 24.0 * 3600.0 + epoch) * 1e6) as i64;
    if let Ok(mut stmt) = conn.prepare(visits) {
        if let Ok(rows) = stmt.query_map([since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))) {
            for (id, at) in rows.flatten() {
                let s = score.entry(id).or_default();
                s.0 += crate::usage::decayed(now - secs(at));
                s.1 += 1;
            }
        }
    }
    let Ok(mut stmt) = conn.prepare(pages) else {
        return (Vec::new(), None);
    };
    let rows = stmt.query_map([max as i64], |r| {
        let id: i64 = r.get(0)?;
        let (score, times) = score.get(&id).copied().unwrap_or((0.0, 0));
        Ok(Visit {
            url: r.get(1)?,
            title: r.get(2)?,
            last: secs(r.get::<_, Option<i64>>(3)?.unwrap_or(0)),
            score,
            pic: None,
            times,
            played: false,
        })
    });
    (rows.map(|rows| rows.flatten().collect()).unwrap_or_default(), has)
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
        Browser::Chromium(_) => {
            // (Its own, else its site's: any page of its origin's.)
            let of = |test: &str, page: String| {
                let sql = format!(
                    "SELECT b.image_data FROM icon_mapping m JOIN favicon_bitmaps b ON b.icon_id = m.icon_id \
                     WHERE m.page_url {test} ORDER BY b.width DESC LIMIT 1"
                );
                conn.query_row(&sql, [page], |r| r.get(0)).ok()
            };
            let like = |s: &str| s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            of("= ?1", url.to_owned()).or_else(|| of("LIKE ?1 ESCAPE '\\'", format!("{}/%", like(&origin(url)?.0))))
        }
    }
}

/// A page that has no favicon: a sheet with its corner turned.
const SHEET: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64" fill="none" stroke="#8c8c8c" stroke-width="3" stroke-linejoin="round"><path d="M16 6h22l14 14v38H16z"/><path d="M38 6v14h14"/></svg>"##;

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

/// A YouTube list's id, read from the list's own page (`/playlist?list=<id>`;
/// a video played from a list is the video's page).
fn list_id(url: &str) -> Option<&str> {
    let (_, host) = origin(url)?;
    if !host.ends_with("youtube.com") || video_id(url).is_some() {
        return None;
    }
    let rest = url.split_once("://")?.1;
    let query = rest.get(rest.find('/')?..)?.strip_prefix("/playlist?")?;
    let id = query.split(['&', '#']).find_map(|kv| kv.strip_prefix("list="))?;
    (!id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then_some(id)
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
/// ours, in its own shape (`fitted`).
fn video_picture(dir: &Path, id: &str, entry: &Path, body: u64) -> Option<PathBuf> {
    let path = dir.join(format!("{id}-f4.png"));
    if path.exists() {
        return Some(path);
    }
    let bytes = std::fs::read(entry).ok()?;
    fitted(bytes.get(..body as usize)?, &path, true)
}

/// The longest side a recent's picture is kept at.
const PICTURE_SIDE: u32 = 512;

/// A picture WHOLE, in its own shape (Max, 2026-10-23: "más rectangulares,
/// la miniatura entera, forma libre — las de YouTube Music son cuadradas,
/// las de YouTube rectángulos"), at most `PICTURE_SIDE`, written to `path`
/// (a PNG) — only what padded it cut away (`unbanded`; a `video`'s also
/// the bars round a song's cover, and that cut WHOLE beside it for Quick
/// Look when it is bigger than the box's — `big_of`).
fn fitted(bytes: &[u8], path: &Path, video: bool) -> Option<PathBuf> {
    let img = decode(bytes)?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let img = match unbanded(&img, video) {
        Some(cut) => {
            if video && cut.width().max(cut.height()) > PICTURE_SIDE {
                let big = big_of(path);
                let tmp = big.with_extension("part");
                if cut.to_rgba8().save_with_format(&tmp, image::ImageFormat::Png).is_ok() {
                    let _ = std::fs::rename(&tmp, &big);
                }
            }
            cut
        }
        None => img,
    };
    let img = if img.width().max(img.height()) > PICTURE_SIDE {
        img.resize(PICTURE_SIDE, PICTURE_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let tmp = path.with_extension("part");
    img.to_rgba8().save_with_format(&tmp, image::ImageFormat::Png).ok()?;
    std::fs::rename(&tmp, path).ok()?;
    Some(path.to_owned())
}

/// A recent's picture AS BIG AS IT WAS FOUND, for Quick Look (Max,
/// 2026-10-27: "the quick look is small"): the kitchen's own file, or a
/// video's from Seam's cache written out once — only when it is bigger than
/// the box's (`PICTURE_SIDE`), within `decode`'s bounds, and not a 4:3 one
/// with black bands (the box's cut is the better one); else none.
pub(crate) fn big_picture(src: &Path, body: u64) -> Option<PathBuf> {
    let path = if body == 0 {
        src.to_owned()
    } else {
        let bytes = std::fs::read(src).ok()?;
        let bytes = bytes.get(..usize::try_from(body).ok()?)?;
        let ext = match bytes {
            [0xff, 0xd8, ..] => "jpg",
            [0x89, b'P', b'N', b'G', ..] => "png",
            [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
            _ => return None,
        };
        let out = cache("web-pictures")?.join(format!("big-{}.{ext}", hash_of(src.to_string_lossy().as_bytes())));
        if !out.exists() {
            let tmp = out.with_extension("part");
            std::fs::write(&tmp, bytes).ok()?;
            std::fs::rename(&tmp, &out).ok()?;
        }
        out
    };
    let (w, h) = image::image_dimensions(&path).ok()?;
    let banded = w * 3 == h * 4 && video_id_of_cache(src);
    (w.max(h) > PICTURE_SIDE && w <= 4096 && h <= 4096 && !banded).then_some(path)
}

/// Whether a picture came out of Seam's cache (a video's: YouTube's 4:3
/// ones carry black bands).
fn video_id_of_cache(src: &Path) -> bool {
    src.components().any(|c| c.as_os_str() == "cache2")
}

/// When a recent was seen, as said under its title in Quick Look: "just
/// now", "12 min ago", "today at 13:02", "yesterday at 21:04", "3 days
/// ago", "2 weeks ago", "4 months ago".
pub(crate) fn seen_when(at: f64, now: f64) -> String {
    use crate::i18n::{fill, tr};
    if at <= 0.0 {
        return String::new();
    }
    let ago = (now - at).max(0.0);
    if ago < 90.0 {
        return tr("just now").to_owned();
    }
    if ago < 3600.0 {
        return fill(tr("{n} min ago"), &[("n", &((ago / 60.0).round() as u64))]);
    }
    let local = |secs: f64| -> Option<libc::tm> {
        let secs = secs as libc::time_t;
        // SAFETY: `localtime_r` fills a caller-owned `tm` from a valid `time_t`.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            (!libc::localtime_r(&secs, &mut tm).is_null()).then_some(tm)
        }
    };
    let (Some(then), Some(today)) = (local(at), local(now)) else {
        return String::new();
    };
    let hm = format!("{:02}:{:02}", then.tm_hour, then.tm_min);
    let midnight = |secs: f64, tm: &libc::tm| secs - f64::from(tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec);
    let days = ((midnight(now, &today) - midnight(at, &then)) / 86_400.0).round() as i64;
    match days {
        ..=0 => fill(tr("today at {time}"), &[("time", &hm)]),
        1 => fill(tr("yesterday at {time}"), &[("time", &hm)]),
        2..=13 => fill(tr("{n} days ago"), &[("n", &days)]),
        14..=59 => fill(tr("{n} weeks ago"), &[("n", &(days / 7))]),
        _ => fill(tr("{n} months ago"), &[("n", &(days / 30))]),
    }
}

/// What Quick Look says of a recent under its title: how it was seen and
/// when, and how often ("Watched today at 13:02 · 5 times").
pub(crate) fn seen_line(r: &Recent, now: f64) -> String {
    use crate::i18n::{fill, tr};
    let when = seen_when(r.last, now);
    // (A music site's page is a song played, as what Seam heard play.)
    let music = origin(&r.url).and_then(|(_, h)| site_of_host(&h.to_lowercase())).is_some_and(|s| MUSIC.contains(&s.slug));
    let mut line = if when.is_empty() {
        String::new()
    } else if r.played || music {
        fill(tr("Played {when}"), &[("when", &when)])
    } else if video_id(&r.url).is_some() {
        fill(tr("Watched {when}"), &[("when", &when)])
    } else {
        fill(tr("Visited {when}"), &[("when", &when)])
    };
    if r.times > 1 {
        let times = fill(tr("{n} times"), &[("n", &r.times)]);
        line = if line.is_empty() { times } else { format!("{line} · {times}") };
    }
    line
}

/// A picture without what YouTube padded it with: a 16:9 one between two
/// black bands in a 4:3, and — `square`, a video's (Max, 2026-10-28: "fix
/// the covers too") — a song's square cover between two bars of one colour
/// (the cover's own: black, olive…) in a 16:9; cut only where the bands
/// are, never into the picture. None when nothing is cut.
fn unbanded(img: &image::DynamicImage, square: bool) -> Option<image::DynamicImage> {
    let (w, h) = (img.width(), img.height());
    let (mut y, mut ch) = (0, h);
    if w * 3 == h * 4 && banded(img) {
        y = (h - w * 9 / 16) / 2;
        ch = h - 2 * y;
    }
    let (mut x, mut cw) = (0, w);
    let bars = (w * 9).abs_diff(ch * 16) <= w / 64 && w > ch;
    if square && bars && (w - ch) / 2 > 16 {
        let rgb = img.to_rgb8();
        // (A bar: one colour all over, give or take what the JPEG did to it;
        // its edges by the cover and the bands left out — a JPEG block
        // smears them.)
        let bar = |x0: u32, x1: u32| -> Option<[u8; 3]> {
            let first = rgb.get_pixel(x0, y + ch / 2).0;
            let near = |p: [u8; 3]| p.iter().zip(first).all(|(&a, b)| a.abs_diff(b) <= 12);
            (x0..x1).step_by(3).all(|bx| (y + 8..y + ch - 8).step_by(3).all(|by| near(rgb.get_pixel(bx, by).0))).then_some(first)
        };
        let p = (w - ch) / 2;
        if let (Some(l), Some(r)) = (bar(0, p - 8), bar(w - p + 8, w)) {
            if l.iter().zip(r).all(|(&a, b)| a.abs_diff(b) <= 16) {
                (x, cw) = (p, w - 2 * p);
            }
        }
    }
    (x > 0 || y > 0).then(|| img.crop_imm(x, y, cw, ch))
}

/// Where `fitted` keeps the whole cut of the picture it wrote at `small`.
fn big_of(small: &Path) -> PathBuf {
    let stem = small.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    small.with_file_name(format!("{stem}-big.png"))
}

/// Whether a 4:3 picture is a 16:9 one between two black bands (rows of
/// both dark all across — looked at a JPEG block away from the picture:
/// its edge rings into the bands' nearest rows, 2026-10-28).
fn banded(img: &image::DynamicImage) -> bool {
    let (w, h) = (img.width(), img.height());
    let band = (h - w * 9 / 16) / 2;
    let rgb = img.to_rgb8();
    let dark = |y: u32| (0..w).step_by(4).all(|x| rgb.get_pixel(x, y).0.iter().all(|&c| c < 24));
    band > 10 && dark(1) && dark(band - 8) && dark(h - band + 8) && dark(h - 2)
}

/// A picture file's shape (width / height), read from its header.
fn shape_of(path: &Path) -> Option<f32> {
    let (w, h) = image::image_dimensions(path).ok()?;
    (w > 0 && h > 0).then(|| w as f32 / h as f32)
}

/// How many pixels a picture file has, read from its header (0: unreadable).
fn pixels(path: &Path) -> u64 {
    image::image_dimensions(path).map_or(0, |(w, h)| u64::from(w) * u64::from(h))
}

/// Of the picture Seam heard a recent play with and the one its video's page
/// kept (`page`, a video's page only), the one to show: what played — unless
/// the page's is bigger; it is the same video (YouTube Music's player says a
/// video by a 400×225 still, its page kept it at 1280×720).
fn heard_or_page(heard: Option<PathBuf>, page: Option<PathBuf>) -> Option<PathBuf> {
    match (heard, page) {
        (Some(h), Some(p)) if pixels(&p) > pixels(&h) => Some(p),
        (h, _) => h,
    }
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
    /// When Seam's history was last cleared whole (unix seconds; 0 never).
    #[serde(default)]
    cleared: f64,
    #[serde(skip)]
    dir: PathBuf,
    /// Pictures more than one page wears: a site's logo, or the first
    /// page's of an app that changes page without loading — no page's own
    /// (one video at several addresses, with a list or without, is one page).
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
            let mut seen: HashMap<String, String> = HashMap::new();
            for (url, page) in &k.pages {
                let Some(pic) = page.pic.clone() else {
                    continue;
                };
                // (One page's addresses — a post and its comments, a tracking
                // tag — wear its picture as one: `page_key`.)
                let page = match (video_id(url), list_id(url)) {
                    (Some(id), _) => id.to_owned(),
                    (None, Some(list)) => format!("list:{list}"),
                    (None, None) => page_key(origin(url).and_then(|(_, h)| site_of_host(&h)), url),
                };
                match seen.get(&pic) {
                    Some(other) if *other != page => {
                        k.shared.insert(pic);
                    }
                    Some(_) => {}
                    None => {
                        seen.insert(pic, page);
                    }
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
        // (A music site's song has an artist; what plays without one is an
        // advertisement: Spotify's "Advertisement".)
        .filter(|p| !music || !p.artist.trim().is_empty())
        // (A YouTube play away from its video's page was heard in a mini player on
        // another page: it would open that page, not the video.)
        .filter(|p| !p.host.ends_with("youtube.com") || video_id(&p.url).is_some())
        .map(|p| {
            let (song, artist): (String, String) =
                (clean_title(None, &p.title).chars().take(200).collect(), clean_title(None, &p.artist).chars().take(120).collect());
            let title = if music && !artist.is_empty() { format!("{song} • {artist}") } else { song.clone() };
            let own = site.is_some_and(|s| is_page(s, &p.url));
            let url = match site.filter(|_| !own).and_then(|s| search_url(s, &format!("{song} {artist}"))) {
                Some(search) => search,
                None => p.url.clone(),
            };
            let pic = p.pic.as_deref().and_then(|n| k.picture(n)).map(|p| p.to_string_lossy().into_owned());
            let visit = Visit {
                url,
                title: title.clone(),
                last: p.at,
                score: f64::from(p.n.max(1)) * crate::usage::decayed(now - p.at),
                pic,
                times: p.n.max(1),
                played: true,
            };
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
    /// The picture's own shape (width / height) when it is a preview to be
    /// shown whole — a video's, a cover, a page's; none for a favicon's tile.
    pub aspect: Option<f32>,
    /// When it was seen last (unix seconds; 0 unknown): one put away since
    /// is not shown (`Forgot`).
    pub last: f64,
    /// How many times it was opened in the last weeks (played, a play).
    pub times: u32,
    /// Something heard PLAY (a song, a video), not a page looked at.
    pub played: bool,
    /// Where its picture is AS BIG AS IT WAS FOUND (Quick Look shows that,
    /// `big_picture`): a file of Seam's kitchen, or a video's in Seam's
    /// cache (the entry and its body's length; 0: the whole file).
    pub big: Option<(PathBuf, u64)>,
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
            // (A video's page, or a list's — its picture is its first video's:
            // what padded its picture cut — `unbanded`.)
            let video_page = video_id(&v.url).is_some();
            let square = video_page || list_id(&v.url).is_some();
            let cooked = |src: PathBuf| {
                let cut = if square { "f4" } else { "f3" };
                let out = shots.as_deref()?.join(format!("k-{}-{cut}.png", src.file_stem()?.to_string_lossy()));
                if out.exists() {
                    return Some(out);
                }
                let bytes = std::fs::read(&src).ok()?;
                fitted(&bytes, &out, square)
            };
            let cached = video_id(&v.url).and_then(|id| Some((id, videos.get(id)?)));
            let video = || cached.and_then(|(id, (_, entry, body))| video_picture(shots.as_deref()?, id, entry, *body));
            let page = || kitchen.page_picture(&v.url);
            let heard = heard_or_page(v.pic.as_deref().map(PathBuf::from), video_page.then(page).flatten());
            let played = heard.clone().and_then(cooked);
            let (preview, big) = match played {
                Some(p) => (Some(p), heard.map(|h| (h, 0))),
                None => match video() {
                    Some(p) => (Some(p), cached.map(|(_, (_, entry, body))| (entry.clone(), *body))),
                    None => {
                        let src = page();
                        (src.clone().and_then(cooked), src.map(|s| (s, 0)))
                    }
                },
            };
            let big = big.filter(|_| preview.is_some());
            // (Cut, Quick Look shows the cut too.)
            let big = match preview.as_deref().map(big_of).filter(|b| b.exists()) {
                Some(cut) => Some((cut, 0)),
                None => big,
            };
            let aspect = preview.as_deref().and_then(shape_of);
            // (A page with no favicon of its own or its site's — a file shown
            // whole — wears a sheet: Quick Look has something to show it by.)
            let picture = preview
                .or_else(|| {
                    let data = favicon(browser, icons.as_deref()?, &v.url)?;
                    tile_in(tiles.as_deref()?, &data)
                })
                .or_else(|| tile_in(tiles.as_deref()?, SHEET.as_bytes()));
            Recent { url: v.url, title, picture, dir: false, aspect, last: v.last, times: v.times, played: v.played, big }
        })
        .collect();
    // (Now and then: the picture caches kept under their bound.)
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    if CALLS.fetch_add(1, Ordering::Relaxed).is_multiple_of(16) {
        for dir in [&tiles, &shots].into_iter().flatten() {
            trim_dir(dir, PICTURES_KEPT);
        }
        let cache2 = match browser {
            Browser::Firefox(p) if p.join("cache2/entries").is_dir() => Some(&videos),
            _ => None,
        };
        if let Some(shots) = &shots {
            sweep_unkept(shots, kitchen, cache2);
        }
    }
    out
}

/// The pictures made of Seam's that Seam keeps no more — a page or the
/// history forgotten (its kitchen's), its cache cleared (a video's) —
/// gone too: the dock keeps a copy only while Seam keeps what it was made
/// of (`videos`: Seam's cache's index, when it was read).
fn sweep_unkept(dir: &Path, kitchen: &Kitchen, videos: Option<&Pictures>) {
    let seam_has = |stem: &str| ["jpg", "png", "webp"].iter().any(|x| kitchen.dir.join("pics").join(format!("{stem}.{x}")).exists());
    let entries: Option<HashSet<String>> = videos.map(|v| v.values().map(|(_, e, _)| hash_of(e.to_string_lossy().as_bytes())).collect());
    let hex = |s: &str| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit());
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let kept = if let Some(stem) = name.strip_prefix("k-").and_then(|r| r.get(..16)).filter(|s| hex(s) && name.as_bytes().get(18) == Some(&b'-')) {
            // (No kitchen read: nothing to tell by.)
            kitchen.dir.as_os_str().is_empty() || seam_has(stem)
        } else if let Some(hash) = name.strip_prefix("big-").and_then(|r| r.get(..16)).filter(|s| hex(s)) {
            entries.as_ref().is_none_or(|e| e.contains(hash))
        } else if name.len() > 12 && name.as_bytes()[11] == b'-' && name[..11].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            videos.is_none_or(|v| v.contains_key(&name[..11]))
        } else {
            true
        };
        if !kept {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A path that is not a place one went to (signing in or out — GitHub's
/// "/sessions/…", a sign-in's "client_id=…" —, paying).
fn passing_path(url: &str) -> bool {
    let path = url.split_once("://").map_or("", |(_, r)| r.find('/').map_or("", |i| &r[i..])).to_lowercase();
    const WORDS: [&str; 24] = [
        "login", "signin", "sign-in", "signup", "sign-up", "logout", "oauth", "oauth2", "auth", "oidc", "sso", "saml", "session",
        "sessions", "2fa", "two-factor", "verify", "authorize", "consent", "register", "checkout", "client_id", "redirect_uri",
        "user_code",
    ];
    path.split(['/', '?', '#', '&', '=']).any(|seg| WORDS.iter().any(|w| seg == *w || seg.starts_with(&format!("{w}."))))
}

/// A page an app named before it kept it (ChatGPT's "/c/local-chatgpt:…"
/// while a chat is new): it opens nothing.
fn placeholder(url: &str) -> bool {
    url.split(['/', '?', '#']).any(|seg| seg.strip_prefix("local-").is_some_and(|r| r.contains(':') || r.to_lowercase().contains("%3a")))
}

/// Where a site's pages say which thing they are, the thing's id next
/// ("/comments/<id>" of a Reddit post, "/status/<id>" on X, "/dp/<id>" on
/// Amazon, "/d/<id>" of a Google document).
const ID_MARKS: [&str; 18] = [
    "/comments/", "/status/", "/dp/", "/gp/product/", "/itm/", "/video/", "/questions/", "/pin/", "/rooms/", "/title/", "/p/", "/reel/",
    "/reels/", "/c/", "/chat/", "/artifact/", "/d/", "/posts/",
];

/// What stands for ONE page whatever else its address says: a video, a
/// list, on a site Golem knows the thing after its kind (a Reddit post's
/// address, its comment's, its picture's open; a post on X with its photo
/// open), else the address without what only moves within the page
/// ("#lightbox", a wiki's "#/media/…" picture; a "#/route" or "#NOTE/…" is a
/// page of its own).
fn page_key(site: Option<&Site>, url: &str) -> String {
    if let Some(id) = video_id(url) {
        return format!("v:{id}");
    }
    if let Some(id) = list_id(url) {
        return format!("list:{id}");
    }
    let Some((host, path)) = url.split_once("://").and_then(|(_, r)| r.find('/').map(|i| (&r[..i], &r[i..]))) else {
        return url.to_owned();
    };
    let host = host.to_lowercase();
    if site.is_some() {
        let bare = path.split(['?', '#']).next().unwrap_or("");
        for mark in ID_MARKS {
            if let Some((_, rest)) = bare.split_once(mark) {
                let id = rest.split('/').next().unwrap_or("");
                if !id.is_empty() {
                    return format!("{host}{mark}{id}");
                }
            }
        }
    }
    let path = match path.split_once('#') {
        Some((page, within)) if !within.contains('/') || within.starts_with("/media/") => page,
        _ => path,
    };
    // (A tab of a page is that page: GitHub's "?tab=repositories".)
    let path = match path.split_once('?') {
        Some((page, query)) if site.is_some() => {
            let kept: Vec<&str> = query.split('&').filter(|kv| !kv.starts_with("tab=") && !kv.is_empty()).collect();
            if kept.is_empty() { page.to_owned() } else { format!("{page}?{}", kept.join("&")) }
        }
        _ => path.to_owned(),
    };
    format!("{host}{}", path.trim_end_matches('/'))
}

/// The words a page's address names it by — a Reddit post's
/// ("/comments/<id>/<words>"), a question's on Stack Overflow — when it has
/// some.
fn slug_words(url: &str) -> Option<Vec<String>> {
    let path = url.split_once("://")?.1.split(['?', '#']).next()?;
    let rest = ["/comments/", "/questions/"].iter().find_map(|m| path.split_once(m).map(|(_, r)| r))?;
    let words: Vec<String> = rest.split('/').nth(1)?.split(['_', '-']).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    (words.len() >= 2).then_some(words)
}

/// Whether `title` is the one of the page at `url` by the words its address
/// names it by (a title the app kept from the page before — Reddit's "Inbox"
/// over a post — says none of them).
fn fits_address(url: &str, title: &str) -> bool {
    let Some(words) = slug_words(url) else {
        return true;
    };
    let said: String = title.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    let long: Vec<&String> = words.iter().filter(|w| w.chars().count() >= 4).collect();
    long.is_empty() || long.iter().any(|w| said.contains(w.as_str()))
}

/// A page's name from its address's words ("What to work on outside of
/// work"), when no title it wore was its own.
fn address_title(url: &str) -> Option<String> {
    let words = slug_words(url)?.join(" ");
    let mut letters = words.chars();
    let first = letters.next()?;
    Some(first.to_uppercase().chain(letters).collect())
}

/// What every app calls its own pages — never a page's title of its own
/// ("Home", "Inbox", "Web Player" once its name is gone; "omarchy - Search").
fn chrome_word(title: &str) -> bool {
    const WORDS: [&str; 17] = [
        "home", "inicio", "inbox", "bandeja de entrada", "notifications", "notificaciones", "friends", "amigos", "explore", "explorar",
        "search", "buscar", "web player", "dashboard", "loading", "cargando", "messages",
    ];
    let t = title.trim().to_lowercase();
    WORDS.contains(&t.as_str()) || t.starts_with("redirecting") || t.ends_with(" - search") || t.ends_with(" - buscar")
}

/// The titles a webapp's OWN pages wore — its start, its known pages, its
/// search's, its sign-in's (cleaned, lower case): a page of an app that
/// changes page without loading may still wear the one of the page before
/// (Cloudflare's "Cloudflare Dashboard | Manage Your Account" of its sign-in).
fn chrome_titles(visits: &[Visit], site: Option<&Site>, start: &str, name: &str) -> HashSet<String> {
    let start = start.trim_end_matches('/');
    let path_of = |url: &str| url.split_once("://").map_or(String::new(), |(_, r)| r.find('/').map_or(String::new(), |i| r[i..].to_owned()));
    let search = site.and_then(|s| s.search).map(|s| path_of(s.split('{').next().unwrap_or("")));
    let host = origin(start).map(|(_, h)| h.to_lowercase()).unwrap_or_default();
    visits
        .iter()
        .filter(|v| {
            let on = match site {
                Some(s) => origin(&v.url).is_some_and(|(_, h)| host_is(&h, s.hosts)) && !is_page(s, &v.url),
                None => origin(&v.url).is_some_and(|(_, h)| h.to_lowercase() == host),
            };
            let path = path_of(&v.url);
            on && (v.url.trim_end_matches('/') == start
                || passing_path(&v.url)
                || site.is_some_and(|s| s.links.iter().any(|l| is_link(l, &path)))
                || search.as_deref().is_some_and(|q| !q.is_empty() && path.starts_with(q)))
        })
        .map(|v| without_name(&clean_title(site, &v.title), name).to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `pages` once each by the page they are (`page_key`): the newest stands
/// for it, with the whole group's use, named by the newest title that is
/// the page's own (`own`) — and at that title's address (the post, not its
/// photo open; the article, not its picture) —, else by its address's words,
/// else not shown.
fn once_per_page(pages: Vec<(Visit, String)>, site: Option<&Site>, own: impl Fn(&Visit, &str) -> bool) -> Vec<(Visit, String)> {
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<(Visit, String, bool)> = Vec::new();
    for (v, title) in pages {
        let key = page_key(site, &v.url);
        let fit = own(&v, &title);
        match at.get(&key) {
            Some(&i) => {
                out[i].0.score += v.score;
                if out[i].0.pic.is_none() {
                    out[i].0.pic = v.pic;
                }
                if !out[i].2 && fit {
                    (out[i].1, out[i].2) = (title, true);
                    out[i].0.url = v.url;
                }
            }
            None => {
                at.insert(key, out.len());
                out.push((v, title, fit));
            }
        }
    }
    out.into_iter()
        .filter_map(|(v, title, fit)| match fit {
            true => Some((v, title)),
            false => address_title(&v.url).map(|t| (v, t)),
        })
        .collect()
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
    let chrome = chrome_titles(&visits, site, start, name);
    let names: Vec<String> = [Some(name), site.map(|s| s.name)].into_iter().flatten().map(str::to_lowercase).collect();
    // (A video's title is its own but the app's name; another page's not one
    // its app's own pages wore — an app that changes page without loading
    // leaves the one before's —, nor one its address's words deny.)
    let own = |v: &Visit, title: &str| {
        let t = title.to_lowercase();
        let named = !t.is_empty() && !names.contains(&t) && !chrome_word(&t);
        named && !media_view(&v.url) && (video_id(&v.url).is_some() || (!chrome.contains(&t) && fits_address(&v.url, title)))
    };
    let pages: Vec<(Visit, String)> = visits
        .into_iter()
        .filter(|v| worth(&v.url) && !passing_path(&v.url) && !placeholder(&v.url))
        .map(|v| {
            let title = without_name(&clean_title(site, &v.title), name);
            (v, title)
        })
        .collect();
    let pages = once_per_page(pages, site, own);
    let mut pages = once_each(pages.into_iter(), |v, title| video_id(&v.url).map_or_else(|| title.to_lowercase(), str::to_owned));
    pages.truncate(max);
    pages
}

/// How many pages a lone app's recents are chosen from.
const CANDIDATES: usize = 400;

/// A reading of an app's recents: them, and what the owner keeps of pages
/// the history has no more — their keys among the picks and those put away
/// (`gone`: a page forgotten, the history cleared) — and when Seam last
/// forgot the whole of it (`cleared`): the dock forgets them too (Max,
/// 2026-10-28: "we need no errors" — nothing of a page wiped kept).
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Reading {
    pub recents: Vec<Recent>,
    pub gone: Vec<String>,
    pub cleared: f64,
}

/// What a webapp (named `name`, opening at `start`; of `site` when Golem
/// knows it) showed lately: from Seam's history, where every webapp lives —
/// in THE ORDER (`rank`, the owner's `picks` counted in), at most `max`.
/// (The tests' and the audit's: the box reads a `Reading`.)
#[cfg(test)]
pub(crate) fn webapp_recents(site: Option<&Site>, start: &str, name: &str, wanted: &Wanted) -> Vec<Recent> {
    webapp_reading(site, start, name, wanted).recents
}

/// `webapp_recents`, and what of the owner's the history has no more.
pub(crate) fn webapp_reading(site: Option<&Site>, start: &str, name: &str, wanted: &Wanted) -> Reading {
    let (browser, k) = (seam(), kitchen());
    // (Seam's title only when it says more than the app's name: an app that
    // keeps "YouTube Music" over a song must not hide the history's name of it.)
    let names: Vec<String> = [Some(name), site.map(|s| s.name)].into_iter().flatten().map(str::to_lowercase).collect();
    // (The owner's keys of its pages: which the history still has.)
    let hosts = webapp_hosts(site, start);
    let mut ask: Vec<String> = wanted.picks.keys().chain(wanted.forgot.hidden.keys()).filter(|key| on_hosts(key, &hosts)).cloned().collect();
    ask.sort();
    ask.dedup();
    let (visits, has) = history_with(&browser, 4000, &ask);
    let chrome = chrome_titles(&visits, site, start, name);
    let says = |t: &str| {
        let t = without_name(&clean_title(site, t), name).to_lowercase();
        !t.is_empty() && !names.contains(&t) && !chrome_word(&t) && !chrome.contains(&t)
    };
    // (A search in it: the whole history looked through for its words, as
    // the titles it shows say them.)
    let mut pages = webapp_pages(kept_pictures(retitled(visits, &k, says), &k), site, start, name, usize::MAX);
    pages.retain(|(v, title)| wanted.says(&v.url, title));
    pages.truncate(CANDIDATES);
    let plays = played(&k, site, start, crate::usage::now_secs());
    // (A song is kept by the kitchen, not the history: Spotify's opens a search.)
    let gone = match has {
        Some(has) => ask.into_iter().filter(|key| !has.contains(key) && !plays.iter().any(|(v, _)| v.url == *key)).collect(),
        None => Vec::new(),
    };
    let mut pages = with_plays(pages, plays);
    pages.retain(|(v, title)| wanted.keeps(&v.url, title, v.last));
    seam_cleared_since(&k, wanted);
    Reading { recents: pictured(&browser, &k, ranked_pages(pages, &wanted.picks, wanted.max)), gone, cleared: k.cleared }
}

/// A history with each page's title as Seam saw it last, where it kept one
/// (an app that names the song after the history wrote "YouTube Music").
/// `visits` wearing the picture Seam's kitchen kept for each one's address:
/// a page several visits are (its address tidied, a tracking tag on it, a
/// post's comment) shows the one any of them has (`once_per_page`,
/// `once_each` carry it) — the dock looked only at the address it shows.
fn kept_pictures(mut visits: Vec<Visit>, k: &Kitchen) -> Vec<Visit> {
    for v in visits.iter_mut().filter(|v| v.pic.is_none()) {
        v.pic = k.page_picture(&v.url).map(|p| p.to_string_lossy().into_owned());
    }
    visits
}

fn retitled(mut visits: Vec<Visit>, k: &Kitchen, says: impl Fn(&str) -> bool) -> Vec<Visit> {
    for v in &mut visits {
        if let Some(t) = k.title_of(&v.url).filter(|t| says(t)) {
            v.title = t;
        }
    }
    visits
}

/// A webapp's pages and what it played, newest first, once each (a video,
/// a title): a song heard and its page are one — named as the play names it
/// when that says who sings it (the history calls the page "Mentirosa", the
/// play "Mentirosa • Ráfaga"), whichever was the last.
fn with_plays(mut pages: Vec<(Visit, String)>, plays: Vec<(Visit, String)>) -> Vec<(Visit, String)> {
    let sung: HashMap<&str, &str> = plays
        .iter()
        .filter(|(_, title)| title.contains(" • "))
        .filter_map(|(v, title)| Some((video_id(&v.url)?, title.as_str())))
        .collect();
    for (v, title) in &mut pages {
        if let Some(&named) = video_id(&v.url).and_then(|id| sung.get(id)).filter(|_| !title.contains(" • ")) {
            v.title = named.to_owned();
            *title = named.to_owned();
        }
    }
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
/// (The tests' and the audit's: the box reads a `Reading`.)
#[cfg(test)]
pub(crate) fn browser_recents(browser: &Browser, skip: &[String], wanted: &Wanted) -> Vec<Recent> {
    browser_reading(browser, skip, wanted).recents
}

/// `browser_recents`, and — Seam's — when its history was cleared whole
/// (what one page of it had is not told apart from another browser's).
pub(crate) fn browser_reading(browser: &Browser, skip: &[String], wanted: &Wanted) -> Reading {
    // (Seam's: as Seam saw its pages; another browser's as it wrote them.)
    let k = if *browser == seam() { kitchen() } else { Kitchen::default() };
    // (Each page looked at before a site's newest is kept: one put away
    // gives way to the one before it, a search finds any page of a site.)
    let visits = retitled(history(browser, 3000), &k, |t| !t.trim().is_empty())
        .into_iter()
        .filter(|v| wanted.keeps(&v.url, &v.title, v.last))
        .collect();
    let pages = browser_pages(visits, skip, CANDIDATES);
    seam_cleared_since(&k, wanted);
    Reading { recents: pictured(browser, &k, ranked_pages(pages, &wanted.picks, wanted.max)), gone: Vec::new(), cleared: k.cleared }
}

/// Whether `url` is a page on one of `hosts` (patterns as a site's).
pub(crate) fn on_hosts(url: &str, hosts: &[String]) -> bool {
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    is_web(url) && origin(url).is_some_and(|(_, h)| host_is(&h.to_lowercase(), &hosts))
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
/// read off the loop) — and, a search in an app ("youtube daft"), those of
/// its that said the words last asked for (the words, the recents).
#[derive(Debug, Default)]
pub(crate) struct Recents {
    pub got: HashMap<String, (Instant, Vec<Recent>)>,
    pub found: HashMap<String, (Instant, Vec<String>, Vec<Recent>)>,
    pub asking: HashSet<String>,
    /// What the owner put away from them.
    pub forgot: Forgot,
}

/// What a reading of recents wants (`app_actions.rs`): the owner's picks
/// counted in, none put away since it was last seen (`Forgot`), only those
/// that say every one of `words` (a search in the app: "youtube daft"), at
/// most `max`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Wanted {
    pub picks: Picks,
    pub forgot: Forgot,
    pub words: Vec<String>,
    pub max: usize,
}

impl Wanted {
    /// Whether a page (or a file) seen last at `last` is one to show.
    pub(crate) fn keeps(&self, url: &str, title: &str, last: f64) -> bool {
        self.forgot.keeps(url, last) && self.says(url, title)
    }

    /// Whether its address or its title says every word.
    pub(crate) fn says(&self, url: &str, title: &str) -> bool {
        says(&self.words, url, title)
    }
}

/// The words of a search in an app, as `says` takes them.
pub(crate) fn words_of(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a page's `title`, or its address, says every one of `words`
/// (lowercase): a word of three letters or more in the address too ("pixel"
/// in an Amazon product's); a shorter one in the title alone (an address has
/// every letter and digit somewhere).
pub(crate) fn says(words: &[String], url: &str, title: &str) -> bool {
    if words.is_empty() {
        return true;
    }
    let (title, url) = (title.to_lowercase(), url.to_lowercase());
    words.iter().all(|w| title.contains(w.as_str()) || (w.chars().count() >= 3 && url.contains(w.as_str())))
}

/// RECENTS PUT AWAY (Max, 2026-10-27): everything seen before `since` (the
/// Launcher page's Clear), and each recent Delete took in the box, by its
/// address or path, as seen before then. One seen AGAIN after comes back,
/// as a page visited again comes back to a history.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Forgot {
    #[serde(default)]
    pub since: f64,
    #[serde(default)]
    pub hidden: HashMap<String, f64>,
    /// When Seam's history was cleared whole, as last seen (`Reading::cleared`).
    #[serde(default)]
    pub seam_cleared: f64,
}

/// Where it is kept, in the data dir.
const FORGOT_FILE: &str = "recents-forgot.json";
/// The most recents kept put away one by one (the oldest go first).
const HIDDEN_KEPT: usize = 4000;

impl Forgot {
    pub(crate) fn load() -> Self {
        crate::persist::read_json(&crate::persist::data_path(FORGOT_FILE)).unwrap_or_default()
    }

    fn save(&self) {
        crate::persist::write_json("recents", &crate::persist::data_path(FORGOT_FILE), self);
    }

    /// Whether a recent seen last at `last` (unix seconds; 0 unknown) is
    /// still to be shown.
    pub(crate) fn keeps(&self, key: &str, last: f64) -> bool {
        (self.since <= 0.0 || last > self.since) && self.hidden.get(key).is_none_or(|&at| last > at)
    }

    /// Put `key` away, as seen before `now`.
    pub(crate) fn hide(&mut self, key: &str, now: f64) {
        self.hidden.insert(key.to_owned(), now);
        if self.hidden.len() > HIDDEN_KEPT {
            let mut ats: Vec<f64> = self.hidden.values().copied().collect();
            ats.sort_by(f64::total_cmp);
            let cut = ats[self.hidden.len() - HIDDEN_KEPT];
            self.hidden.retain(|_, at| *at >= cut);
        }
        self.save();
    }

    /// `keys` (pages the history has no more) not kept put away: there is
    /// nothing of them to hide.
    pub(crate) fn drop_keys(&mut self, keys: &[String]) {
        if self.without(keys) {
            self.save();
        }
    }

    /// `drop_keys`, unsaved: whether any went.
    fn without(&mut self, keys: &[String]) -> bool {
        let n = self.hidden.len();
        self.hidden.retain(|k, _| !keys.contains(k));
        self.hidden.len() != n
    }

    /// Seam's history cleared whole at `at`: no address put away kept, once.
    /// Whether it was news.
    pub(crate) fn seam_cleared(&mut self, at: f64) -> bool {
        let news = self.cleared_at(at);
        if news {
            self.save();
        }
        news
    }

    /// `seam_cleared`, unsaved.
    fn cleared_at(&mut self, at: f64) -> bool {
        if at <= self.seam_cleared {
            return false;
        }
        self.seam_cleared = at;
        self.hidden.retain(|k, _| !is_web(k));
        true
    }

    /// Put away everything seen before `now`.
    pub(crate) fn clear(&mut self, now: f64) {
        self.since = now;
        self.hidden.clear();
        self.save();
    }
}

/// Seam's history cleared whole since the owner's were last told it: the
/// pictures read for its pages gone first (the loop forgets their keys).
fn seam_cleared_since(k: &Kitchen, wanted: &Wanted) {
    if k.cleared > wanted.forgot.seam_cleared {
        clear_pictures();
    }
}

/// The pictures read for recents (favicons on tiles, videos', pages'),
/// gone: the box's recents were cleared.
pub(crate) fn clear_pictures() {
    for what in ["web-tiles", "web-pictures"] {
        if let Some(dir) = cache(what) {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

/// How long recents read stay good.
pub(crate) const FRESH: Duration = Duration::from_secs(60);

#[cfg(test)]
mod audit;

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture `w`×`h` of `pad`, with a busy one (`inner`) in the middle.
    fn padded(w: u32, h: u32, pad: [u8; 3], inner: (u32, u32, u32, u32)) -> image::DynamicImage {
        let (x0, y0, iw, ih) = inner;
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            if x >= x0 && x < x0 + iw && y >= y0 && y < y0 + ih {
                image::Rgb([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x + y) % 256) as u8])
            } else {
                image::Rgb(pad)
            }
        }))
    }

    #[test]
    fn a_songs_cover_comes_out_of_youtubes_padding() {
        // YouTube's big picture of a song with no video: the square cover
        // between two bars of a colour of its own (olive, Los Charros').
        let olive = [60, 62, 18];
        let cut = unbanded(&padded(1280, 720, olive, (280, 0, 720, 720)), true).expect("cut");
        assert_eq!((cut.width(), cut.height()), (720, 720));
        assert_eq!(cut.to_rgb8().get_pixel(0, 0).0, [(280 * 7 % 256) as u8, 0, (280 % 256) as u8], "the cover's own first pixel");
        // Its 4:3 one: black bands over and under, then the bars.
        let mut small = padded(480, 360, olive, (105, 45, 270, 270)).to_rgb8();
        for x in 0..480 {
            for y in (0..45).chain(315..360) {
                small.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let cut = unbanded(&image::DynamicImage::ImageRgb8(small), true).expect("cut");
        assert_eq!((cut.width(), cut.height()), (270, 270));
        // A video's own picture stays whole: nothing even at its sides.
        assert!(unbanded(&padded(1280, 720, olive, (0, 0, 1280, 720)), true).is_none());
        // A 4:3 video in 16:9 (black bars a narrower one): the bars are not
        // the square's, so nothing is cut into it.
        assert!(unbanded(&padded(1280, 720, [0, 0, 0], (160, 0, 960, 720)), true).is_none());
        // Not a video's picture: only the 4:3 bands are YouTube's, never bars.
        assert!(unbanded(&padded(1280, 720, olive, (280, 0, 720, 720)), false).is_none());
        let banded43 = padded(480, 360, [0, 0, 0], (0, 45, 480, 270));
        assert_eq!(unbanded(&banded43, false).map(|c| (c.width(), c.height())), Some((480, 270)));
        // (A JPEG's: the picture's edge rings into the bands' nearest rows.)
        let mut rung = banded43.to_rgb8();
        for x in 0..480 {
            for y in [43, 44, 315, 316] {
                rung.put_pixel(x, y, image::Rgb([40, 40, 40]));
            }
        }
        assert_eq!(unbanded(&image::DynamicImage::ImageRgb8(rung), false).map(|c| (c.width(), c.height())), Some((480, 270)));
        // A 4:3 video dark only at its very top and bottom: not banded.
        assert!(unbanded(&padded(480, 360, [0, 0, 0], (0, 30, 480, 300)), false).is_none());
    }

    #[test]
    fn a_videos_picture_at_two_addresses_is_still_its_own() {
        let dir = std::env::temp_dir().join(format!("wa-kitchen-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pics")).unwrap();
        for f in ["00000000000000a1.jpg", "00000000000000b2.jpg", "00000000000000c3.jpg"] {
            std::fs::write(dir.join("pics").join(f), b"x").unwrap();
        }
        let mu = "https://music.youtube.com/watch?v=";
        let json = serde_json::json!({"v": 1, "plays": [], "pages": {
            // A song at two addresses, Seam's cover for both (COVERS FROM BEFORE).
            format!("{mu}sFZjqVnWBhc"): {"pic": "00000000000000a1.jpg"},
            format!("{mu}sFZjqVnWBhc&list=RDAMVMsFZjqVnWBhc"): {"pic": "00000000000000a1.jpg"},
            // A list at two addresses (COVERS FROM BEFORE: its own file).
            "https://music.youtube.com/playlist?list=UU_x5": {"pic": "00000000000000c3.jpg"},
            "https://music.youtube.com/playlist?list=UU_x5&feature=share": {"pic": "00000000000000c3.jpg"},
            // A site's logo on two of its pages.
            "https://example.com/a": {"pic": "00000000000000b2.jpg"},
            "https://example.com/b": {"pic": "00000000000000b2.jpg"},
        }});
        std::fs::write(dir.join("recents.json"), json.to_string()).unwrap();
        let k = kitchen_in(&dir);
        assert!(k.page_picture(&format!("{mu}sFZjqVnWBhc")).is_some());
        assert!(k.page_picture(&format!("{mu}sFZjqVnWBhc&list=RDAMVMsFZjqVnWBhc")).is_some());
        assert!(k.page_picture("https://music.youtube.com/playlist?list=UU_x5&feature=share").is_some());
        assert!(k.page_picture("https://example.com/a").is_none(), "a logo is no page's own");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_videos_bigger_picture_beats_the_players_still() {
        let dir = std::env::temp_dir().join(format!("wa-heard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, w: u32, h: u32| {
            let path = dir.join(name);
            image::RgbaImage::new(w, h).save_with_format(&path, image::ImageFormat::Png).unwrap();
            path
        };
        let (still, page, small) = (file("still.png", 400, 225), file("page.png", 1280, 720), file("small.png", 120, 90));
        assert_eq!(heard_or_page(Some(still.clone()), Some(page.clone())), Some(page.clone()));
        // (What played stays when it is the bigger, or the page has none of its own.)
        assert_eq!(heard_or_page(Some(still.clone()), Some(small)), Some(still.clone()));
        assert_eq!(heard_or_page(Some(still.clone()), None), Some(still));
        // (Nothing heard: the page's is found further on, as before.)
        assert_eq!(heard_or_page(None, Some(page)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_cover_is_kept_whole_for_quick_look() {
        let dir = std::env::temp_dir().join(format!("wa-fitted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = Vec::new();
        padded(1280, 720, [60, 62, 18], (280, 0, 720, 720))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let small = fitted(&bytes, &dir.join("k-x-f2.png"), true).expect("fitted");
        assert_eq!(image::image_dimensions(&small).unwrap(), (PICTURE_SIDE, PICTURE_SIDE));
        assert_eq!(image::image_dimensions(big_of(&small)).unwrap(), (720, 720));
        // (Not a video's: no bars cut, nothing beside it.)
        let other = fitted(&bytes, &dir.join("k-y-f1.png"), false).expect("fitted");
        assert_eq!(image::image_dimensions(&other).unwrap(), (512, 288));
        assert!(!big_of(&other).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recent_put_away_comes_back_only_when_seen_again() {
        let mut f = Forgot::default();
        // (Never cleared: one whose time is unknown is shown.)
        assert!(f.keeps("/music/a.mp3", 0.0));
        f.hidden.insert("https://www.youtube.com/watch?v=a".into(), 1000.0);
        assert!(!f.keeps("https://www.youtube.com/watch?v=a", 900.0));
        assert!(f.keeps("https://www.youtube.com/watch?v=a", 1100.0), "watched again after");
        assert!(f.keeps("https://www.youtube.com/watch?v=b", 900.0));
        // Clear: all seen until then, and the unknown.
        f.since = 2000.0;
        assert!(!f.keeps("https://www.youtube.com/watch?v=b", 1900.0));
        assert!(!f.keeps("/music/a.mp3", 0.0));
        assert!(f.keeps("https://www.youtube.com/watch?v=b", 2100.0));
    }

    #[test]
    fn a_search_in_an_app_keeps_the_recents_that_say_every_word() {
        let w = words_of("Daft  PUNK");
        assert!(says(&w, "https://www.youtube.com/watch?v=x", "Daft Punk - Get Lucky"));
        assert!(!says(&w, "https://www.youtube.com/watch?v=x", "Daft Bodies"));
        // A word of three letters or more in the address too; a short one in the title alone.
        assert!(says(&words_of("pixel 8"), "https://www.amazon.es/Google-Pixel-8/dp/B0C", "Google Pixel 8 128 GB"));
        assert!(says(&words_of("pixel"), "https://www.amazon.es/Google-Pixel-8/dp/B0C", "Smartphone"));
        assert!(!says(&words_of("8"), "https://www.youtube.com/watch?v=8a", "Gatos"));
        assert!(says(&[], "https://x.org/", ""));
    }

    #[test]
    fn a_copied_link_is_a_webapps_when_it_is_on_its_hosts() {
        let yt = webapp_hosts(site("youtube"), "https://www.youtube.com");
        assert!(on_hosts("https://www.youtube.com/watch?v=abc", &yt));
        assert!(!on_hosts("https://music.youtube.com/watch?v=abc", &yt));
        assert!(!on_hosts("ftp://www.youtube.com/x", &yt));
        let own = webapp_hosts(None, "https://app.example.org/home");
        assert!(on_hosts("https://APP.example.org/x", &own));
        assert!(!on_hosts("https://example.org/x", &own));
    }

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
        // Words said every day, letters, names: none asks a search.
        for typed in ["so long", "x men", "g major", "ali baba", "keep calm"] {
            assert!(asked(typed).is_none(), "{typed:?} asked a search");
        }
        for s in SITES {
            assert!(s.words.iter().all(|w| w.chars().count() >= 2), "{}: a letter asks its search", s.slug);
        }
        let keep = site("google-keep").unwrap();
        assert_eq!(search_url(keep, "lista super").unwrap(), "https://keep.google.com/#search/text%253Dlista%20super");
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
        let pl = "https://music.youtube.com/playlist?list=";
        assert_eq!(list_id(&format!("{pl}PLOAQmvgSqKxbVm_3IfLjkI44Ab5-1_Fsa")), Some("PLOAQmvgSqKxbVm_3IfLjkI44Ab5-1_Fsa"));
        assert_eq!(list_id("https://www.youtube.com/playlist?feature=x&list=OLAK5uy_abc#t"), Some("OLAK5uy_abc"));
        assert_eq!(list_id("https://music.youtube.com/watch?v=9d9PQ1hrhBk&list=RDAMVM9d9PQ1hrhBk"), None, "a video played from a list");
        assert_eq!(list_id("https://music.youtube.com/browse/playlist?list=x"), None);
        assert_eq!(list_id(pl), None);
        assert_eq!(list_id("https://example.com/playlist?list=PL1"), None);
        assert_eq!(picture_rank("hq720_custom_3.jpg?sqp=x"), 4);
        assert_eq!(picture_rank("mqdefault.jpg"), 3);
        assert_eq!(picture_rank("maxresdefault.jpg"), 0);
    }

    fn visit(url: &str, title: &str) -> Visit {
        Visit { url: url.into(), title: title.into(), last: 0.0, score: 0.0, pic: None, times: 0, played: false }
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
            (Visit { last: 50.0, score: 2.0, ..visit("a", "A") }, "A".to_string()),
            (Visit { last: 40.0, score: 1.0, ..visit("b", "B") }, "B".to_string()),
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

    /// Each site's own kind of page as it is named in a history, and the
    /// name its recent wears.
    const EVERY_SITE: &[(&str, &str, &str, &str)] = &[
        ("youtube", "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "(2) Rick Astley - Never Gonna Give You Up - YouTube", "Rick Astley - Never Gonna Give You Up"),
        ("google", "https://www.google.com/search?q=gatos&client=firefox-b-d", "gatos - Buscar con Google", "gatos"),
        ("facebook", "https://www.facebook.com/groups/123456/", "(5) Linux Argentina | Facebook", "Linux Argentina"),
        ("instagram", "https://www.instagram.com/p/C1a2b3d/", "Leo Messi on Instagram: \"Campeones\"", "Leo Messi on Instagram: \"Campeones\""),
        ("tiktok", "https://www.tiktok.com/@cats/video/7300000000000000000", "Funny cat | TikTok", "Funny cat"),
        ("x", "https://x.com/dhh/status/2101337058362999065", "(1) DHH on X: \"Hello\" / X", "DHH on X: \"Hello\""),
        ("reddit", "https://www.reddit.com/r/linux/comments/1xgyafd/my_new_setup/", "My new setup : r/linux", "My new setup"),
        ("wikipedia", "https://es.wikipedia.org/wiki/Bauhaus", "Bauhaus - Wikipedia, la enciclopedia libre", "Bauhaus"),
        ("amazon", "https://www.amazon.com/Running-Shoes/dp/B0C1234567?th=1", "Amazon.com: Running Shoes : Clothing", "Running Shoes : Clothing"),
        ("chatgpt", "https://chatgpt.com/c/6adfe8d0-c2a8-83e8-a693-146184afd397", "ChatGPT - Recipe ideas", "Recipe ideas"),
        ("netflix", "https://www.netflix.com/title/80057281", "Stranger Things | Netflix Official Site", "Stranger Things"),
        ("linkedin", "https://www.linkedin.com/in/jane-doe/", "(3) Jane Doe | LinkedIn", "Jane Doe"),
        ("spotify", "https://open.spotify.com/album/6l8tOdUItDC01Qmb91Lo8W", "The Game - Album by Queen | Spotify", "The Game - Album by Queen"),
        ("twitch", "https://www.twitch.tv/videos/2200000000", "Speedrun - Twitch", "Speedrun"),
        ("pinterest", "https://www.pinterest.com/pin/123456789/", "Cozy room | Pinterest", "Cozy room"),
        ("discord", "https://discord.com/channels/1234/5678", "Discord | #general | Golem", "#general | Golem"),
        ("google-maps", "https://www.google.com/maps/place/Obelisco/@-34.6,-58.38,17z", "Obelisco - Google Maps", "Obelisco"),
        ("google-drive", "https://drive.google.com/file/d/1abcDEF/view", "report.pdf - Google Drive", "report.pdf"),
        ("google-docs", "https://docs.google.com/document/d/1abcDEF/edit?tab=t.0", "Plan - Google Docs", "Plan"),
        ("claude", "https://claude.ai/chat/47efddb8-9924-4ccf-b1b5-14f1108e0a21", "Greeting - Claude", "Greeting"),
        ("gemini", "https://gemini.google.com/app/1a2b3c4d5e", "Trip ideas - Gemini", "Trip ideas"),
        ("ebay", "https://www.ebay.com/itm/276936012392?_trksid=p2332490", "Google Pixel 9 | eBay", "Google Pixel 9"),
        ("github", "https://github.com/maxpoww/waveview", "maxpoww/waveview · GitHub", "maxpoww/waveview"),
        ("canva", "https://www.canva.com/design/DAF1abc/edit", "Poster - Canva", "Poster"),
        ("aliexpress", "https://www.aliexpress.com/item/1005001.html", "USB cable - AliExpress", "USB cable"),
        ("booking", "https://www.booking.com/hotel/ar/alvear-palace.html", "Alvear Palace Hotel, Buenos Aires", "Alvear Palace Hotel, Buenos Aires"),
        ("airbnb", "https://www.airbnb.com/rooms/12345", "Cabin in the woods - Airbnb", "Cabin in the woods"),
        ("youtube-music", "https://music.youtube.com/watch?v=sFZjqVnWBhc", "Robot Rock - YouTube Music", "Robot Rock"),
        ("steam", "https://store.steampowered.com/app/620/Portal_2/", "Portal 2 on Steam", "Portal 2"),
        ("stackoverflow", "https://stackoverflow.com/questions/123/how-to-exit-vim", "How to exit Vim - Stack Overflow", "How to exit Vim"),
        ("notion", "https://www.notion.so/Plan-1a2b3c", "Plan", "Plan"),
        ("figma", "https://www.figma.com/design/abc123/Dock", "Dock – Figma", "Dock"),
    ];

    #[test]
    fn every_site_shows_its_own_pages_as_they_are_named() {
        let mut seen = HashSet::new();
        for (slug, url, raw, named) in EVERY_SITE {
            let s = site(slug).unwrap_or_else(|| panic!("{slug}: no site"));
            assert!(seen.insert(*slug), "{slug} twice");
            let start = format!("https://{}", s.hosts[0]);
            // Newest first: the page again at another address of its own, its
            // site's own pages (its start, its known pages, a search, a
            // sign-in), then the page.
            let mut history = vec![visit(&format!("{url}#top"), raw), visit(&start, s.name), visit(&format!("{start}/login"), s.name)];
            for l in s.links {
                history.push(visit(&link_url(l, &start), l.name));
            }
            let search = search_url(s, "zapatillas").filter(|q| !is_page(s, q));
            if let Some(q) = &search {
                history.push(visit(q, &format!("zapatillas - {}", s.name)));
            }
            history.push(visit(url, raw));
            let got = webapp_pages(history, Some(s), &start, s.name, 8);
            let titles: Vec<&str> = got.iter().map(|(_, t)| t.as_str()).collect();
            assert_eq!(titles, [*named], "{slug}: {got:?}");
        }
        // Every site with kinds of page worth it is here; the rest show none.
        for s in SITES.iter().filter(|s| !seen.contains(s.slug)) {
            assert!(s.pages.is_empty(), "{}: a site with pages not tried", s.slug);
            let start = format!("https://{}", s.hosts[0]);
            assert!(webapp_pages(vec![visit(&format!("{start}/a/b"), "A page")], Some(s), &start, s.name, 8).is_empty(), "{}", s.slug);
        }
    }

    #[test]
    fn a_wikis_picture_view_is_its_article_and_its_own_pages_are_none() {
        let wiki = site("wikipedia").unwrap();
        let at = |url: &str, title: &str, last: f64| Visit { last, score: 1.0, ..visit(url, title) };
        let visits = vec![
            at("https://en.wikipedia.org/wiki/Bauhaus#/media/File:Joost-schmidt.jpg", "Joost-schmidt - Wikipedia", 9.0),
            at("https://en.wikipedia.org/wiki/Bauhaus", "Bauhaus - Wikipedia", 8.0),
            at("https://en.wikipedia.org/wiki/Special:Search?search=permacultura&go=Go", "permacultura - Search results - Wikipedia", 7.0),
            at("https://es.wikipedia.org/wiki/Wikipedia:Portada", "Wikipedia, la enciclopedia libre", 6.0),
            at("https://en.wikipedia.org/wiki/Main_Page", "Wikipedia, the free encyclopedia", 5.0),
            at("https://es.wikipedia.org/wiki/Especial:Aleatoria", "Gato - Wikipedia, la enciclopedia libre", 4.0),
            at("https://es.wikipedia.org/wiki/Portal:Actualidad", "Portal:Actualidad - Wikipedia, la enciclopedia libre", 3.5),
            at("https://en.wikipedia.org/wiki/Star_Wars:_Episode_IV", "Star Wars: Episode IV - Wikipedia", 3.0),
        ];
        let got = webapp_pages(visits, Some(wiki), "https://www.wikipedia.org", "Wikipedia", 24);
        let shown: Vec<(&str, &str, f64)> = got.iter().map(|(v, t)| (v.url.as_str(), t.as_str(), v.last)).collect();
        assert_eq!(
            shown,
            [
                ("https://en.wikipedia.org/wiki/Bauhaus", "Bauhaus", 9.0),
                ("https://en.wikipedia.org/wiki/Star_Wars:_Episode_IV", "Star Wars: Episode IV", 3.0),
            ]
        );
    }

    #[test]
    fn a_page_without_a_favicon_wears_a_sheet() {
        let dir = scratch("sheet");
        let tile = tile_in(&dir, SHEET.as_bytes()).expect("drawn");
        let img = image::open(&tile).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (crate::apps::ICON_SIZE, crate::apps::ICON_SIZE));
        // Its ground is the favicons' card; its lines darker, inside it.
        assert_eq!(img.get_pixel(4, 4).0, TILE_GROUND);
        let dark = img.pixels().filter(|p| p.0[0] < 200).count();
        assert!(dark > 500, "{dark} dark pixels");
        if std::env::var_os("WR_KEEP").is_some() {
            println!("{}", tile.display());
        } else {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_page_an_app_named_with_its_own_pages_title_is_not_shown_so() {
        // X kept "Home" over a post with its photo open, and the search's
        // title over another; the post itself came back named — at the
        // post's own address, the one that names it.
        let x = site("x");
        let history = vec![
            visit("https://x.com/dhh/status/2107462374915523022/photo/1", "(1) Home / X"),
            visit("https://x.com/dhh/status/2102366320369016915/photo/1", "omarchy - Search / X"),
            visit("https://x.com/search?q=omarchy&src=typed_query", "omarchy - Search / X"),
            visit("https://x.com/home", "(1) Home / X"),
            visit("https://x.com/dhh/status/2107462374915523022", "DHH on X: \"Linux\" / X"),
        ];
        let got = webapp_pages(history, x, "https://x.com", "X", 8);
        assert_eq!(got.iter().map(|(v, t)| (v.url.as_str(), t.as_str())).collect::<Vec<_>>(), [(
            "https://x.com/dhh/status/2107462374915523022",
            "DHH on X: \"Linux\""
        )]);
        // Reddit's "Inbox" and its tagline over posts, and a picture of one
        // open: one post each, by its own title, else its address's words;
        // cross-posts once (their subreddit is not their title).
        let reddit = site("reddit");
        let history = vec![
            visit("https://www.reddit.com/r/linux/comments/1xecwnz/ok_ill_make_it_shorter_why_omarchy/19rl/", "Inbox"),
            visit("https://www.reddit.com/r/AskProgrammers/comments/1xdgn4g/what_to_work_on_outside_of_work/", "Reddit - The heart of the internet"),
            visit("https://www.reddit.com/r/unixporn/comments/1x95tsq/xfce_its_just_mac_os_x/#lightbox", "r/unixporn - the home for *NIX customization!"),
            visit("https://www.reddit.com/r/unixporn/", "r/unixporn - the home for *NIX customization!"),
            visit("https://www.reddit.com/r/unixporn/comments/1x95tsq/xfce_its_just_mac_os_x/", "[XFCE] it's just Mac OS X : r/unixporn"),
            visit("https://www.reddit.com/r/linux/comments/1xecwnz/comment/19rlbvf7me1b6/", "ok, I'll make it shorter: Why omarchy : r/linux"),
            visit("https://www.reddit.com/r/NixOS/comments/1xf30to/hello_everybody/", "Hello everybody : r/NixOS"),
            visit("https://www.reddit.com/r/DistroHopping/comments/1xdtgan/hello_everybody/", "Hello everybody : r/DistroHopping"),
        ];
        let got = webapp_pages(history, reddit, "https://www.reddit.com", "Reddit", 8);
        assert_eq!(
            got.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
            ["ok, I'll make it shorter: Why omarchy", "What to work on outside of work", "[XFCE] it's just Mac OS X", "Hello everybody"]
        );
        // A sign-in's title the app kept over a page of its own (Cloudflare).
        let history = vec![
            visit("https://dash.cloudflare.com/abc/r2/overview", "Cloudflare Dashboard | Manage Your Account"),
            visit("https://dash.cloudflare.com/login", "Cloudflare Dashboard | Manage Your Account"),
            visit("https://dash.cloudflare.com/abc/home", "Account home | Cloudflare"),
        ];
        let got = webapp_pages(history, None, "https://dash.cloudflare.com", "Cloudflare", 8);
        assert_eq!(got.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["Account home"]);
        // A known page of its own is its row's action, not a recent; a chat not
        // kept yet opens nothing; a sign-in's steps are no place.
        let pages = |slug: &str, history: Vec<Visit>| {
            let s = site(slug).unwrap();
            webapp_pages(history, Some(s), &format!("https://{}", s.hosts[0]), s.name, 8)
        };
        assert!(pages("discord", vec![visit("https://discord.com/channels/@me", "Discord | Friends")]).is_empty());
        assert!(pages("youtube-music", vec![visit("https://music.youtube.com/playlist?list=LM", "Liked music - YouTube Music")]).is_empty());
        assert!(pages("chatgpt", vec![visit("https://chatgpt.com/c/local-chatgpt%3A738e6c28", "what is the best tool")]).is_empty());
        let github = vec![
            visit("https://github.com/sessions/verified-device/mobile?auto=true", "Where software is built"),
            visit("https://github.com/sessions/social/google/initiate?return_to=", "Redirecting to Google"),
            visit("https://github.com/max-pow?tab=repositories", "max-pow / Repositories"),
            visit("https://github.com/max-pow", "max-pow"),
        ];
        assert_eq!(pages("github", github).iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["max-pow / Repositories"]);
        // What shows nothing in a title, and a run of spaces.
        assert_eq!(clean_title(None, "\u{200e}Apple Music - Web Player"), "Apple Music - Web Player");
        assert_eq!(clean_title(None, "Stromae   Alors on danse"), "Stromae Alors on danse");
        assert!(webapp_pages(vec![visit("https://music.apple.com/us/new", "\u{200e}Apple Music - Web Player")], None, "https://music.apple.com", "Apple Music", 8).is_empty());
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
        let page = (Visit { last: 5.0, score: 1.0, ..visit("https://music.youtube.com/watch?v=9d9PQ1hrhBk", "Robot Rock") }, "Robot Rock".to_string());
        let both = with_plays(vec![page.clone()], ym.clone());
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].1, "Robot Rock • Daft Punk");
        // Its page seen since: the page, still named with who sings it.
        let newer = (Visit { last: 30.0, ..page.0.clone() }, page.1.clone());
        let both = with_plays(vec![newer], ym);
        assert_eq!((both.len(), both[0].0.last, both[0].1.as_str(), both[0].0.title.as_str()), (1, 30.0, "Robot Rock • Daft Punk", "Robot Rock • Daft Punk"));
        // Not a file of its own, or not ours at all: nothing.
        assert_eq!(kitchen_in(&dir.join("nowhere")).plays.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_several_visits_are_wears_the_picture_any_of_them_has() {
        let dir = scratch("one-page-pics");
        std::fs::create_dir_all(dir.join("pics")).unwrap();
        for name in ["0123456789abcdef.jpg", "fedcba9876543210.jpg"] {
            std::fs::write(dir.join("pics").join(name), b"jpeg").unwrap();
        }
        std::fs::write(
            dir.join("recents.json"),
            r#"{"v":1,"pages":{
                "https://es.wikipedia.org/wiki/Permacultura":{"title":"Permacultura - Wikipedia, la enciclopedia libre","pic":"0123456789abcdef.jpg"},
                "https://www.reddit.com/r/linux/comments/abc123/a_post/":{"title":"A post","pic":"fedcba9876543210.jpg"},
                "https://www.reddit.com/r/linux/comments/abc123/comment/def456/?screen_view_count=2":{"title":"A post","pic":"fedcba9876543210.jpg"}
            },"plays":[]}"#,
        )
        .unwrap();
        let k = kitchen_in(&dir);
        // A post and its comment wearing one picture are one page: not a logo.
        assert!(k.page_picture("https://www.reddit.com/r/linux/comments/abc123/a_post/").is_some());
        // The newest visit had a tracking tag; the picture is on the clean address.
        let at = |url: &str, last: f64| Visit { last, score: 1.0, ..visit(url, "Permacultura - Wikipedia, la enciclopedia libre") };
        let visits = vec![at("https://es.wikipedia.org/wiki/Permacultura?wprov=srpw1_0", 9.0), at("https://es.wikipedia.org/wiki/Permacultura", 8.0)];
        let got = webapp_pages(kept_pictures(visits, &k), site("wikipedia"), "https://www.wikipedia.org", "Wikipedia", 24);
        assert_eq!(got.len(), 1);
        assert!(got[0].0.pic.as_deref().is_some_and(|p| p.ends_with("0123456789abcdef.jpg")), "{:?}", got[0].0.pic);
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
            let got = webapp_recents(Some(s), &format!("https://{}", s.hosts[0]), s.name, &Wanted { max: 8, ..Default::default() });
            println!("{slug}: {} in {:?}", got.len(), at.elapsed());
            for r in got {
                println!("   {:60} {}", r.title.chars().take(60).collect::<String>(), r.picture.map(|p| p.display().to_string()).unwrap_or_default());
            }
        }
        for (name, start) in [("Google Keep", "https://keep.google.com"), ("Cloudflare", "https://dash.cloudflare.com")] {
            let got = webapp_recents(None, start, name, &Wanted { max: 8, ..Default::default() });
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
            let got = browser_recents(&b, if prog == "seam" { &skip } else { &[] }, &Wanted { max: 8, ..Default::default() });
            println!("{prog}: {} in {:?}", got.len(), at.elapsed());
            for r in got {
                println!("   {:60} {}", r.title.chars().take(60).collect::<String>(), r.url);
            }
        }
    }

    #[test]
    fn what_the_history_has_no_more_is_kept_no_more() {
        // Which of the owner's keys a history still has — none told when it
        // cannot be read (nothing is then taken for gone).
        let dir = scratch("gone");
        let places = rusqlite::Connection::open(dir.join("places.sqlite")).unwrap();
        places
            .execute_batch(
                "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, last_visit_date INTEGER);
                 CREATE TABLE moz_historyvisits (place_id INTEGER, visit_date INTEGER);
                 INSERT INTO moz_places VALUES (1, 'https://www.youtube.com/watch?v=a', 'A - YouTube', 10);",
            )
            .unwrap();
        drop(places);
        let ask = vec!["https://www.youtube.com/watch?v=a".to_string(), "https://www.youtube.com/watch?v=wiped".to_string()];
        let (visits, has) = history_with(&Browser::Firefox(dir.clone()), 10, &ask);
        assert_eq!(visits.len(), 1);
        assert_eq!(has, Some(["https://www.youtube.com/watch?v=a".to_string()].into()));
        assert_eq!(history_with(&Browser::Firefox(dir.join("none")), 10, &ask).1, None);
        // The pictures made of Seam's: only while Seam keeps what they were
        // made of (its kitchen's picture, its cache's entry).
        let shots = dir.join("shots");
        let pics = dir.join("kitchen/pics");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::create_dir_all(&pics).unwrap();
        std::fs::write(pics.join("00000000000000aa.jpg"), b"jpg").unwrap();
        let entry = dir.join("ENTRY");
        let videos: Pictures = [("dQw4w9WgXcQ".to_string(), (2u8, entry.clone(), 3u64))].into();
        let big = format!("big-{}.jpg", hash_of(entry.to_string_lossy().as_bytes()));
        let made = ["k-00000000000000aa-f4.png", "k-00000000000000aa-f4-big.png", "k-00000000000000bb-f3.png", "dQw4w9WgXcQ-f4.png", "k-abcdefghi-f4.png", &big, "big-00000000000000cc.jpg", "other.png"];
        for name in made {
            std::fs::write(shots.join(name), b"png").unwrap();
        }
        let kitchen = Kitchen { dir: dir.join("kitchen"), ..Default::default() };
        sweep_unkept(&shots, &kitchen, Some(&videos));
        let mut left: Vec<String> = std::fs::read_dir(&shots).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        let mut want = vec!["k-00000000000000aa-f4.png", "k-00000000000000aa-f4-big.png", "dQw4w9WgXcQ-f4.png", big.as_str(), "other.png"];
        want.sort();
        assert_eq!(left, want, "a page's picture Seam forgot, a video its cache let go");
        // No kitchen read and no cache read: nothing to tell by, nothing goes.
        sweep_unkept(&shots, &Kitchen::default(), None);
        assert_eq!(std::fs::read_dir(&shots).unwrap().count(), want.len());
        // The put away (unsaved here: the owner's file is not a test's): a
        // key gone; all the web's once Seam's history is cleared, once.
        let both = || Forgot { hidden: [("https://a.com/x".to_string(), 5.0), ("/home/me/file.txt".to_string(), 5.0)].into(), ..Default::default() };
        let mut f = both();
        assert!(f.without(&["https://a.com/x".to_string()]) && !f.without(&["https://a.com/x".to_string()]));
        assert_eq!(f.hidden.keys().collect::<Vec<_>>(), ["/home/me/file.txt"]);
        let mut f = both();
        assert!(!f.cleared_at(0.0), "never cleared");
        assert!(f.cleared_at(100.0) && !f.cleared_at(100.0) && !f.cleared_at(50.0));
        assert_eq!(f.hidden.keys().collect::<Vec<_>>(), ["/home/me/file.txt"], "a file is not Seam's");
        let _ = std::fs::remove_dir_all(&dir);
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





