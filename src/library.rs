//! Walking the music folder on disk.

use crate::tags::{Track, is_audio};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// What a folder represents in the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Artist,
    Album,
    Neither,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Artist => "Artist",
            Role::Album => "Album",
            Role::Neither => "Neither",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Role::Artist => "icons/user-round.svg",
            Role::Album => "icons/disc-3.svg",
            Role::Neither => "icons/folder.svg",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FolderEntry {
    pub name: String,
    pub path: PathBuf,
    /// Whether this folder has subfolders of its own (shows a chevron).
    pub has_children: bool,
    /// Role guessed from the folder's contents.
    pub auto_role: Role,
}

fn has_audio_directly(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|r| r.flatten().any(|e| is_audio(&e.path())))
        .unwrap_or(false)
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && visible(p))
                .collect()
        })
        .unwrap_or_default()
}

/// Guess a folder's role from what's inside it:
/// songs (or `CD 1`-style disc folders) -> Album; album folders -> Artist.
pub fn guess_role(dir: &Path) -> Role {
    if is_disc_folder(dir) {
        return Role::Neither;
    }
    let children = child_dirs(dir);
    let with_audio: Vec<_> = children.iter().filter(|c| has_audio_directly(c)).collect();
    // Album folders inside make this an artist, even with a few loose songs
    // next to them.
    if with_audio.iter().any(|c| !is_disc_folder(c)) {
        return Role::Artist;
    }
    if has_audio_directly(dir) || !with_audio.is_empty() {
        return Role::Album;
    }
    // An artist whose albums are split into disc folders.
    if children.iter().any(|c| {
        child_dirs(c)
            .iter()
            .any(|d| is_disc_folder(d) && has_audio_directly(d))
    }) {
        return Role::Artist;
    }
    Role::Neither
}

fn visible(path: &Path) -> bool {
    !path
        .file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(true)
}

fn sort_key(s: &str) -> String {
    let lower = s.to_lowercase();
    lower
        .strip_prefix("the ")
        .map(str::to_string)
        .unwrap_or(lower)
}

/// Immediate subfolders of `dir`, sorted Finder-style.
pub fn subfolders(dir: &Path) -> Vec<FolderEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<FolderEntry> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && visible(p))
        .map(|path| {
            let has_children = std::fs::read_dir(&path)
                .map(|r| r.flatten().any(|e| e.path().is_dir() && visible(&e.path())))
                .unwrap_or(false);
            FolderEntry {
                auto_role: guess_role(&path),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path,
                has_children,
            }
        })
        .collect();
    out.sort_by_cached_key(|e| sort_key(&e.name));
    out
}

/// All audio files under `dir`, recursively, in path order.
pub fn audio_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in read.flatten() {
            let p = entry.path();
            if !visible(&p) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if is_audio(&p) {
                out.push(p);
            }
        }
    }
    out.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
    out
}

/// What the folder layout says about a file: `.../Artist/Album/[Disc 1/]song.ext`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathInfo {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_year: Option<String>,
    /// The folder playing the Album role for this file.
    pub album_dir: Option<PathBuf>,
    /// The user picked these roles (vs. easytag guessing them).
    pub artist_explicit: bool,
    pub album_explicit: bool,
    pub disc: Option<u32>,
    pub track: Option<u32>,
    pub title: Option<String>,
}

fn disc_folder(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    for prefix in ["cd", "disc", "disk"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = rest.trim_start_matches([' ', '_', '-', '.']);
            if let Ok(n) = rest.parse() {
                return Some(n);
            }
        }
    }
    None
}

/// Strip a leading track number like `01 `, `01 - `, `1. `, `01_`.
/// Also strips an `Artist - ` prefix that matches the artist folder.
pub fn parse_filename(stem: &str, artist: Option<&str>) -> (Option<u32>, String) {
    let s = stem.trim();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    let (track, mut rest) = if !digits.is_empty() && digits.len() <= 3 {
        let after = &s[digits.len()..];
        // Only treat as a track number if followed by a separator (avoid "1999 Song").
        if after.is_empty() || after.starts_with([' ', '.', '-', '_', ')']) {
            (
                digits.parse().ok(),
                after
                    .trim_start_matches([' ', '.', '-', '_', ')'])
                    .to_string(),
            )
        } else {
            (None, s.to_string())
        }
    } else {
        (None, s.to_string())
    };
    if let Some(artist) = artist {
        let prefix = format!("{} - ", artist.to_lowercase());
        if rest.to_lowercase().starts_with(&prefix) {
            rest = rest[prefix.len()..].to_string();
        }
    }
    let rest = rest.replace('_', " ").trim().to_string();
    (
        track,
        if rest.is_empty() {
            stem.to_string()
        } else {
            rest
        },
    )
}

/// Works out each file's artist/album from folder roles: the ones the user
/// chose, falling back to guesses from folder contents.
pub struct Resolver {
    root: PathBuf,
    explicit: HashMap<PathBuf, Role>,
    cache: HashMap<PathBuf, Role>,
}

impl Resolver {
    pub fn new(root: PathBuf, explicit: HashMap<PathBuf, Role>) -> Self {
        Self {
            root,
            explicit,
            cache: HashMap::new(),
        }
    }

    pub fn role(&mut self, dir: &Path) -> Role {
        if let Some(r) = self.explicit.get(dir) {
            return *r;
        }
        if let Some(r) = self.cache.get(dir) {
            return *r;
        }
        let r = guess_role(dir);
        self.cache.insert(dir.to_path_buf(), r);
        r
    }

    pub fn info(&mut self, file: &Path) -> PathInfo {
        let mut info = PathInfo::default();
        // Folders between the root (exclusive) and the file, nearest first.
        let mut dirs: Vec<PathBuf> = vec![];
        let mut cur = file.parent();
        while let Some(d) = cur {
            if d == self.root || !d.starts_with(&self.root) {
                break;
            }
            dirs.push(d.to_path_buf());
            cur = d.parent();
        }

        if let Some(nearest) = dirs.first()
            && let Some(name) = nearest.file_name()
            && let Some(n) = disc_folder(&name.to_string_lossy())
        {
            info.disc = Some(n);
        }

        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let roles: Vec<Role> = dirs.iter().map(|d| self.role(d)).collect();
        let album_ix = roles.iter().position(|r| *r == Role::Album);
        let artist_ix = match album_ix {
            Some(a) => roles[a + 1..]
                .iter()
                .position(|r| *r == Role::Artist)
                .map(|i| i + a + 1),
            None => roles.iter().position(|r| *r == Role::Artist),
        };

        if let Some(a) = album_ix {
            let raw = name(&dirs[a]);
            info.album_dir = Some(dirs[a].clone());
            info.album_explicit = self.explicit.contains_key(&dirs[a]);
            info.album = Some(clean_album(&raw));
            info.album_year = folder_year(&raw);
            // "Artist - Album" folders with no artist folder above them.
            if artist_ix.is_none()
                && let Some((artist, album)) = split_artist_album(&raw)
            {
                info.artist = Some(artist);
                info.album = Some(album);
            }
        }
        if let Some(i) = artist_ix {
            info.artist = Some(name(&dirs[i]));
            info.artist_explicit = self.explicit.contains_key(&dirs[i]);
        }

        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (track, title) = parse_filename(&stem, info.artist.as_deref());
        info.track = track;
        info.title = Some(title);
        info
    }

    /// Group tracks by (artist, album) for online lookups.
    pub fn group(
        &mut self,
        tracks: Vec<Track>,
    ) -> BTreeMap<(String, String), Vec<(Track, PathInfo)>> {
        let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for t in tracks {
            let info = self.info(&t.path);
            let key = (
                info.artist.clone().unwrap_or_default(),
                info.album.clone().unwrap_or_default(),
            );
            groups.entry(key).or_default().push((t, info));
        }
        groups
    }
}

/// "Pusha T - Blowbama" -> ("Pusha T", "Blowbama"). Not "2001 - Discovery".
pub fn split_artist_album(name: &str) -> Option<(String, String)> {
    let (a, b) = name.split_once(" - ")?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() || (a.len() == 4 && a.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some((a.to_string(), clean_album(b)))
}

/// "2011 - Album Name" / "Album Name (2011)" -> "Album Name"
pub fn clean_album(name: &str) -> String {
    let s = name.trim();
    let year_prefix = s.len() > 7
        && s[..4].chars().all(|c| c.is_ascii_digit())
        && s[4..].starts_with([' ', '-', '.']);
    let s = if year_prefix {
        s[4..].trim_start_matches([' ', '-', '.'])
    } else {
        s
    };
    let s = if s.ends_with(')') && s.len() > 7 {
        let open = s.len() - 6;
        if s.is_char_boundary(open)
            && s[open..].starts_with('(')
            && s[open + 1..s.len() - 1].chars().all(|c| c.is_ascii_digit())
        {
            s[..open].trim_end()
        } else {
            s
        }
    } else {
        s
    };
    s.to_string()
}

/// A year in the album folder name, if any.
pub fn folder_year(name: &str) -> Option<String> {
    let bytes: Vec<char> = name.chars().collect();
    for w in bytes.windows(4) {
        let s: String = w.iter().collect();
        if let Ok(y) = s.parse::<u32>()
            && (1900..=2100).contains(&y)
        {
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "easytag-lib-{}-{}",
            std::process::id(),
            files.len() + files[0].len()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        }
        root
    }

    #[test]
    fn infers_from_layout() {
        let root = lib(&[
            "Music/Radiohead/OK Computer/02 - Paranoid Android.mp3",
            "Music/Radiohead/Kid A/01 - Everything.mp3",
            "Artist/2001 - Big Album/CD 2/1. Song.flac",
            "Pusha T - Blowbama/01 - Track.mp3",
            "my comps/road trip/song.mp3",
            "Ye/Loose Single.mp3",
            "Ye/Donda/01 - Song.mp3",
        ]);
        let mut r = Resolver::new(root.clone(), HashMap::new());
        assert_eq!(r.role(&root.join("Music")), Role::Neither);
        assert_eq!(r.role(&root.join("Music/Radiohead")), Role::Artist);
        assert_eq!(
            r.role(&root.join("Music/Radiohead/OK Computer")),
            Role::Album
        );
        assert_eq!(r.role(&root.join("Artist")), Role::Artist);
        assert_eq!(r.role(&root.join("Artist/2001 - Big Album")), Role::Album);
        // Loose songs next to album folders: still an artist.
        assert_eq!(r.role(&root.join("Ye")), Role::Artist);

        let info = r.info(&root.join("Music/Radiohead/OK Computer/02 - Paranoid Android.mp3"));
        assert_eq!(info.artist.as_deref(), Some("Radiohead"));
        assert_eq!(info.album.as_deref(), Some("OK Computer"));
        assert_eq!(info.track, Some(2));
        assert_eq!(info.title.as_deref(), Some("Paranoid Android"));

        let info = r.info(&root.join("Artist/2001 - Big Album/CD 2/1. Song.flac"));
        assert_eq!(info.artist.as_deref(), Some("Artist"));
        assert_eq!(info.album.as_deref(), Some("Big Album"));
        assert_eq!(info.album_year.as_deref(), Some("2001"));
        assert_eq!(info.disc, Some(2));
        assert_eq!(info.track, Some(1));

        let info = r.info(&root.join("Pusha T - Blowbama/01 - Track.mp3"));
        assert_eq!(info.artist.as_deref(), Some("Pusha T"));
        assert_eq!(info.album.as_deref(), Some("Blowbama"));

        // Guessed wrong ("my comps" looks like an artist); the user overrides it.
        let info = r.info(&root.join("my comps/road trip/song.mp3"));
        assert_eq!(info.artist.as_deref(), Some("my comps"));
        let mut r = Resolver::new(
            root.clone(),
            HashMap::from([(root.join("my comps"), Role::Neither)]),
        );
        let info = r.info(&root.join("my comps/road trip/song.mp3"));
        assert_eq!(info.artist, None);
        assert_eq!(info.album.as_deref(), Some("road trip"));

        // Explicit Album on an artist-looking folder; Neither on an album.
        let mut r = Resolver::new(
            root.clone(),
            HashMap::from([
                (root.join("Music/Radiohead"), Role::Album),
                (root.join("Music/Radiohead/Kid A"), Role::Neither),
            ]),
        );
        let info = r.info(&root.join("Music/Radiohead/Kid A/01 - Everything.mp3"));
        assert_eq!(info.album.as_deref(), Some("Radiohead"));
        assert_eq!(info.artist, None);
        assert!(info.album_explicit);

        // Guessed roles are marked as such (auto-tag only fills blanks with them).
        let mut r = Resolver::new(root.clone(), HashMap::new());
        let info = r.info(&root.join("Music/Radiohead/OK Computer/02 - Paranoid Android.mp3"));
        assert!(!info.artist_explicit && !info.album_explicit);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn filenames() {
        assert_eq!(
            parse_filename("1999 Prince", None),
            (None, "1999 Prince".into())
        );
        assert_eq!(
            parse_filename("07_Some_Song", None),
            (Some(7), "Some Song".into())
        );
        assert_eq!(
            parse_filename("03 - Muse - Hysteria", Some("Muse")),
            (Some(3), "Hysteria".into())
        );
        assert_eq!(clean_album("Album (2004)"), "Album");
        assert_eq!(split_artist_album("2001 - Discovery"), None);
        assert_eq!(
            split_artist_album("Pusha T - Blowbama"),
            Some(("Pusha T".into(), "Blowbama".into()))
        );
    }
}

// ---------------------------------------------------------------------------
// Folder art (cover.jpg and friends)

const ART_STEMS: &[&str] = &["cover", "folder", "front", "album"];
const ART_EXTS: &[&str] = &["jpg", "jpeg", "png"];

/// The album-art image file in `dir` (cover.jpg, folder.png, …), if any.
pub fn folder_art(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_lowercase();
            let ext = p.extension()?.to_str()?.to_lowercase();
            let rank = ART_STEMS.iter().position(|s| *s == stem)?;
            ART_EXTS.contains(&ext.as_str()).then_some((rank, p))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(_, p)| p)
}

pub fn is_disc_folder(dir: &Path) -> bool {
    dir.file_name()
        .is_some_and(|n| disc_folder(&n.to_string_lossy()).is_some())
}

/// Art for the album `file` belongs to: cover.jpg etc. in the song's own
/// folder, or the one above when the song sits in a `CD 1`-style disc folder.
/// Never higher: an artist's folder art doesn't belong to its albums.
pub fn album_folder_art(file: &Path) -> Option<PathBuf> {
    let parent = file.parent()?;
    folder_art(parent).or_else(|| {
        is_disc_folder(parent)
            .then(|| parent.parent().and_then(folder_art))
            .flatten()
    })
}

/// Songs that belong to the album in `dir`: the ones directly inside it and in
/// its disc folders. Other subfolders are separate albums and are left alone.
pub fn album_songs(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for d in std::iter::once(dir.to_path_buf())
        .chain(child_dirs(dir).into_iter().filter(|d| is_disc_folder(d)))
    {
        if let Ok(r) = std::fs::read_dir(&d) {
            out.extend(r.flatten().map(|e| e.path()).filter(|p| is_audio(p)));
        }
    }
    out.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
    out
}

/// Save `bytes` as `dir/cover.jpg` (converting other formats to JPEG).
pub fn save_folder_art(dir: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let jpeg = if image::guess_format(bytes)? == image::ImageFormat::Jpeg {
        bytes.to_vec()
    } else {
        let img = image::load_from_memory(bytes)?.to_rgb8();
        let mut out = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92).encode_image(&img)?;
        out.into_inner()
    };
    let target = dir.join("cover.jpg");
    let tmp = dir.join(".cover.jpg.easytag-tmp");
    std::fs::write(&tmp, &jpeg)?;
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

/// The album folder a song belongs to: its own folder, or the one above a
/// `CD 1`-style disc folder.
pub fn album_dir_of(file: &Path) -> Option<&Path> {
    let parent = file.parent()?;
    if is_disc_folder(parent) {
        parent.parent()
    } else {
        Some(parent)
    }
}

/// A folder's own art: its cover file, or (for album folders only) art
/// embedded in its songs. Never borrowed from albums inside it.
pub fn folder_art_bytes(dir: &Path, is_album: bool) -> Option<Vec<u8>> {
    match folder_art(dir) {
        Some(p) => std::fs::read(p).ok(),
        None if !is_album => None,
        None => album_songs(dir)
            .iter()
            .take(3)
            .find_map(|f| crate::tags::read_cover(f)),
    }
}

/// Shrink image bytes to fit `size`×`size`, as PNG.
pub fn png_thumbnail(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.thumbnail(size, size)
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

/// A small PNG thumbnail of a folder's own art (see [`folder_art_bytes`]).
pub fn folder_thumbnail(dir: &Path, is_album: bool) -> Option<Vec<u8>> {
    png_thumbnail(&folder_art_bytes(dir, is_album)?, 96)
}

#[cfg(test)]
mod art_tests {
    use super::*;

    #[test]
    fn finds_and_saves_folder_art() {
        let dir = std::env::temp_dir().join(format!("easytag-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let album = dir.join("Artist/Album");
        std::fs::create_dir_all(&album).unwrap();
        assert_eq!(folder_art(&album), None);
        std::fs::write(album.join("Folder.PNG"), b"x").unwrap();
        assert_eq!(folder_art(&album), Some(album.join("Folder.PNG")));

        // A real (tiny) PNG gets converted to cover.jpg, which then wins.
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(4, 4)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let saved = save_folder_art(&album, png.get_ref()).unwrap();
        assert_eq!(saved, album.join("cover.jpg"));
        assert_eq!(folder_art(&album), Some(album.join("cover.jpg")));
        let bytes = std::fs::read(&saved).unwrap();
        assert_eq!(
            image::guess_format(&bytes).unwrap(),
            image::ImageFormat::Jpeg
        );

        let song = album.join("01 - Song.mp3");
        assert_eq!(album_folder_art(&song), Some(album.join("cover.jpg")));
        let disc_song = album.join("CD 2/01 - Song.mp3");
        assert_eq!(album_folder_art(&disc_song), Some(album.join("cover.jpg")));
        assert!(folder_thumbnail(&album, true).is_some());

        // Art never flows between an artist folder and its albums.
        let other = dir.join("Artist/Other Album");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(dir.join("Artist/cover.jpg"), &bytes).unwrap();
        assert_eq!(album_folder_art(&other.join("01 - x.mp3")), None);
        std::fs::remove_file(dir.join("Artist/cover.jpg")).unwrap();
        assert!(folder_thumbnail(&dir.join("Artist"), false).is_none());

        // Album songs: its own + disc folders, not other albums inside it.
        std::fs::create_dir_all(album.join("CD 2")).unwrap();
        std::fs::write(album.join("CD 2/01 - a.mp3"), b"").unwrap();
        std::fs::write(album.join("02 - b.mp3"), b"").unwrap();
        std::fs::create_dir_all(album.join("Bonus Album")).unwrap();
        std::fs::write(album.join("Bonus Album/01 - c.mp3"), b"").unwrap();
        assert_eq!(album_songs(&album).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod real_library_check {
    use super::*;

    /// Read-only: EASYTAG_LIB=/path/to/library cargo test real_library -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_library() {
        let root = PathBuf::from(std::env::var("EASYTAG_LIB").expect("EASYTAG_LIB"));
        for e in subfolders(&root) {
            let role = guess_role(&e.path);
            let thumb = folder_thumbnail(&e.path, role == Role::Album).is_some();
            println!("{:<24} {:<8} thumbnail={}", e.name, role.label(), thumb);
        }
    }
}

// ---------------------------------------------------------------------------
// Trash

/// Move items to the Trash and return where each one ended up (same order),
/// so the move can be undone. Uses Finder, so "Put Back" works too.
#[cfg(not(test))]
pub fn move_to_trash(items: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    const SCRIPT: &str = r#"
on run argv
    set theItems to {}
    repeat with p in argv
        set end of theItems to ((POSIX file (p as text)) as alias)
    end repeat
    tell application "Finder" to set trashed to delete theItems
    if class of trashed is not list then set trashed to {trashed}
    set out to {}
    repeat with t in trashed
        set end of out to POSIX path of (t as alias)
    end repeat
    set AppleScript's text item delimiters to linefeed
    return out as text
end run
"#;
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(SCRIPT)
        .args(items)
        .output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| PathBuf::from(l.trim_end_matches('/')))
        .collect();
    if paths.len() != items.len() {
        anyhow::bail!("Finder reported {} of {} items", paths.len(), items.len());
    }
    Ok(paths)
}

/// Tests use a scratch folder instead of the real Trash.
#[cfg(test)]
pub fn move_to_trash(items: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let trash = std::env::temp_dir().join(format!("easytag-test-trash-{}", std::process::id()));
    std::fs::create_dir_all(&trash)?;
    items
        .iter()
        .map(|p| {
            let mut to = trash.join(p.file_name().unwrap_or_default());
            let mut n = 2;
            while to.exists() {
                to = trash.join(format!(
                    "{} {n}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                ));
                n += 1;
            }
            std::fs::rename(p, &to)?;
            Ok(to)
        })
        .collect()
}
