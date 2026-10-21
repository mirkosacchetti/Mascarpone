//! What comes from the net, the way Lollypop does it, without keys of
//! one's own. Artists' portraits and biographies: MusicBrainz names the
//! artist and links it to Wikidata, Wikidata to the Wikipedia article, whose
//! summary gives the biography and the lead picture; TheAudioDB (its
//! free key is `123`) gives a portrait by MusicBrainz id and a biography
//! when Wikipedia has none; Deezer a picture when nobody else has one.
//! Everything found is kept in the database (the words) and in
//! `~/.cache/mascarpone/artists` (a JPEG with the face) and never asked
//! twice; what is not found is remembered as such too (a JSON there from
//! before the database is taken in the first time its artist is asked). An album's cover, when
//! mpd has none, from Deezer or the Cover Art Archive, written as
//! `cover.jpg` in the album's folder like the others. MusicBrainz wants
//! a request a second at most and a User-Agent that says who asks.
//! For the radio, an artist's links to others: MusicBrainz's relations
//! (the band's members, collaborations, tributes), Wikidata's
//! influences (P737, resolved to names and MusicBrainz ids in one more
//! call), Deezer's related artists; all kept in the database by the
//! radio thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::covers;

const USER_AGENT: &str = "mascarpone/0.1 (https://random.cards)";

/// What the net said about an artist, as kept on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Info {
    pub bio: Option<String>,
    pub wikipedia: Option<String>,
    /// The portrait file is there.
    pub portrait: bool,
}

pub fn cache_dir() -> PathBuf {
    crate::dirs::cache().join("mascarpone").join("artists")
}

pub fn portrait_path(name: &str) -> PathBuf {
    cache_dir().join(format!("{}.jpg", covers::hash(name)))
}

fn info_path(name: &str) -> PathBuf {
    cache_dir().join(format!("{}.json", covers::hash(name)))
}

/// The net's throttle: MusicBrainz's last call, so the next waits.
pub struct Net {
    client: reqwest::Client,
    musicbrainz: Mutex<Instant>,
}

impl Net {
    pub fn new() -> Arc<Net> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .build()
            .expect("http client");
        Arc::new(Net { client, musicbrainz: Mutex::new(Instant::now() - Duration::from_secs(2)) })
    }

    /// A call to MusicBrainz, a second after the last; a 503 (busy, or
    /// too many calls) is tried again a few times, after a wait.
    async fn musicbrainz(&self, url: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        let mut last = self.musicbrainz.lock().await;
        for attempt in 0.. {
            let wait = Duration::from_millis(1100).saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            let response = self.client.get(url).query(query).send().await?;
            *last = Instant::now();
            if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE && attempt < 3 {
                tokio::time::sleep(Duration::from_secs(5 << attempt)).await;
                continue;
            }
            return Ok(response.error_for_status()?.json().await?);
        }
        unreachable!()
    }

    async fn json(&self, url: &str) -> Result<serde_json::Value> {
        Ok(self.client.get(url).send().await?.error_for_status()?.json().await?)
    }
}

/// Forgets what is kept on an artist, for a new try.
pub fn forget(db: &crate::db::Shared, name: &str) {
    if let Err(e) = db.lock().unwrap().forget_info(name) {
        eprintln!("artist {name}: {e}");
    }
    let _ = std::fs::remove_file(info_path(name));
    let _ = std::fs::remove_file(portrait_path(name));
}

/// The artist's info, from the database or from the net (and then into
/// the database). A net failure leaves nothing, so it is tried again later.
pub async fn lookup(net: &Net, db: &crate::db::Shared, name: &str) -> Info {
    match db.lock().unwrap().artist_info(name) {
        Ok(Some(info)) => return info,
        Ok(None) => {}
        Err(e) => eprintln!("artist {name}: {e}"),
    }
    // the file from before the database: taken in, and gone
    let path = info_path(name);
    let info = match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Info>(&b).ok()) {
        Some(info) => {
            let _ = std::fs::remove_file(&path);
            info
        }
        None => match fetch(net, name).await {
            Ok(info) => info,
            Err(e) => {
                eprintln!("artist {name}: {e}");
                return Info::default();
            }
        },
    };
    if let Err(e) = db.lock().unwrap().write_info(name, &info) {
        eprintln!("artist {name}: {e}");
    }
    info
}

async fn fetch(net: &Net, name: &str) -> Result<Info> {
    let mut info = Info::default();
    let mut image_url: Option<String> = None;
    let mut audiodb_bio: Option<String> = None;

    // MusicBrainz: the artist, then its links
    let found = net
        .musicbrainz("https://musicbrainz.org/ws/2/artist/", &[("query", format!("artist:\"{name}\"")), ("fmt", "json".into()), ("limit", "1".into())])
        .await?;
    let artist = found["artists"].get(0).filter(|a| a["score"].as_i64().unwrap_or(0) >= 85);
    if let Some(id) = artist.and_then(|a| a["id"].as_str()) {
        // TheAudioDB by MusicBrainz id: a portrait, and words in reserve
        if let Ok(found) = net.json(&format!("https://www.theaudiodb.com/api/v1/json/123/artist-mb.php?i={id}")).await {
            if let Some(a) = found["artists"].get(0) {
                image_url = a["strArtistThumb"].as_str().filter(|u| !u.is_empty()).map(str::to_owned);
                audiodb_bio = ["strBiographyEN", "strBiographyIT"].iter().find_map(|k| a[k].as_str().filter(|b| !b.trim().is_empty()).map(|b| b.trim().to_owned()));
            }
        }
        let full = net.musicbrainz(&format!("https://musicbrainz.org/ws/2/artist/{id}"), &[("inc", "url-rels".into()), ("fmt", "json".into())]).await?;
        let mut wikidata: Option<String> = None;
        let mut wikipedia: Option<(String, String)> = None; // (lang, title)
        for rel in full["relations"].as_array().into_iter().flatten() {
            let url = rel["url"]["resource"].as_str().unwrap_or_default();
            match rel["type"].as_str().unwrap_or_default() {
                "wikidata" => wikidata = url.rsplit('/').next().map(str::to_owned),
                "wikipedia" => wikipedia = wiki_parts(url).or(wikipedia),
                _ => {}
            }
        }
        // Wikidata: the article, English first
        if wikipedia.is_none() {
            if let Some(q) = &wikidata {
                let entity = net.json(&format!("https://www.wikidata.org/wiki/Special:EntityData/{q}.json")).await?;
                let links = &entity["entities"][q]["sitelinks"];
                for lang in ["en", "it"] {
                    if let Some(title) = links[format!("{lang}wiki")]["title"].as_str() {
                        wikipedia = Some((lang.into(), title.to_owned()));
                        break;
                    }
                }
            }
        }
        // Wikipedia: the summary, the words and the picture
        if let Some((lang, title)) = wikipedia {
            let title = title.replace(' ', "_");
            let url = format!("https://{lang}.wikipedia.org/api/rest_v1/page/summary/{}", urlencode(&title));
            if let Ok(summary) = net.json(&url).await {
                info.bio = summary["extract"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
                info.wikipedia = summary["content_urls"]["desktop"]["page"].as_str().map(str::to_owned);
                if image_url.is_none() {
                    image_url = summary["originalimage"]["source"].as_str().map(str::to_owned);
                }
            }
        }
    }
    if info.bio.is_none() {
        info.bio = audiodb_bio;
    }

    // Deezer for a face when Wikipedia has none
    if image_url.is_none() {
        let found = net.client.get("https://api.deezer.com/search/artist").query(&[("q", name), ("limit", "1")]).send().await?.error_for_status()?.json::<serde_json::Value>().await?;
        if let Some(first) = found["data"].get(0) {
            if first["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                image_url = deezer_image(&first["picture_xl"]);
            }
        }
    }

    if let Some(url) = image_url {
        let bytes = net.client.get(&url).send().await?.error_for_status()?.bytes().await?;
        let path = portrait_path(name);
        match tokio::task::spawn_blocking(move || covers::thumbnail(&bytes, &path)).await? {
            Ok(()) => info.portrait = true,
            Err(e) => eprintln!("artist {name}: picture: {e}"),
        }
    }
    Ok(info)
}

/// What the net says an artist is linked to, and their ids there.
pub struct Links {
    pub mbid: Option<String>,
    pub deezer: Option<i64>,
    pub links: Vec<crate::db::Link>,
}

/// An artist's links: MusicBrainz's artist relations and the Wikidata
/// id, Wikidata's influences, Deezer's related artists. An error only
/// when MusicBrainz could not be asked; the rest missing is just missing.
pub async fn links(net: &Net, name: &str) -> Result<Links> {
    let mut links = Links { mbid: None, deezer: None, links: Vec::new() };
    let link = |other: &str, other_mbid: Option<&str>, other_deezer: Option<i64>, kind: &str, source: &str| crate::db::Link { other: other.to_owned(), other_mbid: other_mbid.map(str::to_owned), other_deezer, kind: kind.to_owned(), source: source.to_owned() };
    let found = net
        .musicbrainz("https://musicbrainz.org/ws/2/artist/", &[("query", format!("artist:\"{name}\"")), ("fmt", "json".into()), ("limit", "1".into())])
        .await?;
    let artist = found["artists"].get(0).filter(|a| a["score"].as_i64().unwrap_or(0) >= 85);
    if let Some(id) = artist.and_then(|a| a["id"].as_str()) {
        links.mbid = Some(id.to_owned());
        let full = net.musicbrainz(&format!("https://musicbrainz.org/ws/2/artist/{id}"), &[("inc", "artist-rels+url-rels".into()), ("fmt", "json".into())]).await?;
        let mut wikidata: Option<String> = None;
        for rel in full["relations"].as_array().into_iter().flatten() {
            let kind = rel["type"].as_str().unwrap_or_default();
            if let Some(other) = rel["artist"]["name"].as_str() {
                if !other.eq_ignore_ascii_case(name) {
                    links.links.push(link(other, rel["artist"]["id"].as_str(), None, kind, "musicbrainz"));
                }
            } else if kind == "wikidata" {
                wikidata = rel["url"]["resource"].as_str().and_then(|u| u.rsplit('/').next()).map(str::to_owned);
            }
        }
        // Wikidata: whom the artist was influenced by, then those
        // entities' names and MusicBrainz ids in one call
        if let Some(q) = wikidata {
            if let Ok(entity) = net.json(&format!("https://www.wikidata.org/wiki/Special:EntityData/{q}.json")).await {
                let ids: Vec<String> = entity["entities"][&q]["claims"]["P737"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["mainsnak"]["datavalue"]["value"]["id"].as_str().map(str::to_owned))
                    .take(50)
                    .collect();
                if !ids.is_empty() {
                    let url = format!("https://www.wikidata.org/w/api.php?action=wbgetentities&ids={}&props=labels|claims&languages=en&format=json", ids.join("|"));
                    if let Ok(found) = net.json(&url).await {
                        for id in &ids {
                            let e = &found["entities"][id];
                            let Some(label) = e["labels"]["en"]["value"].as_str() else { continue };
                            let mbid = e["claims"]["P434"][0]["mainsnak"]["datavalue"]["value"].as_str();
                            links.links.push(link(label, mbid, None, "influenced by", "wikidata"));
                        }
                    }
                }
            }
        }
    }
    // Deezer: the artist, then the related ones
    let found = net.client.get("https://api.deezer.com/search/artist").query(&[("q", name), ("limit", "1")]).send().await?.error_for_status()?.json::<serde_json::Value>().await?;
    let deezer = found["data"].get(0).filter(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name))).and_then(|a| a["id"].as_i64());
    if let Some(id) = deezer {
        links.deezer = Some(id);
        if let Ok(related) = net.json(&format!("https://api.deezer.com/artist/{id}/related?limit=25")).await {
            for a in related["data"].as_array().into_iter().flatten() {
                if let Some(other) = a["name"].as_str() {
                    links.links.push(link(other, None, a["id"].as_i64(), "similar", "deezer"));
                }
            }
        }
    }
    Ok(links)
}

/// Language and title of a Wikipedia article url.
fn wiki_parts(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let lang = host.strip_suffix(".wikipedia.org")?;
    let title = path.strip_prefix("wiki/")?;
    Some((lang.to_owned(), urldecode(title)))
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'(' | b')' | b',' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).replace('_', " ")
}

/// A title for matching: lowercase, without what brackets add ("(2011
/// reissue)", "[Deluxe Edition]"), spaces squeezed.
fn plain(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            c if depth == 0 => out.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A picture's address from Deezer, unless it is Deezer's own stand-in:
/// for an artist or an album without a picture it still gives one, with
/// an empty id (`.../images/artist//1000x1000-...`), a grey person or
/// disc on white.
fn deezer_image(url: &serde_json::Value) -> Option<String> {
    url.as_str().filter(|u| !u.is_empty() && !u.contains("/images/artist//") && !u.contains("/images/cover//")).map(str::to_owned)
}

/// An album's cover from the net, as the bytes of a JPEG: Deezer's
/// (1000 px) when a match has the album's title (edition notes aside)
/// and artist, else the Cover Art Archive's front (500 px) of the
/// release group MusicBrainz has under that title and artist.
pub async fn album_cover(net: &Net, artist: &str, album: &str) -> Result<Option<Vec<u8>>> {
    let wanted = plain(album);
    let found = net
        .client
        .get("https://api.deezer.com/search/album")
        .query(&[("q", format!("artist:\"{artist}\" album:\"{wanted}\"")), ("limit", "5".into())])
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    let mut url: Option<String> = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["title"].as_str().is_some_and(|t| plain(t) == wanted) && a["artist"]["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(artist)))
        .and_then(|a| deezer_image(&a["cover_xl"]));
    if url.is_none() {
        let found = net
            .musicbrainz("https://musicbrainz.org/ws/2/release-group/", &[("query", format!("releasegroup:\"{wanted}\" AND artist:\"{artist}\"")), ("fmt", "json".into()), ("limit", "5".into())])
            .await?;
        let group = found["release-groups"].as_array().into_iter().flatten().find(|g| g["title"].as_str().is_some_and(|t| plain(t) == wanted));
        if let Some(id) = group.and_then(|g| g["id"].as_str()) {
            url = Some(format!("https://coverartarchive.org/release-group/{id}/front-500"));
        }
    }
    let Some(url) = url else { return Ok(None) };
    let response = net.client.get(&url).send().await?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let bytes = response.bytes().await?.to_vec();
    // whatever came, a JPEG goes in the folder
    let jpeg = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let image = image::load_from_memory(&bytes)?.into_rgb8();
        let mut out = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&image)?;
        Ok(out.into_inner())
    })
    .await??;
    Ok(Some(jpeg))
}
