//! Reading and writing audio tags (via `lofty`).

use anyhow::{Context as _, Result, anyhow};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFile};
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage as _, OpusFile, SpeexFile, VorbisFile};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use std::path::{Path, PathBuf};

/// The editable fields shown in the tag panel, in Mp3tag's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Title,
    Artist,
    Album,
    Year,
    Track,
    Genre,
    Comment,
    AlbumArtist,
    Composer,
    Disc,
    Filename,
}

impl Field {
    pub const ALL: [Field; 11] = [
        Field::Title,
        Field::Artist,
        Field::Album,
        Field::Year,
        Field::Track,
        Field::Genre,
        Field::Comment,
        Field::AlbumArtist,
        Field::Composer,
        Field::Disc,
        Field::Filename,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Artist => "Artist",
            Field::Album => "Album",
            Field::Year => "Year",
            Field::Track => "Track",
            Field::Genre => "Genre",
            Field::Comment => "Comment",
            Field::AlbumArtist => "Album Artist",
            Field::Composer => "Composer",
            Field::Disc => "Disc",
            Field::Filename => "Filename",
        }
    }
}

/// Everything we display about one audio file.
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub track: String,
    pub genre: String,
    pub comment: String,
    pub album_artist: String,
    pub composer: String,
    pub disc: String,
    pub duration_secs: u64,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub format: String,
    pub tag_type: String,
    pub has_cover: bool,
    /// iTunes-style parental advisory flag.
    pub explicit: bool,
    pub error: Option<String>,
}

impl Track {
    pub fn get(&self, field: Field) -> String {
        match field {
            Field::Title => self.title.clone(),
            Field::Artist => self.artist.clone(),
            Field::Album => self.album.clone(),
            Field::Year => self.year.clone(),
            Field::Track => self.track.clone(),
            Field::Genre => self.genre.clone(),
            Field::Comment => self.comment.clone(),
            Field::AlbumArtist => self.album_artist.clone(),
            Field::Composer => self.composer.clone(),
            Field::Disc => self.disc.clone(),
            Field::Filename => self.file_stem(),
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn file_stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn length(&self) -> String {
        let s = self.duration_secs;
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    }
}

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "mp4", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "ape",
    "wv", "mpc", "spx",
];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn tag_of(file: &TaggedFile) -> Option<&Tag> {
    file.primary_tag().or_else(|| file.first_tag())
}

fn pair(n: Option<u32>, total: Option<u32>) -> String {
    match (n, total) {
        (Some(n), Some(t)) => format!("{n}/{t}"),
        (Some(n), None) => n.to_string(),
        (None, Some(t)) => format!("/{t}"),
        (None, None) => String::new(),
    }
}

/// Read a file's tags. Never fails: unreadable files come back with `error` set.
pub fn read_track(path: &Path) -> Track {
    let mut track = Track {
        path: path.to_path_buf(),
        format: path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default(),
        ..Default::default()
    };
    let file = match lofty::read_from_path(path) {
        Ok(f) => f,
        Err(e) => {
            track.error = Some(e.to_string());
            return track;
        }
    };
    let props = file.properties();
    track.duration_secs = props.duration().as_secs();
    track.bitrate = props.audio_bitrate();
    track.sample_rate = props.sample_rate();

    if let Some(tag) = tag_of(&file) {
        let s = |k: ItemKey| tag.get_string(k).unwrap_or_default().to_string();
        track.tag_type = format!("{:?}", tag.tag_type());
        track.title = s(ItemKey::TrackTitle);
        track.artist = s(ItemKey::TrackArtist);
        track.album = s(ItemKey::AlbumTitle);
        track.album_artist = s(ItemKey::AlbumArtist);
        track.genre = s(ItemKey::Genre);
        track.comment = s(ItemKey::Comment);
        track.composer = s(ItemKey::Composer);
        track.year = tag
            .get_string(ItemKey::RecordingDate)
            .or_else(|| tag.get_string(ItemKey::Year))
            .or_else(|| tag.get_string(ItemKey::OriginalReleaseDate))
            .unwrap_or_default()
            .to_string();
        track.track = pair(tag.track(), tag.track_total());
        track.disc = pair(tag.disk(), tag.disk_total());
        track.has_cover = !tag.pictures().is_empty();
        track.explicit = tag
            .get_string(ItemKey::ParentalAdvisory)
            .is_some_and(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("explicit"));
    }
    if let Some(explicit) = vorbis_explicit(path, file.file_type()) {
        track.explicit = explicit;
    }
    track
}

/// Front cover (or the first picture) embedded in the file.
pub fn read_cover(path: &Path) -> Option<Vec<u8>> {
    let file = lofty::read_from_path(path).ok()?;
    let tag = tag_of(&file)?;
    let pics = tag.pictures();
    pics.iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pics.first())
        .map(|p| p.data().to_vec())
}

/// A set of changes to apply to a file. `None` = leave untouched;
/// `Some("")` = remove the field.
#[derive(Clone, Debug, Default)]
pub struct Edit {
    pub fields: Vec<(Field, String)>,
    pub cover: Option<CoverEdit>,
    pub explicit: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum CoverEdit {
    Set(Vec<u8>),
    Remove,
}

impl Edit {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.cover.is_none() && self.explicit.is_none()
    }
}

fn parse_pair(s: &str) -> (Option<u32>, Option<u32>) {
    let s = s.trim();
    let mut parts = s.splitn(2, '/');
    let a = parts.next().and_then(|p| p.trim().parse().ok());
    let b = parts.next().and_then(|p| p.trim().parse().ok());
    (a, b)
}

pub fn mime_for(bytes: &[u8]) -> MimeType {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        MimeType::Png
    } else if bytes.starts_with(b"GIF8") {
        MimeType::Gif
    } else if bytes.starts_with(b"BM") {
        MimeType::Bmp
    } else {
        MimeType::Jpeg
    }
}

fn set_text(tag: &mut Tag, key: ItemKey, value: &str) {
    if value.trim().is_empty() {
        tag.remove_key(key);
    } else {
        tag.insert_text(key, value.to_string());
    }
}

/// Apply text-field and explicit edits to a generic tag.
fn apply_fields(tag: &mut Tag, edit: &Edit) {
    for (field, value) in &edit.fields {
        match field {
            Field::Title => set_text(tag, ItemKey::TrackTitle, value),
            Field::Artist => set_text(tag, ItemKey::TrackArtist, value),
            Field::Album => set_text(tag, ItemKey::AlbumTitle, value),
            Field::AlbumArtist => set_text(tag, ItemKey::AlbumArtist, value),
            Field::Genre => set_text(tag, ItemKey::Genre, value),
            Field::Comment => set_text(tag, ItemKey::Comment, value),
            Field::Composer => set_text(tag, ItemKey::Composer, value),
            Field::Year => {
                tag.remove_key(ItemKey::Year);
                set_text(tag, ItemKey::RecordingDate, value);
            }
            Field::Track => {
                let (n, t) = parse_pair(value);
                tag.remove_track();
                tag.remove_track_total();
                if let Some(n) = n {
                    tag.set_track(n);
                }
                if let Some(t) = t {
                    tag.set_track_total(t);
                }
            }
            Field::Disc => {
                let (n, t) = parse_pair(value);
                tag.remove_disk();
                tag.remove_disk_total();
                if let Some(n) = n {
                    tag.set_disk(n);
                }
                if let Some(t) = t {
                    tag.set_disk_total(t);
                }
            }
            Field::Filename => {}
        }
    }
    match edit.explicit {
        Some(true) => {
            tag.insert_text(ItemKey::ParentalAdvisory, "1".into());
        }
        Some(false) => tag.remove_key(ItemKey::ParentalAdvisory),
        None => {}
    }
}

fn cover_picture(bytes: &[u8]) -> Picture {
    Picture::unchecked(bytes.to_vec())
        .pic_type(PictureType::CoverFront)
        .mime_type(mime_for(bytes))
        .build()
}

fn apply_cover(tag: &mut Tag, cover: &Option<CoverEdit>) {
    match cover {
        Some(CoverEdit::Set(bytes)) => {
            tag.remove_picture_type(PictureType::CoverFront);
            tag.remove_picture_type(PictureType::Other);
            tag.push_picture(cover_picture(bytes));
        }
        Some(CoverEdit::Remove) => {
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
        }
        None => {}
    }
}

/// Vorbis comments have no standard key for "explicit"; iTunes and most
/// taggers use this one.
const VORBIS_ADVISORY: &str = "ITUNESADVISORY";

/// Edit a Vorbis comment block in place. Keys easytag doesn't know about
/// (ReplayGain, custom fields, …) are carried through untouched.
fn edit_vorbis(vc: &mut VorbisComments, edit: &Edit, with_pictures: bool) {
    let (remainder, mut tag) = std::mem::take(vc).split_tag();
    apply_fields(&mut tag, edit);
    if with_pictures {
        apply_cover(&mut tag, &edit.cover);
    }
    *vc = remainder.merge_tag(tag);
    match edit.explicit {
        Some(true) => vc.insert(VORBIS_ADVISORY.into(), "1".into()),
        Some(false) => vc.remove(VORBIS_ADVISORY).for_each(drop),
        None => {}
    }
}

fn write_flac(path: &Path, edit: &Edit) -> Result<()> {
    let mut flac = FlacFile::read_from(&mut std::fs::File::open(path)?, ParseOptions::new())?;
    let mut vc = flac.remove_vorbis_comments().unwrap_or_default();
    // FLAC keeps pictures in their own blocks, owned by the file.
    edit_vorbis(&mut vc, edit, false);
    flac.set_vorbis_comments(vc);
    match &edit.cover {
        Some(CoverEdit::Set(bytes)) => {
            flac.remove_picture_type(PictureType::CoverFront);
            flac.remove_picture_type(PictureType::Other);
            flac.insert_picture(cover_picture(bytes), None)?;
        }
        Some(CoverEdit::Remove) => {
            while !flac.pictures().is_empty() {
                flac.remove_picture(0);
            }
        }
        None => {}
    }
    flac.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

macro_rules! write_ogg {
    ($ty:ty, $path:expr, $edit:expr) => {{
        let mut file = <$ty>::read_from(&mut std::fs::File::open($path)?, ParseOptions::new())?;
        edit_vorbis(file.vorbis_comments_mut(), $edit, true);
        file.save_to_path($path, WriteOptions::default())?;
    }};
}

fn write_generic(path: &Path, edit: &Edit) -> Result<()> {
    let mut file = lofty::read_from_path(path)?;
    let tag_type = file.primary_tag_type();
    if file.primary_tag().is_none() {
        // Carry over anything from a secondary tag (e.g. an ID3v1-only MP3).
        let mut new_tag = Tag::new(tag_type);
        if let Some(existing) = file.first_tag() {
            for item in existing.items() {
                new_tag.insert(item.clone());
            }
            for pic in existing.pictures() {
                new_tag.push_picture(pic.clone());
            }
        }
        file.insert_tag(new_tag);
    }
    let tag = file
        .primary_tag_mut()
        .ok_or_else(|| anyhow!("this format can't hold tags"))?;
    apply_fields(tag, edit);
    apply_cover(tag, &edit.cover);
    fix_languages(tag);
    tag.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

fn file_type(path: &Path) -> Option<FileType> {
    lofty::probe::Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .file_type()
}

/// Read the explicit flag straight from Vorbis comments (FLAC/OGG).
fn vorbis_explicit(path: &Path, ft: FileType) -> Option<bool> {
    let opts = ParseOptions::new().read_properties(false);
    let mut f = std::fs::File::open(path).ok()?;
    let value = match ft {
        FileType::Flac => FlacFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()?
            .get(VORBIS_ADVISORY)
            .map(str::to_string),
        FileType::Vorbis => VorbisFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()
            .get(VORBIS_ADVISORY)
            .map(str::to_string),
        FileType::Opus => OpusFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()
            .get(VORBIS_ADVISORY)
            .map(str::to_string),
        FileType::Speex => SpeexFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()
            .get(VORBIS_ADVISORY)
            .map(str::to_string),
        _ => return None,
    };
    Some(value.is_some_and(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("explicit")))
}

/// Apply an edit to a file on disk. Returns the file's (possibly renamed) path.
pub fn write_track(path: &Path, edit: &Edit) -> Result<PathBuf> {
    let has_tag_edits = edit.fields.iter().any(|(f, _)| *f != Field::Filename)
        || edit.cover.is_some()
        || edit.explicit.is_some();

    if has_tag_edits {
        let result = match file_type(path) {
            Some(FileType::Flac) => write_flac(path, edit),
            Some(FileType::Vorbis) => (|| {
                write_ogg!(VorbisFile, path, edit);
                Ok(())
            })(),
            Some(FileType::Opus) => (|| {
                write_ogg!(OpusFile, path, edit);
                Ok(())
            })(),
            Some(FileType::Speex) => (|| {
                write_ogg!(SpeexFile, path, edit);
                Ok(())
            })(),
            _ => write_generic(path, edit),
        };
        result.with_context(|| format!("saving {}", path.display()))?;
    }

    // Rename last so a failed tag write never leaves a half-renamed file.
    if let Some((_, stem)) = edit.fields.iter().find(|(f, _)| *f == Field::Filename) {
        let stem = sanitize_filename(stem);
        if !stem.is_empty() {
            let mut new_path = path.with_file_name(&stem);
            if let Some(ext) = path.extension() {
                new_path.set_extension(ext);
            }
            if new_path != path {
                if new_path.exists() {
                    return Err(anyhow!("{} already exists", new_path.display()));
                }
                std::fs::rename(path, &new_path)?;
                return Ok(new_path);
            }
        }
    }
    Ok(path.to_path_buf())
}

/// ID3v2 lyrics/comment frames (USLT/COMM) need a 3-letter language code.
/// Lots of files in the wild carry blank or NUL codes, which lofty refuses to
/// write back, so replace those with ISO 639-2 "XXX" (unknown). Items that
/// don't use a language ignore it, so this is safe to apply to every item.
fn fix_languages(tag: &mut Tag) {
    use lofty::tag::items::UNKNOWN_LANGUAGE;
    let valid = |lang: &[u8; 3]| lang.iter().all(u8::is_ascii_alphabetic);
    let mut keys: Vec<ItemKey> = vec![];
    for item in tag.items() {
        if !valid(item.lang()) && !keys.contains(&item.key()) {
            keys.push(item.key());
        }
    }
    for key in keys {
        let items: Vec<_> = tag.take(key).collect();
        for mut item in items {
            if !valid(item.lang()) {
                item.set_lang(UNKNOWN_LANGUAGE);
            }
            tag.insert_unchecked(item);
        }
    }
}

pub fn sanitize_filename(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '/' | ':' | '\0' => '-',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run with: EASYTAG_SAMPLES=/dir/with/tone.{mp3,flac,m4a} cargo test -- --ignored
    #[test]
    #[ignore]
    fn roundtrip_formats() {
        let dir = PathBuf::from(std::env::var("EASYTAG_SAMPLES").expect("EASYTAG_SAMPLES"));
        let tmp = std::env::temp_dir().join("easytag-roundtrip");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // 1x1 PNG
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52,
            0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0A, 0x49,
            0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0, 1, 0, 0, 5, 0, 1, 0x0D, 0x0A, 0x2D, 0xB4, 0, 0,
            0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        // An MP3 whose ID3v2.3 USLT (lyrics) frame has a blank language code,
        // like many files in the wild. Saving must not fail on it.
        {
            let mut body = vec![0u8, 0, 0, 0, 0];
            body.extend_from_slice(b"la la la");
            let mut frame = b"USLT".to_vec();
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(&[0, 0]);
            frame.extend_from_slice(&body);
            let size = frame.len() as u32;
            let synchsafe = [
                ((size >> 21) & 0x7f) as u8,
                ((size >> 14) & 0x7f) as u8,
                ((size >> 7) & 0x7f) as u8,
                (size & 0x7f) as u8,
            ];
            let mut bytes = b"ID3\x03\x00\x00".to_vec();
            bytes.extend_from_slice(&synchsafe);
            bytes.extend_from_slice(&frame);
            bytes.extend_from_slice(&std::fs::read(dir.join("tone.mp3")).unwrap());
            let bad = tmp.join("bad-lyrics.mp3");
            std::fs::write(&bad, bytes).unwrap();
            let edit = Edit {
                fields: vec![(Field::Title, "Saved".into())],
                cover: None,
                explicit: None,
            };
            write_track(&bad, &edit).expect("blank lyrics language must not block saving");
            assert_eq!(read_track(&bad).title, "Saved");
            let f = lofty::read_from_path(&bad).unwrap();
            let lyrics = f.primary_tag().unwrap().get_string(ItemKey::UnsyncLyrics);
            assert_eq!(lyrics, Some("la la la"), "lyrics must survive the save");
        }
        for ext in ["mp3", "flac", "m4a", "opus"] {
            let src = tmp.join(format!("tone.{ext}"));
            std::fs::copy(dir.join(format!("tone.{ext}")), &src).unwrap();
            let edit = Edit {
                fields: vec![
                    (Field::Title, "Ünïcode Title".into()),
                    (Field::Artist, "Artist".into()),
                    (Field::Album, "Album".into()),
                    (Field::AlbumArtist, "Album Artist".into()),
                    (Field::Year, "1997".into()),
                    (Field::Track, "3/12".into()),
                    (Field::Disc, "1/2".into()),
                    (Field::Genre, "Rock".into()),
                    (Field::Comment, "hello".into()),
                    (Field::Composer, "Someone".into()),
                    (Field::Filename, "03 - Renamed".into()),
                ],
                cover: Some(CoverEdit::Set(png.clone())),
                explicit: Some(true),
            };
            let new_path = write_track(&src, &edit).unwrap();
            assert_eq!(
                new_path.file_name().unwrap(),
                format!("03 - Renamed.{ext}").as_str()
            );
            let t = read_track(&new_path);
            assert_eq!(t.error, None, "{ext}");
            assert_eq!(t.title, "Ünïcode Title", "{ext}");
            assert_eq!(t.artist, "Artist", "{ext}");
            assert_eq!(t.album, "Album", "{ext}");
            assert_eq!(t.album_artist, "Album Artist", "{ext}");
            assert_eq!(t.year, "1997", "{ext}");
            assert_eq!(t.track, "3/12", "{ext}");
            assert_eq!(t.disc, "1/2", "{ext}");
            assert_eq!(t.genre, "Rock", "{ext}");
            assert_eq!(t.comment, "hello", "{ext}");
            assert_eq!(t.composer, "Someone", "{ext}");
            assert!(t.has_cover, "{ext}");
            assert!(t.explicit, "{ext}");
            assert_eq!(read_cover(&new_path).unwrap(), png, "{ext}");

            // Clearing fields and removing the cover.
            let edit = Edit {
                fields: vec![(Field::Comment, String::new()), (Field::Track, "4".into())],
                cover: Some(CoverEdit::Remove),
                explicit: Some(false),
            };
            write_track(&new_path, &edit).unwrap();
            let t = read_track(&new_path);
            assert_eq!(t.comment, "", "{ext}");
            assert_eq!(t.track, "4", "{ext}");
            assert_eq!(t.title, "Ünïcode Title", "{ext}");
            assert!(!t.has_cover, "{ext}");
            assert!(!t.explicit, "{ext}");
        }
    }
}

#[cfg(test)]
mod preserve_tests {
    use super::*;

    /// EASYTAG_KEEP=/dir/with/k.{mp3,flac,opus} (files carrying a MY_CUSTOM tag)
    #[test]
    #[ignore]
    fn keeps_unknown_tags_and_art() {
        let dir = PathBuf::from(std::env::var("EASYTAG_KEEP").expect("EASYTAG_KEEP"));
        let png = {
            let mut c = std::io::Cursor::new(Vec::new());
            image::RgbImage::new(2, 2)
                .write_to(&mut c, image::ImageFormat::Png)
                .unwrap();
            c.into_inner()
        };
        for ext in ["mp3", "flac", "opus"] {
            let f = std::env::temp_dir().join(format!("easytag-keep.{ext}"));
            std::fs::copy(dir.join(format!("k.{ext}")), &f).unwrap();
            // Add art + explicit, then change only the title.
            let edit = Edit {
                cover: Some(CoverEdit::Set(png.clone())),
                explicit: Some(true),
                ..Default::default()
            };
            write_track(&f, &edit).unwrap();
            let edit = Edit {
                fields: vec![(Field::Title, "Changed".into())],
                ..Default::default()
            };
            write_track(&f, &edit).unwrap();

            let t = read_track(&f);
            assert_eq!(t.title, "Changed", "{ext}");
            assert!(t.has_cover, "{ext} lost its art on a title-only save");
            assert!(t.explicit, "{ext} lost explicit");
            let out = std::process::Command::new("ffprobe")
                .args([
                    "-loglevel",
                    "error",
                    "-show_entries",
                    "stream_tags:format_tags",
                    "-of",
                    "compact",
                ])
                .arg(&f)
                .output()
                .unwrap();
            let tags = String::from_utf8_lossy(&out.stdout);
            println!("{ext}: {tags}");
            assert!(
                tags.to_uppercase().contains("MY_CUSTOM=HELLO"),
                "{ext} lost custom tag: {tags}"
            );
        }
    }
}
