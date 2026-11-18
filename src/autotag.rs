//! Online lookups for album metadata and cover art.
//!
//! Sources, in order: Apple's iTunes Search API (no key, great artwork),
//! then MusicBrainz + Cover Art Archive as a fallback.

use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::time::Duration;

const USER_AGENT: &str = "easytag/0.1 (music tag editor)";

#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // artist/album/source describe the match; handy when debugging lookups
pub struct AlbumInfo {
    pub artist: String,
    pub album: String,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub cover: Option<Vec<u8>>,
    pub source: &'static str,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    Ok(agent.get(url).call()?.body_mut().read_to_string()?)
}

fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    Ok(agent
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(30 * 1024 * 1024)
        .read_to_vec()?)
}

/// Lowercase, alphanumerics only, for fuzzy comparisons.
fn norm(s: &str) -> String {
    let s = s.to_lowercase();
    let s = s.strip_prefix("the ").unwrap_or(&s);
    s.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Strip things stores append: "Album (Deluxe Edition)", "Album - Single".
fn base_title(s: &str) -> String {
    let mut s = s.to_string();
    for sep in [" (", " [", " - "] {
        if let Some(i) = s.find(sep)
            && i > 0
        {
            s.truncate(i);
        }
    }
    s
}

fn score(want_artist: &str, want_album: &str, artist: &str, album: &str) -> i32 {
    let (wa, wl, a, l) = (
        norm(want_artist),
        norm(want_album),
        norm(artist),
        norm(album),
    );
    let mut score = 0;
    if l == wl {
        score += 10;
    } else if norm(&base_title(album)) == norm(&base_title(want_album)) {
        score += 7;
    } else if !wl.is_empty() && (l.contains(&wl) || wl.contains(&l)) {
        score += 4;
    } else {
        return -100;
    }
    if wa.is_empty() || a == wa {
        score += 10;
    } else if a.contains(&wa) || wa.contains(&a) {
        score += 5;
    } else {
        return -100;
    }
    score
}

#[derive(Deserialize)]
struct ItunesResponse {
    results: Vec<ItunesAlbum>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesAlbum {
    artist_name: Option<String>,
    collection_name: Option<String>,
    artwork_url100: Option<String>,
    release_date: Option<String>,
    primary_genre_name: Option<String>,
}

fn itunes(agent: &ureq::Agent, artist: &str, album: &str) -> Result<Option<AlbumInfo>> {
    let term = format!("{artist} {album}");
    let url = format!(
        "https://itunes.apple.com/search?media=music&entity=album&limit=25&term={}",
        urlencoding::encode(&term)
    );
    let resp: ItunesResponse = serde_json::from_str(&get_text(agent, &url)?)?;
    let best = resp
        .results
        .into_iter()
        .filter_map(|r| {
            let s = score(
                artist,
                album,
                r.artist_name.as_deref().unwrap_or_default(),
                r.collection_name.as_deref().unwrap_or_default(),
            );
            (s > 0).then_some((s, r))
        })
        .max_by_key(|(s, _)| *s);
    let Some((_, r)) = best else {
        return Ok(None);
    };
    let cover = match &r.artwork_url100 {
        Some(u) => {
            let big = u.replace("100x100bb", "1200x1200bb");
            get_bytes(agent, &big).or_else(|_| get_bytes(agent, u)).ok()
        }
        None => None,
    };
    Ok(Some(AlbumInfo {
        artist: r.artist_name.unwrap_or_default(),
        album: r.collection_name.unwrap_or_default(),
        year: r.release_date.and_then(|d| d.get(..4).map(str::to_string)),
        genre: r.primary_genre_name,
        cover,
        source: "iTunes",
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct MbResponse {
    release_groups: Vec<MbReleaseGroup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct MbReleaseGroup {
    id: String,
    title: String,
    first_release_date: Option<String>,
    artist_credit: Option<Vec<MbCredit>>,
}

#[derive(Deserialize)]
struct MbCredit {
    name: String,
}

fn lucene_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

fn musicbrainz(agent: &ureq::Agent, artist: &str, album: &str) -> Result<Option<AlbumInfo>> {
    let query = format!(
        "releasegroup:\"{}\" AND artist:\"{}\"",
        lucene_escape(album),
        lucene_escape(artist)
    );
    let url = format!(
        "https://musicbrainz.org/ws/2/release-group/?fmt=json&limit=10&query={}",
        urlencoding::encode(&query)
    );
    let resp: MbResponse = serde_json::from_str(&get_text(agent, &url)?)?;
    let candidates: Vec<_> = resp
        .release_groups
        .into_iter()
        .filter_map(|g| {
            let credit = g
                .artist_credit
                .as_ref()
                .and_then(|c| c.first())
                .map(|c| c.name.clone())
                .unwrap_or_default();
            let s = score(artist, album, &credit, &g.title);
            (s > 0).then_some((s, credit, g))
        })
        .collect();
    let mut candidates = candidates;
    candidates.sort_by_key(|(s, _, _)| -s);
    for (_, credit, g) in candidates.into_iter().take(3) {
        let url = format!(
            "https://coverartarchive.org/release-group/{}/front-1200",
            g.id
        );
        let cover = get_bytes(agent, &url)
            .or_else(|_| {
                get_bytes(
                    agent,
                    &format!("https://coverartarchive.org/release-group/{}/front", g.id),
                )
            })
            .ok();
        if cover.is_some() {
            return Ok(Some(AlbumInfo {
                artist: credit,
                album: g.title,
                year: g
                    .first_release_date
                    .and_then(|d| d.get(..4).map(str::to_string)),
                genre: None,
                cover,
                source: "MusicBrainz",
            }));
        }
    }
    Ok(None)
}

/// Look up an album online. Returns the best match found that has artwork,
/// or an error describing why nothing was found.
pub fn lookup_album(artist: &str, album: &str) -> Result<AlbumInfo> {
    let agent = agent();
    let mut errors = vec![];
    let mut partial = None;
    match itunes(&agent, artist, album) {
        Ok(Some(info)) if info.cover.is_some() => return Ok(info),
        Ok(Some(info)) => partial = Some(info),
        Ok(None) => {}
        Err(e) => errors.push(format!("iTunes: {e}")),
    }
    match musicbrainz(&agent, artist, album) {
        Ok(Some(mut info)) => {
            if let Some(p) = &partial {
                info.genre = info.genre.or(p.genre.clone());
            }
            return Ok(info);
        }
        Ok(None) => {}
        Err(e) => errors.push(format!("MusicBrainz: {e}")),
    }
    if let Some(p) = partial {
        return Ok(p);
    }
    if errors.is_empty() {
        Err(anyhow!("no match for \"{artist} – {album}\""))
    } else {
        Err(anyhow!(errors.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring() {
        assert!(score("Radiohead", "OK Computer", "Radiohead", "OK Computer") > 0);
        assert!(
            score(
                "Radiohead",
                "OK Computer",
                "Radiohead",
                "OK Computer OKNOTOK 1997 2017"
            ) > 0
        );
        assert!(
            score(
                "Beatles",
                "Abbey Road",
                "The Beatles",
                "Abbey Road (Remastered)"
            ) > 0
        );
        assert!(score("Radiohead", "OK Computer", "Coldplay", "OK Computer") < 0);
    }
}

#[cfg(test)]
mod online_tests {
    #[test]
    #[ignore]
    fn finds_real_albums() {
        for (artist, album) in [
            ("Radiohead", "OK Computer"),
            ("Daft Punk", "Discovery"),
            ("The Beatles", "Abbey Road"),
        ] {
            let info = super::lookup_album(artist, album).unwrap();
            let cover = info.cover.expect("cover");
            println!(
                "{artist} – {album}: {} via {} year={:?} genre={:?} {} KB",
                info.album,
                info.source,
                info.year,
                info.genre,
                cover.len() / 1024
            );
            assert!(cover.len() > 10_000);
        }
        assert!(super::lookup_album("Unknown Band", "Nonexistent Album Zzq").is_err());
    }
}
