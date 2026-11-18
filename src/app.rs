//! The main window: folder columns | file grid | tag panel.

use crate::autotag;
use crate::library::{self, FolderEntry, Resolver, Role};
use crate::settings::Settings;
use crate::tags::{self, CoverEdit, Edit, Field, Track};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::{self as input, Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Theme, ThemeMode, TitleBar, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

gpui_kit::actions!(
    easytag,
    [
        Quit,
        OpenFolder,
        Save,
        Reload,
        AutoTag,
        SelectAll,
        SelectNext,
        SelectPrev,
        ExtendNext,
        ExtendPrev,
        ToggleTheme,
        ClearSelection,
        UndoMove,
        TrashSelection,
    ]
);

pub const GRID_CONTEXT: &str = "FileGrid";
/// The whole window. Shortcuts bound here lose to a focused text field's own
/// (⌘Z, Escape), unlike context-free bindings, which match at every depth.
pub const APP_CONTEXT: &str = "Library";

/// Let the frosted window show through. GPUI Kit paints the whole window
/// surface with the theme's background token; clear only its *paint* (the
/// token's solid colour, which every other colour derives from, is kept).
/// Themes reset the token whenever they change, so this re-applies itself.
pub fn enable_glass(cx: &mut App) {
    fn apply(cx: &mut App) {
        let clear: Background = gpui_kit::transparent_black().into();
        if Theme::global(cx).tokens.background.background != clear {
            Theme::update(cx, |theme| theme.tokens.background.background = clear);
        }
    }
    apply(cx);
    cx.observe_global::<Theme>(apply).detach();
}

/// The app's keyboard shortcuts (also used by the UI tests).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-r", Reload, None),
        KeyBinding::new("cmd-shift-t", AutoTag, None),
        KeyBinding::new("cmd-a", SelectAll, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-backspace", TrashSelection, Some(GRID_CONTEXT)),
        KeyBinding::new("escape", ClearSelection, Some(APP_CONTEXT)),
        KeyBinding::new("cmd-z", UndoMove, Some(APP_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(GRID_CONTEXT)),
        KeyBinding::new("up", SelectPrev, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-down", ExtendNext, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-up", ExtendPrev, Some(GRID_CONTEXT)),
    ]);
}

const COLUMN_WIDTH: f32 = 210.;
const MIN_BROWSER: f32 = 180.;
const MIN_INSPECTOR: f32 = 260.;
const MAX_INSPECTOR: f32 = 560.;
const ROW_HEIGHT: f32 = 30.;
/// Rows are inset so the selection reads as a rounded pill.
const ROW_INSET: f32 = 6.;

// ---------------------------------------------------------------------------
// Grid columns

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Col {
    Filename,
    Title,
    Explicit,
    Artist,
    Album,
    Track,
    Disc,
    Year,
    Genre,
    AlbumArtist,
    Length,
    Bitrate,
    Format,
    Cover,
    Path,
}

impl Col {
    const ALL: [Col; 15] = [
        Col::Cover,
        Col::Filename,
        Col::Title,
        Col::Explicit,
        Col::Artist,
        Col::Album,
        Col::Track,
        Col::Disc,
        Col::Year,
        Col::Genre,
        Col::AlbumArtist,
        Col::Length,
        Col::Bitrate,
        Col::Format,
        Col::Path,
    ];

    fn title(self) -> &'static str {
        match self {
            Col::Filename => "Filename",
            Col::Title => "Title",
            Col::Explicit => "",
            Col::Artist => "Artist",
            Col::Album => "Album",
            Col::Track => "Track",
            Col::Disc => "Disc",
            Col::Year => "Year",
            Col::Genre => "Genre",
            Col::AlbumArtist => "Album Artist",
            Col::Length => "Length",
            Col::Bitrate => "Bitrate",
            Col::Format => "Format",
            Col::Cover => "",
            Col::Path => "Path",
        }
    }

    fn width(self) -> f32 {
        match self {
            Col::Filename => 240.,
            Col::Title => 220.,
            Col::Artist | Col::Album | Col::AlbumArtist => 170.,
            Col::Track | Col::Disc => 56.,
            Col::Year => 56.,
            Col::Genre => 110.,
            Col::Length | Col::Format => 64.,
            Col::Cover => 40.,
            Col::Explicit => 30.,
            Col::Bitrate => 76.,
            Col::Path => 360.,
        }
    }

    /// Technical columns are drawn in a quieter colour than names and titles.
    fn secondary(self) -> bool {
        matches!(
            self,
            Col::Track
                | Col::Disc
                | Col::Year
                | Col::Genre
                | Col::Length
                | Col::Bitrate
                | Col::Format
                | Col::Path
        )
    }

    fn numeric(self) -> bool {
        matches!(
            self,
            Col::Track | Col::Disc | Col::Year | Col::Length | Col::Bitrate
        )
    }

    fn text(self, t: &Track, root: Option<&Path>) -> String {
        match self {
            Col::Filename => t.file_name(),
            Col::Title => t.title.clone(),
            Col::Explicit => if t.explicit { "E" } else { "" }.into(),
            Col::Artist => t.artist.clone(),
            Col::Album => t.album.clone(),
            Col::Track => t.track.clone(),
            Col::Disc => t.disc.clone(),
            Col::Year => t.year.clone(),
            Col::Genre => t.genre.clone(),
            Col::AlbumArtist => t.album_artist.clone(),
            Col::Length => t.length(),
            Col::Bitrate => t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default(),
            Col::Format => t.format.clone(),
            Col::Cover => if t.has_cover { "✓" } else { "" }.into(),
            Col::Path => {
                let dir = t.path.parent().unwrap_or(Path::new(""));
                root.and_then(|r| dir.strip_prefix(r).ok())
                    .unwrap_or(dir)
                    .display()
                    .to_string()
            }
        }
    }

    fn sort_key(self, t: &Track) -> (u64, String) {
        let lead = |s: &str| -> u64 {
            s.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(u64::MAX)
        };
        match self {
            Col::Track => (lead(&t.track), String::new()),
            Col::Disc => (lead(&t.disc), String::new()),
            Col::Year => (lead(&t.year), String::new()),
            Col::Length => (t.duration_secs, String::new()),
            Col::Bitrate => (t.bitrate.unwrap_or(0) as u64, String::new()),
            Col::Path => (0, t.path.to_string_lossy().to_lowercase()),
            c => (0, c.text(t, None).to_lowercase()),
        }
    }
}

/// The song list's columns and their widths. Filename, Title, Artist and
/// Album share the visible width exactly, so the pane always ends on a column
/// boundary; the technical columns continue to the right when you scroll.
/// The art and explicit columns are left out when no row uses them.
fn grid_columns(show_art: bool, show_explicit: bool, available: f32) -> Vec<(Col, f32)> {
    let cols: Vec<Col> = Col::ALL
        .iter()
        .copied()
        .filter(|c| show_art || *c != Col::Cover)
        .filter(|c| show_explicit || *c != Col::Explicit)
        .collect();
    let share = |c: Col| match c {
        Col::Filename | Col::Title => Some(0.3),
        Col::Artist | Col::Album => Some(0.2),
        _ => None,
    };
    // Everything up to Album is on screen at rest.
    let fixed_on_screen: f32 = cols
        .iter()
        .take_while(|c| **c != Col::Track)
        .filter(|c| share(**c).is_none())
        .map(|c| c.width())
        .sum();
    let flexible = (available - fixed_on_screen).max(0.);
    cols.into_iter()
        .map(|c| match share(c) {
            Some(fraction) => (c, (flexible * fraction).max(c.width() * 0.6)),
            None => (c, c.width()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// State

struct Column {
    /// The folder whose children this column lists.
    dir: PathBuf,
    entries: Vec<FolderEntry>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
}

struct FieldInput {
    field: Field,
    state: Entity<InputState>,
    /// Value shown when the selection was loaded ("" when values differ).
    original: String,
    /// The selected files disagree on this field (shown as "< keep >").
    mixed: bool,
}

#[derive(Default)]
struct CoverView {
    image: Option<Arc<Image>>,
    info: String,
    mixed: bool,
    pending: Option<CoverEdit>,
    /// Bumped on every selection change so stale cover loads are dropped.
    generation: u64,
}

pub struct MainWindow {
    focus: FocusHandle,
    grid_focus: FocusHandle,
    root: Option<PathBuf>,
    columns: Vec<Column>,
    columns_scroll: ScrollHandle,
    current_dir: Option<PathBuf>,
    /// Side panel widths. Resizing the window only changes the song list in
    /// the middle, so the layout never shifts frame to frame.
    browser_width: Pixels,
    inspector_width: Pixels,
    /// The song list pane's measured width (from the last layout), so its
    /// main columns can fill it exactly.
    songs_pane_width: Rc<Cell<f32>>,
    /// Shared with the song list's render closure, so frames don't copy it.
    tracks: Arc<Vec<Track>>,
    selected: HashSet<PathBuf>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    sort: Option<(Col, bool)>,
    loading: bool,
    load_generation: u64,
    fields: Vec<FieldInput>,
    cover: CoverView,
    status: SharedString,
    status_error: bool,
    busy: Option<SharedString>,
    grid_scroll: UniformListScrollHandle,
    /// Sideways scrolling of the song list (its columns are wider than the panel).
    grid_hscroll: ScrollHandle,
    settings: Settings,
    renaming: Option<Renaming>,
    explicit: ExplicitState,
    /// Folder art thumbnails for the column browser (`None` = no art).
    thumbs: HashMap<PathBuf, Option<Arc<Image>>>,
    thumbs_loading: HashSet<PathBuf>,
    /// Album art per album folder, for the small art in each song row.
    row_art: HashMap<PathBuf, Option<Arc<Image>>>,
    row_art_loading: HashSet<PathBuf>,
    /// Large art for the folder header (folder, image).
    header_art: Option<(PathBuf, Option<Arc<Image>>)>,
    /// Moves and trashes, newest last, for ⌘Z.
    undo_moves: Vec<UndoBatch>,
    /// Set by a song row's mouse-down so the list background doesn't also
    /// treat the press as a click on empty space.
    row_pressed: bool,
    /// Mouse went down on an already-selected row: keep the selection (so it
    /// can be dragged) and collapse to that row on mouse-up instead.
    collapse_on_click: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

/// One undoable file operation: (from, to) renames on disk.
struct UndoBatch {
    moves: Vec<(PathBuf, PathBuf)>,
    /// Went to the Trash: the browser stays where it was instead of
    /// following the items.
    trashed: bool,
}

/// A panel divider being dragged.
#[derive(Clone, Copy, PartialEq)]
enum Divider {
    Browser,
    Inspector,
}

/// Nothing to draw while dragging a divider (the panel itself moves).
struct NoPreview;
impl Render for NoPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Songs being dragged out of the song list.
#[derive(Clone)]
struct DraggedSongs(Vec<PathBuf>);

/// A folder being dragged in the column browser.
#[derive(Clone)]
struct DraggedFolder(PathBuf);

/// What follows the pointer while dragging.
struct DragPreview {
    icon: &'static str,
    label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .text_sm()
            .shadow_md()
            .child(Icon::empty().path(self.icon).size_4())
            .child(self.label.clone())
    }
}

/// The Explicit checkbox in the tag panel.
#[derive(Default)]
struct ExplicitState {
    /// `None` when the selected files disagree.
    original: Option<bool>,
    pending: Option<bool>,
}

impl ExplicitState {
    fn shown(&self) -> bool {
        self.pending.or(self.original).unwrap_or(false)
    }

    fn change(&self) -> Option<bool> {
        self.pending.filter(|p| Some(*p) != self.original)
    }
}

/// A folder being renamed inline in the column browser.
struct Renaming {
    path: PathBuf,
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// A small tooltip that appears in place, without animation.
struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .max_w(px(320.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .text_xs()
            .shadow_md()
            .child(self.0.clone())
    }
}

fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

/// Wrap an element so it shows a plain tooltip on hover.
fn with_tip(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    el: impl IntoElement,
) -> Stateful<Div> {
    div().id(id).flex_none().tooltip(tip(text)).child(el)
}

/// Lock a scroll area to the direction a swipe starts in (GPUI's
/// `restrict_scroll_to_axis`, which `uniform_list` doesn't expose).
fn lock_scroll_axis<E: InteractiveElement>(mut el: E) -> E {
    el.interactivity().base_style.restrict_scroll_to_axis = Some(true);
    el
}

/// A thin draggable line between panels (with a wider invisible grab area).
fn divider(which: Divider, theme: &Theme) -> Stateful<Div> {
    let line = theme.border.opacity(0.5);
    div()
        .id(match which {
            Divider::Browser => "divider-browser",
            Divider::Inspector => "divider-inspector",
        })
        .h_full()
        .w(px(1.))
        .flex_none()
        .relative()
        .bg(line)
        .child(
            div()
                .id("grab")
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(-3.))
                .w(px(7.))
                .cursor_col_resize()
                .on_drag(which, |_, _, _, cx| cx.new(|_| NoPreview)),
        )
}

/// Confine a panel's content to the panel (no overflow into neighbours).
fn clip(el: impl IntoElement) -> Div {
    div()
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(el)
}

/// Look up an album online by the folder-derived artist, then by the artists
/// already in the songs' tags (for folders like "my comps" whose name isn't an
/// artist).
fn lookup_album(
    folder_artist: &str,
    album: &str,
    tracks: &[Track],
) -> anyhow::Result<autotag::AlbumInfo> {
    let mut artists: Vec<String> = vec![];
    if !folder_artist.is_empty() {
        artists.push(folder_artist.to_string());
    }
    for t in tracks {
        for a in [&t.album_artist, &t.artist] {
            if !a.is_empty() && !artists.contains(a) {
                artists.push(a.clone());
            }
        }
    }
    artists.truncate(3);
    let mut last = anyhow::anyhow!("no match for {album}");
    for a in artists {
        match autotag::lookup_album(&a, album) {
            Ok(info) => return Ok(info),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `path` with its `old` prefix swapped for `new` (after a folder rename).
fn rebase(path: &Path, old: &Path, new: &Path) -> PathBuf {
    match path.strip_prefix(old) {
        Ok(rest) if rest.as_os_str().is_empty() => new.to_path_buf(),
        Ok(rest) => new.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn image_format(bytes: &[u8]) -> ImageFormat {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        ImageFormat::Png
    } else if bytes.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if bytes.starts_with(b"BM") {
        ImageFormat::Bmp
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else {
        ImageFormat::Jpeg
    }
}

fn cover_info(bytes: &[u8]) -> String {
    let kind = match image_format(bytes) {
        ImageFormat::Png => "PNG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Webp => "WEBP",
        _ => "JPEG",
    };
    let dims = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .map(|(w, h)| format!("{w}×{h} "))
        .unwrap_or_default();
    format!("{dims}{kind} · {} KB", bytes.len() / 1024)
}

/// Read tags for many files using all cores.
fn read_tracks_parallel(paths: Vec<PathBuf>) -> Vec<Track> {
    if paths.is_empty() {
        return vec![];
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);
    let chunk = paths.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(|p| tags::read_track(p)).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![];
        let fields = Field::ALL
            .iter()
            .map(|&field| {
                let state = cx.new(|cx| InputState::new(window, cx));
                subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => cx.notify(),
                        InputEvent::PressEnter { .. } => this.save(window, cx),
                        _ => {}
                    },
                ));
                FieldInput {
                    field,
                    state,
                    original: String::new(),
                    mixed: false,
                }
            })
            .collect();

        subscriptions.push(cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        }));

        let mut this = Self {
            focus: cx.focus_handle(),
            grid_focus: cx.focus_handle(),
            root: None,
            columns: vec![],
            columns_scroll: ScrollHandle::new(),
            current_dir: None,
            tracks: Arc::new(vec![]),
            songs_pane_width: Rc::new(Cell::new(0.)),
            browser_width: px(COLUMN_WIDTH * 2.),
            inspector_width: px(320.),
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            sort: None,
            loading: false,
            load_generation: 0,
            fields,
            cover: CoverView::default(),
            status: SharedString::default(),
            status_error: false,
            busy: None,
            grid_scroll: UniformListScrollHandle::new(),
            grid_hscroll: ScrollHandle::new(),
            settings: Settings::load(),
            renaming: None,
            explicit: ExplicitState::default(),
            thumbs: HashMap::new(),
            thumbs_loading: HashSet::new(),
            row_art: HashMap::new(),
            row_art_loading: HashSet::new(),
            header_art: None,
            undo_moves: vec![],
            row_pressed: false,
            collapse_on_click: None,
            _subscriptions: subscriptions,
        };
        this.refresh_panel(window, cx);
        if let Some(lib) = this.settings.library.clone()
            && lib.is_dir()
        {
            this.open_root(lib, window, cx);
        }
        window.focus(&this.focus, cx);
        this
    }

    // ----- Library / navigation ------------------------------------------

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Library".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| this.open_root(path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(window, cx);
        self.settings.library = Some(root.clone());
        self.settings.save();
        self.columns = vec![Column {
            entries: library::subfolders(&root),
            dir: root.clone(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        }];
        self.root = Some(root.clone());
        self.set_status("");
        self.show_dir(root, window, cx);
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(window, cx);
        // Re-list every column, keeping the selected names where possible.
        for col in &mut self.columns {
            let selected_path = col
                .selected
                .and_then(|i| col.entries.get(i))
                .map(|e| e.path.clone());
            col.entries = library::subfolders(&col.dir);
            col.selected = selected_path.and_then(|p| col.entries.iter().position(|e| e.path == p));
        }
        if let Some(dir) = self.current_dir.clone() {
            self.show_dir(dir, window, cx);
        }
    }

    /// Click on a folder in column `col_ix`.
    fn select_folder(
        &mut self,
        col_ix: usize,
        entry_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .columns
            .get(col_ix)
            .and_then(|c| c.entries.get(entry_ix))
            .cloned()
        else {
            return;
        };
        self.commit_pending(window, cx);
        self.columns.truncate(col_ix + 1);
        self.columns[col_ix].selected = Some(entry_ix);
        let children = library::subfolders(&entry.path);
        if !children.is_empty() {
            self.columns.push(Column {
                dir: entry.path.clone(),
                entries: children,
                selected: None,
                scroll: UniformListScrollHandle::new(),
            });
        }
        let last = self.columns.len() - 1;
        self.columns_scroll.scroll_to_item(last);
        self.show_dir(entry.path, window, cx);
    }

    /// Click on a column header: show everything in that column's folder.
    fn select_column_root(&mut self, col_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if col_ix >= self.columns.len() {
            return;
        }
        self.commit_pending(window, cx);
        self.columns[col_ix].selected = None;
        let dir = self.columns[col_ix].dir.clone();
        self.columns.truncate(col_ix + 1);
        self.show_dir(dir, window, cx);
    }

    fn show_dir(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.current_dir = Some(dir.clone());
        self.load_generation += 1;
        let generation = self.load_generation;
        self.loading = true;
        let keep_selection = self.selected.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let tracks = cx
                .background_spawn(async move { read_tracks_parallel(library::audio_files(&dir)) })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.load_generation != generation {
                    return;
                }
                this.loading = false;
                this.tracks = Arc::new(tracks);
                this.apply_sort();
                this.selected = keep_selection
                    .into_iter()
                    .filter(|p| this.tracks.iter().any(|t| &t.path == p))
                    .collect();
                this.anchor = None;
                this.cursor = None;
                this.refresh_panel(window, cx);
                this.apply_folder_art(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- Sorting / selection -------------------------------------------

    fn apply_sort(&mut self) {
        let tracks = Arc::make_mut(&mut self.tracks);
        match self.sort {
            None => tracks.sort_by_cached_key(|t| t.path.to_string_lossy().to_lowercase()),
            Some((col, asc)) => {
                tracks.sort_by_cached_key(|t| (col.sort_key(t), t.path.clone()));
                if !asc {
                    tracks.reverse();
                }
            }
        }
    }

    fn click_header(&mut self, col: Col, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
        self.apply_sort();
        self.anchor = None;
        cx.notify();
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.tracks
            .iter()
            .filter(|t| self.selected.contains(&t.path))
            .map(|t| t.path.clone())
            .collect()
    }

    fn selected_tracks(&self) -> Vec<&Track> {
        self.tracks
            .iter()
            .filter(|t| self.selected.contains(&t.path))
            .collect()
    }

    fn click_row(
        &mut self,
        ix: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.grid_focus, cx);
        let Some(path) = self.tracks.get(ix).map(|t| t.path.clone()) else {
            return;
        };
        let mut next = self.selected.clone();
        if modifiers.shift
            && let Some(anchor) = self.anchor
        {
            if !modifiers.platform {
                next.clear();
            }
            let (a, b) = if anchor <= ix {
                (anchor, ix)
            } else {
                (ix, anchor)
            };
            for t in &self.tracks[a..=b.min(self.tracks.len() - 1)] {
                next.insert(t.path.clone());
            }
        } else if modifiers.platform {
            if !next.remove(&path) {
                next.insert(path);
            }
            self.anchor = Some(ix);
        } else if next.contains(&path) && next.len() > 1 {
            // Pressing a selected row keeps the selection so it can be dragged;
            // a plain click collapses it on mouse-up.
            self.collapse_on_click = Some(ix);
            self.anchor = Some(ix);
        } else {
            next.clear();
            next.insert(path);
            self.anchor = Some(ix);
        }
        self.cursor = Some(ix);
        self.set_selection(next, window, cx);
    }

    fn row_clicked(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.collapse_on_click.take() == Some(ix)
            && let Some(t) = self.tracks.get(ix)
        {
            let next = HashSet::from([t.path.clone()]);
            self.set_selection(next, window, cx);
        }
    }

    fn set_selection(
        &mut self,
        next: HashSet<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if next == self.selected {
            return;
        }
        self.commit_pending(window, cx);
        self.selected = next;
        self.refresh_panel(window, cx);
        cx.notify();
    }

    fn clear_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.anchor = None;
        self.cursor = None;
        self.set_selection(HashSet::new(), window, cx);
    }

    fn move_cursor(
        &mut self,
        delta: isize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tracks.is_empty() {
            return;
        }
        let current = self
            .cursor
            .unwrap_or(if delta > 0 { usize::MAX } else { 0 });
        let ix = if current == usize::MAX {
            0
        } else {
            (current as isize + delta).clamp(0, self.tracks.len() as isize - 1) as usize
        };
        if extend {
            let anchor = *self
                .anchor
                .get_or_insert(current.min(self.tracks.len() - 1));
            let (a, b) = if anchor <= ix {
                (anchor, ix)
            } else {
                (ix, anchor)
            };
            let next = self.tracks[a..=b].iter().map(|t| t.path.clone()).collect();
            self.cursor = Some(ix);
            self.set_selection(next, window, cx);
        } else {
            self.anchor = Some(ix);
            self.cursor = Some(ix);
            let next = HashSet::from([self.tracks[ix].path.clone()]);
            self.set_selection(next, window, cx);
        }
        self.grid_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
    }

    // ----- Tag panel -----------------------------------------------------

    fn field_dirty(&self, f: &FieldInput, cx: &App) -> bool {
        let value = f.state.read(cx).value();
        if f.mixed {
            !value.is_empty()
        } else {
            value.as_ref() != f.original
        }
    }

    fn pending_edit(&self, cx: &App) -> Edit {
        let fields = self
            .fields
            .iter()
            .filter(|f| self.field_dirty(f, cx))
            .filter(|f| f.field != Field::Filename || self.selected.len() == 1)
            .map(|f| (f.field, f.state.read(cx).value().to_string()))
            .collect();
        Edit {
            fields,
            cover: self.cover.pending.clone(),
            explicit: self.explicit.change(),
        }
    }

    fn has_pending(&self, cx: &App) -> bool {
        !self.selected.is_empty() && !self.pending_edit(cx).is_empty()
    }

    /// Fill the tag panel from the current selection.
    fn refresh_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection: Vec<Track> = self.selected_tracks().into_iter().cloned().collect();
        let count = selection.len();
        for f in &mut self.fields {
            let values: Vec<String> = selection.iter().map(|t| t.get(f.field)).collect();
            let first = values.first().cloned().unwrap_or_default();
            let mixed = values.iter().any(|v| *v != first);
            f.mixed = mixed;
            f.original = if mixed { String::new() } else { first };
            let placeholder = if count == 0 {
                ""
            } else if f.field == Field::Filename && count > 1 {
                "< multiple files >"
            } else if mixed {
                "< keep >"
            } else {
                ""
            };
            let value = f.original.clone();
            let disabled = count == 0 || (f.field == Field::Filename && count > 1);
            f.state.update(cx, |s, cx| {
                s.set_value(value, window, cx);
                s.set_placeholder(placeholder, window, cx);
                s.set_disabled(disabled, cx);
            });
        }

        self.explicit = ExplicitState {
            original: match selection.first() {
                Some(first) if selection.iter().all(|t| t.explicit == first.explicit) => {
                    Some(first.explicit)
                }
                Some(_) => None,
                None => Some(false),
            },
            pending: None,
        };

        // Cover art: load from the first selected file in the background.
        self.cover.pending = None;
        self.cover.generation += 1;
        self.cover.image = None;
        self.cover.info = String::new();
        self.cover.mixed =
            selection.iter().any(|t| t.has_cover) && selection.iter().any(|t| !t.has_cover);
        let generation = self.cover.generation;
        if let Some(first) = selection.iter().find(|t| t.has_cover) {
            let path = first.path.clone();
            cx.spawn(async move |this, cx| {
                let bytes = cx
                    .background_spawn(async move { tags::read_cover(&path) })
                    .await;
                this.update(cx, |this, cx| {
                    if this.cover.generation != generation {
                        return;
                    }
                    if let Some(bytes) = bytes {
                        this.cover.info = cover_info(&bytes);
                        this.cover.image =
                            Some(Arc::new(Image::from_bytes(image_format(&bytes), bytes)));
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn set_pending_cover(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.cover.info = cover_info(&bytes);
        self.cover.image = Some(Arc::new(Image::from_bytes(
            image_format(&bytes),
            bytes.clone(),
        )));
        self.cover.mixed = false;
        self.cover.pending = Some(CoverEdit::Set(bytes));
        cx.notify();
    }

    fn remove_cover(&mut self, cx: &mut Context<Self>) {
        self.cover.image = None;
        self.cover.info = String::new();
        self.cover.mixed = false;
        self.cover.pending = Some(CoverEdit::Remove);
        cx.notify();
    }

    fn choose_cover_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use as Cover".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.load_cover_file(&path, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn load_cover_file(&mut self, path: &Path, cx: &mut Context<Self>) {
        match std::fs::read(path) {
            Ok(bytes) if image::guess_format(&bytes).is_ok() => self.set_pending_cover(bytes, cx),
            Ok(_) => self.set_error(format!("{} isn't an image", file_label(path))),
            Err(e) => self.set_error(format!("Couldn't read image: {e}")),
        }
        cx.notify();
    }

    fn resolver(&self) -> Option<Resolver> {
        Some(Resolver::new(
            self.root.clone()?,
            self.settings.roles.clone(),
        ))
    }

    /// Look up artwork for the album currently in the Artist/Album fields.
    fn find_cover_online(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = |f: Field, this: &Self, cx: &App| -> String {
            this.fields
                .iter()
                .find(|x| x.field == f)
                .map(|x| x.state.read(cx).value().to_string())
                .unwrap_or_default()
        };
        let mut artist = value(Field::AlbumArtist, self, cx);
        if artist.is_empty() {
            artist = value(Field::Artist, self, cx);
        }
        let mut album = value(Field::Album, self, cx);
        // Fall back to the folder roles.
        if (artist.is_empty() || album.is_empty())
            && let (Some(mut resolver), Some(path)) =
                (self.resolver(), self.selected_paths().first())
        {
            let info = resolver.info(path);
            if artist.is_empty() {
                artist = info.artist.unwrap_or_default();
            }
            if album.is_empty() {
                album = info.album.unwrap_or_default();
            }
        }
        if album.is_empty() {
            self.set_error("Need an album name to search for artwork.");
            cx.notify();
            return;
        }
        self.busy = Some(format!("Searching artwork for {artist} – {album}…").into());
        cx.notify();
        let generation = self.cover.generation;
        cx.spawn_in(window, async move |this, cx| {
            let (a, b) = (artist.clone(), album.clone());
            let result = cx
                .background_spawn(async move { autotag::lookup_album(&a, &b) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                if this.cover.generation != generation {
                    return;
                }
                match result {
                    Ok(info) => {
                        if let Some(bytes) = info.cover {
                            this.set_pending_cover(bytes, cx);
                            this.set_status("");
                        } else {
                            this.set_error(format!("No artwork found for {artist} – {album}"));
                        }
                    }
                    Err(e) => this.set_error(format!("Artwork lookup failed: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Save pending edits for the current selection (if any) before it changes.
    fn commit_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_pending(cx) {
            self.save(window, cx);
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        let edit = self.pending_edit(cx);
        if paths.is_empty() || edit.is_empty() {
            return;
        }
        // Mark the panel clean right away so a quick selection change doesn't double-save.
        for f in &mut self.fields {
            if !f.mixed || !f.state.read(cx).value().is_empty() {
                f.original = f.state.read(cx).value().to_string();
                f.mixed = false;
            }
        }
        self.cover.pending = None;
        if let Some(v) = self.explicit.pending.take() {
            self.explicit.original = Some(v);
        }
        let edits = paths.into_iter().map(|p| (p, edit.clone())).collect();
        self.write_edits(edits, String::new(), window, cx);
    }

    /// Write edits in the background, then re-read the affected files.
    fn write_edits(
        &mut self,
        edits: Vec<(PathBuf, Edit)>,
        note: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let total = edits.len();
        let art_changed: Vec<PathBuf> = edits
            .iter()
            .filter(|(_, e)| e.cover.is_some())
            .map(|(p, _)| p.clone())
            .collect();
        self.busy = Some(format!("Writing {total} file(s)…").into());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let results = cx
                .background_spawn(async move {
                    edits
                        .into_iter()
                        .map(|(path, edit)| {
                            let res = tags::write_track(&path, &edit);
                            let new_path = res.as_ref().ok().cloned().unwrap_or(path.clone());
                            (path, tags::read_track(&new_path), res.err())
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                let mut errors = vec![];
                let mut selection_touched = false;
                for (old, track, err) in results {
                    if let Some(e) = err {
                        errors.push(format!("{}: {}", file_label(&old), e.root_cause()));
                    }
                    if this.selected.remove(&old) {
                        this.selected.insert(track.path.clone());
                        selection_touched = true;
                    }
                    if let Some(slot) = Arc::make_mut(&mut this.tracks)
                        .iter_mut()
                        .find(|t| t.path == old)
                    {
                        *slot = track;
                    }
                }
                if errors.is_empty() {
                    this.set_status(note.trim().to_string());
                } else {
                    let msg = format!(
                        "{} of {total} file(s) couldn't be saved. {}",
                        errors.len(),
                        errors.first().cloned().unwrap_or_default()
                    );
                    window.push_notification(Notification::error(msg), cx);
                    this.set_status("");
                }
                this.invalidate_thumbs(&art_changed);
                // Refresh the panel only if the user hasn't started new edits there.
                if selection_touched && !this.has_pending(cx) {
                    this.refresh_panel(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- Folder art thumbnails ------------------------------------------

    /// Forget cached thumbnails for every folder containing one of `paths`.
    fn invalidate_thumbs(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        self.thumbs
            .retain(|dir, _| !paths.iter().any(|p| p.starts_with(dir)));
        self.row_art
            .retain(|dir, _| !paths.iter().any(|p| p.starts_with(dir)));
        if self
            .header_art
            .as_ref()
            .is_some_and(|(dir, _)| paths.iter().any(|p| p.starts_with(dir)))
        {
            self.header_art = None;
        }
    }

    /// Load the small album art shown in song rows, one image per album.
    fn request_row_art(&mut self, cx: &mut Context<Self>) {
        let mut missing: Vec<PathBuf> = vec![];
        for t in self.tracks.iter() {
            if let Some(dir) = library::album_dir_of(&t.path)
                && !self.row_art.contains_key(dir)
                && !self.row_art_loading.contains(dir)
                && !missing.iter().any(|m| m == dir)
            {
                missing.push(dir.to_path_buf());
            }
        }
        if missing.is_empty() {
            return;
        }
        self.row_art_loading.extend(missing.iter().cloned());
        cx.spawn(async move |this, cx| {
            for dir in missing {
                let d = dir.clone();
                let png = cx
                    .background_spawn(async move {
                        library::png_thumbnail(&library::folder_art_bytes(&d, true)?, 64)
                    })
                    .await;
                let ok = this.update(cx, |this, cx| {
                    this.row_art_loading.remove(&dir);
                    this.row_art.insert(
                        dir,
                        png.map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b))),
                    );
                    cx.notify();
                });
                if ok.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The role of the folder being shown (its row in the browser, if any).
    fn current_role(&self) -> Option<Role> {
        let dir = self.current_dir.as_ref()?;
        self.columns
            .iter()
            .flat_map(|c| c.entries.iter())
            .find(|e| &e.path == dir)
            .map(|e| self.folder_role(e).0)
    }

    /// Load the large art for the folder header when the folder changes.
    fn request_header_art(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.current_dir.clone() else {
            return;
        };
        if self.header_art.as_ref().is_some_and(|(d, _)| *d == dir) {
            return;
        }
        let is_album = self.current_role() == Some(Role::Album);
        self.header_art = Some((dir.clone(), None));
        cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let png = cx
                .background_spawn(async move {
                    library::png_thumbnail(&library::folder_art_bytes(&d, is_album)?, 320)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.header_art.as_ref().is_some_and(|(d, _)| *d == dir) {
                    this.header_art = Some((
                        dir,
                        png.map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b))),
                    ));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Load thumbnails for any visible folders we haven't looked at yet.
    fn request_thumbs(&mut self, cx: &mut Context<Self>) {
        let missing: Vec<(PathBuf, bool)> = self
            .columns
            .iter()
            .flat_map(|c| c.entries.iter())
            .filter(|e| {
                !self.thumbs.contains_key(&e.path) && !self.thumbs_loading.contains(&e.path)
            })
            .map(|e| (e.path.clone(), self.folder_role(e).0 == Role::Album))
            .collect();
        if missing.is_empty() {
            return;
        }
        self.thumbs_loading
            .extend(missing.iter().map(|(p, _)| p.clone()));
        cx.spawn(async move |this, cx| {
            for (dir, is_album) in missing {
                let d = dir.clone();
                let png = cx
                    .background_spawn(async move { library::folder_thumbnail(&d, is_album) })
                    .await;
                let ok = this.update(cx, |this, cx| {
                    this.thumbs_loading.remove(&dir);
                    this.thumbs.insert(
                        dir,
                        png.map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b))),
                    );
                    cx.notify();
                });
                if ok.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Songs without art inside a folder whose art was set in easytag get it
    /// automatically (e.g. new songs dropped into an album folder).
    fn apply_folder_art(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let marked = self.settings.folder_art.clone();
        if marked.is_empty() || self.busy.is_some() {
            return;
        }
        let missing: Vec<PathBuf> = self
            .tracks
            .iter()
            .filter(|t| !t.has_cover && t.error.is_none())
            .map(|t| t.path.clone())
            .collect();
        if missing.is_empty() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let edits = cx
                .background_spawn(async move {
                    let mut art: HashMap<PathBuf, Option<Vec<u8>>> = HashMap::new();
                    missing
                        .into_iter()
                        .filter_map(|path| {
                            let file = library::album_folder_art(&path)?;
                            if !marked.contains(file.parent()?) {
                                return None;
                            }
                            let bytes = art
                                .entry(file.clone())
                                .or_insert_with(|| std::fs::read(&file).ok())
                                .clone()?;
                            Some((
                                path,
                                Edit {
                                    cover: Some(CoverEdit::Set(bytes)),
                                    ..Default::default()
                                },
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            if edits.is_empty() {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.write_edits(edits, String::new(), window, cx)
            })
            .ok();
        })
        .detach();
    }

    // ----- Folders: create, move, undo -------------------------------------

    /// Re-read the column browser from disk, keeping the selected path
    /// (and each column's scroll position) wherever it still exists.
    fn relist(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let chain: Vec<PathBuf> = self
            .columns
            .iter()
            .filter_map(|c| {
                c.selected
                    .and_then(|i| c.entries.get(i))
                    .map(|e| e.path.clone())
            })
            .collect();
        let mut old: HashMap<PathBuf, UniformListScrollHandle> =
            self.columns.drain(..).map(|c| (c.dir, c.scroll)).collect();
        let mut column = |dir: PathBuf| Column {
            entries: library::subfolders(&dir),
            scroll: old
                .remove(&dir)
                .unwrap_or_else(UniformListScrollHandle::new),
            dir,
            selected: None,
        };
        self.columns.push(column(root));
        for path in chain {
            let last = self.columns.last_mut().expect("root column");
            let Some(ix) = last.entries.iter().position(|e| e.path == path) else {
                break;
            };
            last.selected = Some(ix);
            if last.entries[ix].has_children {
                self.columns.push(column(path));
            } else {
                break;
            }
        }
    }

    /// The selected folder when it has no subfolders: it gets an empty column
    /// so there's somewhere to right-click (New Folder) or drop into.
    fn leaf_folder(&self) -> Option<PathBuf> {
        let last = self.columns.last()?;
        let entry = last.selected.and_then(|i| last.entries.get(i))?;
        (!entry.has_children).then(|| entry.path.clone())
    }

    /// Create "untitled folder" inside `parent`, show it, and start renaming it.
    fn new_folder(
        &mut self,
        parent: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        self.commit_pending(window, cx);
        let mut name = "untitled folder".to_string();
        let mut n = 2;
        while parent.join(&name).exists() {
            name = format!("untitled folder {n}");
            n += 1;
        }
        let path = parent.join(&name);
        if let Err(e) = std::fs::create_dir(&path) {
            let msg = format!("Couldn't create a folder in “{}”: {e}", file_label(&parent));
            window.push_notification(Notification::error(msg), cx);
            return None;
        }
        // Open `parent` in the browser if it isn't already a column.
        if !self.columns.iter().any(|c| c.dir == parent) {
            if let Some((ci, ei)) = self.columns.iter().enumerate().find_map(|(ci, c)| {
                c.entries
                    .iter()
                    .position(|e| e.path == parent)
                    .map(|ei| (ci, ei))
            }) {
                self.columns.truncate(ci + 1);
                self.columns[ci].selected = Some(ei);
            }
        }
        self.relist();
        if let Some(col) = self.columns.iter().find(|c| c.dir == parent)
            && let Some(ix) = col.entries.iter().position(|e| e.path == path)
        {
            col.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        let last = self.columns.len().saturating_sub(1);
        self.columns_scroll.scroll_to_item(last);
        self.start_rename(path.clone(), window, cx);
        Some(path)
    }

    /// New folder in the shown folder, with the selected songs moved into it.
    fn new_folder_with_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.current_dir.clone() else {
            return;
        };
        let songs = self.selected_paths();
        if let Some(folder) = self.new_folder(dir, window, cx) {
            self.move_paths(songs, folder, window, cx);
            if let Some(r) = &self.renaming {
                r.input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            }
        }
    }

    /// Move files/folders into `dest`. Never overwrites: clashes are skipped
    /// and reported. The batch can be undone with ⌘Z.
    fn move_paths(
        &mut self,
        items: Vec<PathBuf>,
        dest: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_pending(window, cx);
        let mut moved: Vec<(PathBuf, PathBuf)> = vec![];
        let mut errors: Vec<String> = vec![];
        for from in items {
            let Some(name) = from.file_name() else {
                continue;
            };
            if from.parent() == Some(dest.as_path()) {
                continue;
            }
            if dest.starts_with(&from) {
                errors.push(format!("“{}” can't go inside itself", file_label(&from)));
                continue;
            }
            let to = dest.join(name);
            if to.exists() {
                errors.push(format!(
                    "“{}” already has a “{}”",
                    file_label(&dest),
                    name.to_string_lossy()
                ));
                continue;
            }
            match std::fs::rename(&from, &to) {
                Ok(()) => moved.push((from, to)),
                Err(e) => errors.push(format!("{}: {e}", file_label(&from))),
            }
        }
        if !errors.is_empty() {
            let msg = if errors.len() == 1 {
                format!("Couldn't move {}.", errors[0])
            } else {
                format!("Couldn't move {} items. {}.", errors.len(), errors[0])
            };
            window.push_notification(Notification::error(msg), cx);
        }
        if moved.is_empty() {
            return;
        }
        self.after_moves(&moved, true, window, cx);
        let what = match moved.len() {
            1 => format!("“{}”", file_label(&moved[0].0)),
            n => format!("{n} items"),
        };
        self.set_status(format!("Moved {what} to “{}”.", file_label(&dest)));
        self.undo_moves.push(UndoBatch {
            moves: moved,
            trashed: false,
        });
    }

    /// Move songs or folders to the Trash (undoable with ⌘Z).
    fn trash_paths(&mut self, items: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<PathBuf> = items.into_iter().filter(|p| p.exists()).collect();
        if items.is_empty() {
            return;
        }
        self.commit_pending(window, cx);
        if self
            .renaming
            .as_ref()
            .is_some_and(|r| items.iter().any(|i| r.path.starts_with(i)))
        {
            self.renaming = None;
        }
        match library::move_to_trash(&items) {
            Ok(trashed) => {
                let moved: Vec<(PathBuf, PathBuf)> = items.into_iter().zip(trashed).collect();
                self.after_moves(&moved, false, window, cx);
                let what = match moved.len() {
                    1 => format!("“{}”", file_label(&moved[0].0)),
                    n => format!("{n} items"),
                };
                self.set_status(format!("Moved {what} to the Trash."));
                self.undo_moves.push(UndoBatch {
                    moves: moved,
                    trashed: true,
                });
            }
            Err(e) => {
                let msg = format!("Couldn't move to the Trash: {e}");
                window.push_notification(Notification::error(msg), cx);
            }
        }
    }

    fn undo_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = self.undo_moves.pop() else {
            return;
        };
        let follow = !batch.trashed;
        let mut back = vec![];
        let mut errors = vec![];
        for (from, to) in batch.moves.into_iter().rev() {
            if from.exists() {
                errors.push(file_label(&from));
                continue;
            }
            match std::fs::rename(&to, &from) {
                Ok(()) => back.push((to, from)),
                Err(e) => errors.push(format!("{}: {e}", file_label(&to))),
            }
        }
        if !errors.is_empty() {
            let msg = format!("Couldn't move back: {}", errors.join(", "));
            window.push_notification(Notification::error(msg), cx);
        }
        if !back.is_empty() {
            self.after_moves(&back, follow, window, cx);
            self.set_status("");
        }
    }

    /// Bring settings, thumbnails, the browser and the song list up to date
    /// after `moved` (from, to) pairs happened on disk.
    /// With `follow`, the browser follows a moved folder; for the Trash it
    /// stays put.
    fn after_moves(
        &mut self,
        moved: &[(PathBuf, PathBuf)],
        follow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rebase_all = |p: &Path| {
            moved
                .iter()
                .fold(p.to_path_buf(), |p, (from, to)| rebase(&p, from, to))
        };
        let roles = std::mem::take(&mut self.settings.roles);
        self.settings.roles = roles
            .into_iter()
            .map(|(p, r)| (rebase_all(&p), r))
            .collect();
        let art = std::mem::take(&mut self.settings.folder_art);
        self.settings.folder_art = art.into_iter().map(|p| rebase_all(&p)).collect();
        self.settings.save();
        let touched: Vec<PathBuf> = moved
            .iter()
            .flat_map(|(a, b)| [a.clone(), b.clone()])
            .collect();
        self.invalidate_thumbs(&touched);

        if follow {
            // Keep browsing the same folder, following it if it was moved.
            if let Some(dir) = self.current_dir.clone() {
                self.current_dir = Some(rebase_all(&dir));
            }
            for col in &mut self.columns {
                col.dir = rebase_all(&col.dir);
                for e in &mut col.entries {
                    e.path = rebase_all(&e.path);
                }
            }
            self.selected = self.selected.iter().map(|p| rebase_all(p)).collect();
        } else if let Some(dir) = self.current_dir.clone()
            && let Some((from, _)) = moved.iter().find(|(from, _)| dir.starts_with(from))
        {
            // The folder being shown went to the Trash: show its parent.
            self.current_dir = from.parent().map(Path::to_path_buf);
        }
        self.relist();
        let dir = self
            .current_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| self.root.clone());
        if let Some(dir) = dir {
            self.show_dir(dir, window, cx);
        }
        cx.notify();
    }

    // ----- Status --------------------------------------------------------

    fn set_status(&mut self, msg: impl Into<SharedString>) {
        self.status = msg.into();
        self.status_error = false;
    }

    fn set_error(&mut self, msg: impl Into<SharedString>) {
        self.status = msg.into();
        self.status_error = true;
    }

    // ----- Auto-tag ------------------------------------------------------

    /// Toolbar / ⌘⇧T: the selected files, or everything shown.
    fn auto_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(window, cx);
        let targets: Vec<Track> = if self.selected.is_empty() {
            self.tracks.to_vec()
        } else {
            self.selected_tracks().into_iter().cloned().collect()
        };
        self.auto_tag_tracks(targets, window, cx);
    }

    /// Run `f` with every track inside `dir` (tags read in the background).
    /// `songs` picks which files: `library::audio_files` (everything below
    /// `dir`) or `library::album_songs` (just that album).
    fn with_folder_tracks(
        &mut self,
        dir: PathBuf,
        songs: fn(&Path) -> Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, Vec<Track>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.commit_pending(window, cx);
        self.busy = Some(format!("Reading {}…", file_label(&dir)).into());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let tracks = cx
                .background_spawn(async move { read_tracks_parallel(songs(&dir)) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                if tracks.is_empty() {
                    this.set_error("No audio files in that folder.");
                    cx.notify();
                } else {
                    f(this, tracks, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn auto_tag_tracks(
        &mut self,
        targets: Vec<Track>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut resolver) = self.resolver() else {
            self.set_error("Open a library folder first.");
            cx.notify();
            return;
        };
        if self.busy.is_some() {
            return;
        }
        if targets.is_empty() {
            self.set_status("No songs to auto-tag.");
            cx.notify();
            return;
        }
        self.busy = Some("Auto-tagging…".into());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            // Folder roles touch the disk, so group off the main thread.
            let groups = cx
                .background_spawn(async move { resolver.group(targets) })
                .await;
            let group_count = groups.len();
            let mut edits: Vec<(PathBuf, Edit)> = vec![];
            let mut misses = vec![];
            let mut saved_dirs: Vec<PathBuf> = vec![];
            for (i, ((artist, album), files)) in groups.into_iter().enumerate() {
                this.update(cx, |this, cx| {
                    this.busy = Some(
                        format!("Auto-tagging {}/{group_count}: {artist} – {album}", i + 1).into(),
                    );
                    cx.notify();
                })
                .ok();

                // Art already saved in the folder (cover.jpg etc.) wins over online art.
                let first = files.first().map(|(t, _)| t.path.clone());
                let album_dir = files.first().and_then(|(_, i)| i.album_dir.clone());
                let local_art = cx
                    .background_spawn(async move {
                        let art = library::album_folder_art(&first?)?;
                        std::fs::read(art).ok()
                    })
                    .await;
                let needs_lookup = !album.is_empty()
                    && files.iter().any(|(t, _)| {
                        (!t.has_cover && local_art.is_none())
                            || t.year.is_empty()
                            || t.genre.is_empty()
                    });
                let online = if needs_lookup {
                    let (a, b) = (artist.clone(), album.clone());
                    let sample: Vec<Track> = files.iter().take(5).map(|(t, _)| t.clone()).collect();
                    match cx
                        .background_spawn(async move { lookup_album(&a, &b, &sample) })
                        .await
                    {
                        Ok(info) => Some(info),
                        Err(_) => {
                            misses.push(if artist.is_empty() {
                                album.clone()
                            } else {
                                format!("{artist} – {album}")
                            });
                            None
                        }
                    }
                } else {
                    None
                };
                let online_art = online.as_ref().and_then(|o| o.cover.clone());
                let art = local_art.clone().or(online_art.clone());
                if local_art.is_none()
                    && let Some(bytes) = online_art
                    && files.iter().any(|(t, _)| !t.has_cover)
                {
                    // Keep it with the album so new songs can get it too.
                    if let Some(dir) = album_dir.clone() {
                        let saved = cx
                            .background_spawn(async move {
                                library::save_folder_art(&dir, &bytes).ok().map(|_| dir)
                            })
                            .await;
                        saved_dirs.extend(saved);
                    }
                }

                for (t, info) in files {
                    let mut e = Edit::default();
                    let mut set = |f: Field, v: String| {
                        if !v.is_empty() && t.get(f) != v {
                            e.fields.push((f, v));
                        }
                    };
                    // Roles you picked overwrite; roles easytag guessed only fill blanks.
                    if let Some(artist) = &info.artist {
                        let overwrite = info.artist_explicit;
                        // Keep "Artist feat. Someone" style credits intact.
                        if (overwrite && !t.artist.to_lowercase().contains(&artist.to_lowercase()))
                            || t.artist.is_empty()
                        {
                            set(Field::Artist, artist.clone());
                        }
                        if info.album.is_some() && (overwrite || t.album_artist.is_empty()) {
                            set(Field::AlbumArtist, artist.clone());
                        }
                    }
                    if let Some(album) = &info.album
                        && (info.album_explicit || t.album.is_empty())
                    {
                        set(Field::Album, album.clone());
                    }
                    if t.title.is_empty()
                        && let Some(title) = &info.title
                    {
                        set(Field::Title, title.clone());
                    }
                    if t.track.is_empty()
                        && let Some(n) = info.track
                    {
                        set(Field::Track, n.to_string());
                    }
                    if t.disc.is_empty()
                        && let Some(n) = info.disc
                    {
                        set(Field::Disc, n.to_string());
                    }
                    if t.year.is_empty() {
                        if let Some(y) = online.as_ref().and_then(|o| o.year.clone()) {
                            set(Field::Year, y);
                        } else if let Some(y) = info.album_year.clone() {
                            set(Field::Year, y);
                        }
                    }
                    if t.genre.is_empty()
                        && let Some(g) = online.as_ref().and_then(|o| o.genre.clone())
                    {
                        set(Field::Genre, g);
                    }
                    if !t.has_cover
                        && let Some(bytes) = art.clone()
                    {
                        e.cover = Some(CoverEdit::Set(bytes));
                    }
                    if !e.is_empty() {
                        edits.push((t.path.clone(), e));
                    }
                }
            }

            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.settings.folder_art.extend(saved_dirs.iter().cloned());
                this.settings.save();
                let covers: Vec<_> = saved_dirs.iter().map(|d| d.join("cover.jpg")).collect();
                this.invalidate_thumbs(&covers);
                let mut note = String::new();
                if !misses.is_empty() {
                    note.push_str(&format!(" No online match for {}.", misses.join(", ")));
                }
                if edits.is_empty() {
                    this.set_status(format!("Nothing to change.{note}"));
                    cx.notify();
                    return;
                }
                this.write_edits(edits, note, window, cx);
            })
            .ok();
        })
        .detach();
    }

    // ----- Folder actions (right-click menu) -------------------------------

    fn folder_role(&self, entry: &FolderEntry) -> (Role, bool) {
        match self.settings.roles.get(&entry.path) {
            Some(r) => (*r, true),
            None => (entry.auto_role, false),
        }
    }

    /// `None` = go back to the automatic guess.
    fn set_folder_role(&mut self, path: PathBuf, role: Option<Role>, cx: &mut Context<Self>) {
        self.thumbs.remove(&path);
        match role {
            Some(r) => {
                let status = match r {
                    Role::Neither => String::new(),
                    _ => format!(
                        "Tags in “{}” change when you auto-tag it.",
                        file_label(&path)
                    ),
                };
                self.settings.roles.insert(path, r);
                self.set_status(status);
            }
            None => {
                self.settings.roles.remove(&path);
                self.set_status("");
            }
        }
        self.settings.save();
        cx.notify();
    }

    fn auto_tag_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.with_folder_tracks(
            dir,
            library::audio_files,
            window,
            cx,
            |this, tracks, window, cx| this.auto_tag_tracks(tracks, window, cx),
        );
    }

    // The art actions below only ever touch one album: the songs in `dir`
    // and its disc folders. Albums nested inside are left alone.

    /// Save art into `dir` as cover.jpg, remember it, and embed it in the album's songs.
    fn set_folder_art(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use as Cover".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(image) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| match std::fs::read(&image) {
                Ok(bytes) if image::guess_format(&bytes).is_ok() => {
                    this.apply_art_to_folder(dir, bytes, window, cx)
                }
                _ => {
                    this.set_error(format!("{} isn't an image", file_label(&image)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply_art_to_folder(
        &mut self,
        dir: PathBuf,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = library::save_folder_art(&dir, &bytes) {
            let msg = format!("Couldn't save cover.jpg in “{}”: {e}", file_label(&dir));
            window.push_notification(Notification::error(msg), cx);
        } else {
            self.settings.folder_art.insert(dir.clone());
            self.settings.save();
        }
        self.invalidate_thumbs(std::slice::from_ref(&dir.join("cover.jpg")));
        self.with_folder_tracks(
            dir,
            library::album_songs,
            window,
            cx,
            move |this, tracks, window, cx| {
                let edits = tracks
                    .into_iter()
                    .map(|t| {
                        (
                            t.path,
                            Edit {
                                cover: Some(CoverEdit::Set(bytes.clone())),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                this.write_edits(edits, String::new(), window, cx);
            },
        );
    }

    /// Ask, then remove the album's embedded art and move its cover file to the Trash.
    fn remove_folder_art(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let songs = library::album_songs(&dir).len();
        let message = format!("Remove the art from “{}”?", file_label(&dir));
        let detail = format!(
            "Art embedded in its {songs} song(s) is deleted and can't be recovered. Its cover image moves to the Trash."
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(&detail),
            &["Remove Art", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.remove_folder_art_now(dir, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn remove_folder_art_now(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Only this album's own cover files: its folder and its disc folders.
        let art_files: Vec<PathBuf> = std::iter::once(dir.clone())
            .chain(library::subfolders(&dir).into_iter().map(|e| e.path))
            .filter(|d| *d == dir || library::is_disc_folder(d))
            .filter_map(|d| library::folder_art(&d))
            .collect();
        let mut trashed = 0;
        for f in &art_files {
            match trash::delete(f) {
                Ok(()) => trashed += 1,
                Err(e) => {
                    let msg = format!("Couldn't move {} to the Trash: {e}", f.display());
                    window.push_notification(Notification::error(msg), cx);
                }
            }
        }
        self.settings.folder_art.remove(&dir);
        self.settings.save();
        self.invalidate_thumbs(&art_files);
        let note = if trashed > 0 {
            format!(" Moved {trashed} cover file(s) to the Trash.")
        } else {
            String::new()
        };
        self.with_folder_tracks(
            dir,
            library::album_songs,
            window,
            cx,
            move |this, tracks, window, cx| {
                let edits = tracks
                    .into_iter()
                    .filter(|t| t.has_cover)
                    .map(|t| {
                        (
                            t.path,
                            Edit {
                                cover: Some(CoverEdit::Remove),
                                ..Default::default()
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                if edits.is_empty() {
                    this.set_status(if note.is_empty() {
                        "No artwork to remove.".to_string()
                    } else {
                        note.trim().to_string()
                    });
                    cx.notify();
                } else {
                    this.write_edits(edits, note, window, cx);
                }
            },
        );
    }

    /// Look up art for this album online, save it as cover.jpg, and embed it.
    fn find_folder_art(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut resolver) = self.resolver() else {
            return;
        };
        self.with_folder_tracks(
            dir.clone(),
            library::album_songs,
            window,
            cx,
            move |this, tracks, window, cx| {
                // The album name comes from this folder (as an album), the artist
                // from the folder above it or the songs' own tags.
                let info = resolver.info(&tracks[0].path);
                let album = if info.album_dir.as_ref() == Some(&dir) {
                    info.album.clone().unwrap_or_default()
                } else {
                    library::clean_album(&file_label(&dir))
                };
                let artist = if info.album_dir.as_ref() == Some(&dir) {
                    info.artist.clone().unwrap_or_default()
                } else {
                    String::new()
                };
                this.busy = Some(format!("Searching artwork for {album}…").into());
                cx.notify();
                cx.spawn_in(window, async move |this, cx| {
                    let (a, b) = (artist.clone(), album.clone());
                    let sample: Vec<Track> = tracks.iter().take(5).cloned().collect();
                    let d = dir.clone();
                    let found = cx
                        .background_spawn(async move {
                            let bytes = lookup_album(&a, &b, &sample).ok()?.cover?;
                            let saved = library::save_folder_art(&d, &bytes).is_ok();
                            Some((bytes, saved))
                        })
                        .await;
                    this.update_in(cx, |this, window, cx| {
                        this.busy = None;
                        let Some((bytes, saved)) = found else {
                            this.set_error(format!("No artwork found for {album}."));
                            cx.notify();
                            return;
                        };
                        if saved {
                            this.settings.folder_art.insert(dir.clone());
                            this.settings.save();
                            this.invalidate_thumbs(std::slice::from_ref(&dir.join("cover.jpg")));
                        }
                        let edits = tracks
                            .into_iter()
                            .map(|t| {
                                (
                                    t.path,
                                    Edit {
                                        cover: Some(CoverEdit::Set(bytes.clone())),
                                        ..Default::default()
                                    },
                                )
                            })
                            .collect();
                        this.write_edits(edits, String::new(), window, cx);
                    })
                    .ok();
                })
                .detach();
            },
        );
    }

    fn reveal_in_finder(&mut self, path: &Path) {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }

    fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = file_label(&path);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(window, cx),
                _ => {}
            },
        );
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.renaming = Some(Renaming {
            path,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.renaming.take() else {
            return;
        };
        let new_name = tags::sanitize_filename(&r.input.read(cx).value());
        let old = r.path;
        cx.notify();
        if new_name.is_empty()
            || Some(new_name.as_str()) == old.file_name().and_then(|n| n.to_str())
        {
            return;
        }
        let new = old.with_file_name(&new_name);
        // Case-only renames are fine on macOS's case-insensitive disks.
        let clash = new.exists() && !new_name.eq_ignore_ascii_case(&file_label(&old));
        let result = if clash {
            Err(format!("a folder named “{new_name}” already exists there"))
        } else {
            std::fs::rename(&old, &new).map_err(|e| e.to_string())
        };
        if let Err(e) = result {
            let msg = format!("Couldn't rename “{}”: {e}", file_label(&old));
            window.push_notification(Notification::error(msg), cx);
            return;
        }

        // Follow the rename everywhere we hold paths.
        for col in &mut self.columns {
            col.dir = rebase(&col.dir, &old, &new);
            for e in &mut col.entries {
                if e.path == old {
                    e.name = new_name.clone();
                }
                e.path = rebase(&e.path, &old, &new);
            }
        }
        if let Some(d) = &self.current_dir {
            self.current_dir = Some(rebase(d, &old, &new));
        }
        for t in Arc::make_mut(&mut self.tracks).iter_mut() {
            t.path = rebase(&t.path, &old, &new);
        }
        self.selected = self
            .selected
            .iter()
            .map(|p| rebase(p, &old, &new))
            .collect();
        let roles = std::mem::take(&mut self.settings.roles);
        self.settings.roles = roles
            .into_iter()
            .map(|(p, r)| (rebase(&p, &old, &new), r))
            .collect();
        self.settings.save();
        self.set_status("");
    }

    // ----- Rendering -----------------------------------------------------

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let pending = self.has_pending(cx);

        // Breadcrumb: Local Music › Drake › Certified Lover Boy. Clicking a
        // crumb shows every song in that folder.
        let mut crumbs: Vec<(PathBuf, String)> = vec![];
        if let Some(root) = &self.root {
            crumbs.push((root.clone(), file_label(root)));
            if let Some(cur) = &self.current_dir
                && let Ok(rel) = cur.strip_prefix(root)
            {
                let mut p = root.clone();
                for c in rel.components() {
                    p = p.join(c);
                    crumbs.push((p.clone(), c.as_os_str().to_string_lossy().into_owned()));
                }
            }
        }
        let last = crumbs.len().saturating_sub(1);
        let breadcrumb = h_flex()
            .gap_1()
            .items_center()
            .min_w_0()
            .overflow_hidden()
            .children(crumbs.into_iter().enumerate().map(|(i, (path, name))| {
                let is_last = i == last;
                h_flex()
                    .gap_1()
                    .items_center()
                    .flex_shrink_0()
                    .when(i > 0, |d| {
                        d.child(
                            Icon::empty()
                                .path("icons/chevron-right.svg")
                                .size_3()
                                .text_color(theme.muted_foreground.opacity(0.6)),
                        )
                    })
                    .child(
                        div()
                            .id(("crumb", i))
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .text_size(px(13.))
                            .when(is_last, |d| {
                                d.font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                            })
                            .when(!is_last, |d| {
                                d.text_color(theme.muted_foreground)
                                    .cursor_pointer()
                                    .hover(|s| {
                                        s.bg(overlay(&theme, 0.08)).text_color(theme.foreground)
                                    })
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(ix) = this.columns.iter().position(|c| c.dir == path) {
                                    this.select_column_root(ix, window, cx);
                                }
                            }))
                            .child(name),
                    )
            }));

        let icon_button = |id: &'static str, icon: &'static str| {
            Button::new(id)
                .small()
                .ghost()
                .icon(Icon::empty().path(icon).text_color(theme.muted_foreground))
        };
        TitleBar::new()
            .bg(chrome_bg(&theme))
            .border_color(theme.border.opacity(0.5))
            .child(
            h_flex()
                .w_full()
                .pr_2()
                .gap_1()
                .items_center()
                .child(
                    Icon::empty()
                        .path("icons/tags.svg")
                        .size_4()
                        .text_color(accent(&theme)),
                )
                .child(div().ml_1().child(breadcrumb))
                .child(div().flex_1())
                .child(with_tip(
                    "open-tip",
                    "Open folder (⌘O)",
                    icon_button("open", "icons/folder-open.svg")
                        .on_click(cx.listener(|this, _, window, cx| this.prompt_open(window, cx))),
                ))
                .child(with_tip(
                    "reload-tip",
                    "Reload (⌘R)",
                    icon_button("reload", "icons/refresh-cw.svg")
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                ))
                .child(with_tip(
                    "theme-tip",
                    "Toggle light/dark",
                    icon_button(
                        "theme",
                        if theme.is_dark() {
                            "icons/sun.svg"
                        } else {
                            "icons/moon.svg"
                        },
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
                ))
                .child(div().w(px(1.)).h(px(16.)).mx_1().bg(theme.border))
                .child(with_tip(
                    "autotag-tip",
                    "Fills Artist, Album and art for the selected songs, or every song shown (⌘⇧T)",
                    Button::new("autotag")
                        .small()
                        .ghost()
                        .icon(Icon::empty().path("icons/wand-sparkles.svg").text_color(theme.muted_foreground))
                        .label("Auto-tag")
                        .loading(self.busy.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.auto_tag(window, cx))),
                ))
                // Save only shows up when there's something to save.
                .when(pending, |d| {
                    let blue = accent(&theme);
                    d.child(with_tip(
                        "save-tip",
                        "Save (⌘S or Return)",
                        Button::new("save")
                            .small()
                            .rounded(px(999.))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(blue)
                                    .foreground(gpui_kit::white())
                                    .hover(blue.opacity(0.88))
                                    .active(blue.opacity(0.75)),
                            )
                            .label("Save")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ))
                }),
        )
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        cx.notify();
    }

    fn render_columns(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.current_dir.clone();
        let root = self.root.clone();

        if self.columns.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .p_4()
                .bg(theme.sidebar)
                .child(
                    Icon::empty()
                        .path("icons/folder-open.svg")
                        .size_8()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    Button::new("open-empty")
                        .primary()
                        .small()
                        .label("Open Folder…")
                        .on_click(cx.listener(|this, _, window, cx| this.prompt_open(window, cx))),
                )
                .into_any_element();
        }

        let has_leaf = self.leaf_folder().is_some();
        let last_ix = self.columns.len().saturating_sub(1);
        let columns = self.columns.iter().enumerate().map(|(col_ix, col)| {
            // The rightmost column stretches to fill the browser.
            let stretch = !has_leaf && col_ix == last_ix;
            let title = if Some(&col.dir) == root.as_ref() {
                col.dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Library".into())
            } else {
                col.dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            let header_active = current.as_ref() == Some(&col.dir);
            let rows: Arc<Vec<(FolderEntry, Role, bool)>> = Arc::new(
                col.entries
                    .iter()
                    .map(|e| {
                        let (role, explicit) = self.folder_role(e);
                        (e.clone(), role, explicit)
                    })
                    .collect(),
            );
            let count = rows.len();
            let selected = col.selected;
            let theme = theme.clone();
            let view = cx.entity().downgrade();
            let renaming = self
                .renaming
                .as_ref()
                .map(|r| (r.path.clone(), r.input.clone()));
            let thumbs: Arc<Vec<Option<Arc<Image>>>> = Arc::new(
                col.entries
                    .iter()
                    .map(|e| self.thumbs.get(&e.path).cloned().flatten())
                    .collect(),
            );
            let col_dir = col.dir.clone();
            folder_drop_target(
                v_flex()
                    .id(("column", col_ix))
                    .h_full()
                    .when(stretch, |d| d.flex_1().min_w(px(COLUMN_WIDTH)))
                    .when(!stretch, |d| d.w(px(COLUMN_WIDTH)).flex_none())
                    .border_r_1()
                    .border_color(theme.sidebar_border.opacity(0.6)),
                &cx.entity().downgrade(),
                col_dir.clone(),
            )
            .test_support()
            .child(
                column_header(&theme, title, count, header_active)
                    .id(("col-header", col_ix))
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.foreground))
                    .tooltip(tip("Show every song in this folder"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_column_root(col_ix, window, cx)
                    })),
            )
            .child(
                uniform_list(("col-list", col_ix), count, move |range, _window, _cx| {
                    range
                        .map(|i| {
                            let (e, role, explicit) = rows[i].clone();
                            let is_sel = selected == Some(i);
                            let renaming_this = renaming
                                .as_ref()
                                .filter(|(p, _)| *p == e.path)
                                .map(|(_, input)| input.clone());
                            folder_row(
                                &theme,
                                &view,
                                col_ix,
                                i,
                                e,
                                role,
                                explicit,
                                is_sel,
                                renaming_this,
                                thumbs[i].clone(),
                            )
                        })
                        .collect()
                })
                .py_1()
                .px_1()
                .flex_1()
                .track_scroll(&col.scroll)
                .map(lock_scroll_axis),
            )
            .context_menu({
                let view = cx.entity().downgrade();
                move |menu, _, _| new_folder_menu(menu, &view, col_dir.clone())
            })
            .into_any_element()
        });
        // The selected folder has no subfolders: give it an empty column.
        let leaf = self.leaf_folder().map(|dir| {
            let name = file_label(&dir);
            let view = cx.entity().downgrade();
            folder_drop_target(
                v_flex()
                    .id("leaf-column")
                    .h_full()
                    .flex_1()
                    .min_w(px(COLUMN_WIDTH))
                    .border_r_1()
                    .border_color(theme.sidebar_border.opacity(0.6)),
                &view,
                dir.clone(),
            )
            .test_support()
            .child(column_header(&theme, name, 0, true))
            .child(div().flex_1())
            .context_menu(move |menu, _, _| new_folder_menu(menu, &view, dir.clone()))
        });

        div()
            .id("columns")
            .size_full()
            .bg(panel_bg(&theme))
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&self.columns_scroll)
            .child(
                h_flex()
                    .h_full()
                    .min_w_full()
                    .children(columns)
                    .children(leaf),
            )
            .into_any_element()
    }

    /// The header above the song list: the folder's art, name and a summary.
    fn render_folder_header(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let dir = self.current_dir.clone()?;
        let is_root = self.root.as_ref() == Some(&dir);
        let role = if is_root { None } else { self.current_role() };

        // Shared tags make a better title than the folder name.
        let common = |get: fn(&Track) -> &str| -> Option<String> {
            let first = get(self.tracks.first()?);
            (!first.is_empty() && self.tracks.iter().all(|t| get(t) == first))
                .then(|| first.to_string())
        };
        let (kind, title) = match role {
            Some(Role::Album) => (
                "Album",
                common(|t| t.album.as_str())
                    .unwrap_or_else(|| library::clean_album(&file_label(&dir))),
            ),
            Some(Role::Artist) => ("Artist", file_label(&dir)),
            _ if is_root => ("Library", file_label(&dir)),
            _ => ("Folder", file_label(&dir)),
        };
        let artist = (role == Some(Role::Album))
            .then(|| common(|t| t.album_artist.as_str()).or_else(|| common(|t| t.artist.as_str())))
            .flatten();
        let year = common(|t| t.year.as_str()).map(|y| y.chars().take(4).collect::<String>());
        let songs = self.tracks.len();
        let secs: u64 = self.tracks.iter().map(|t| t.duration_secs).sum();
        let length = if secs >= 3600 {
            format!("{} hr {} min", secs / 3600, (secs / 60) % 60)
        } else {
            format!("{} min", (secs + 59) / 60)
        };
        let albums = (role == Some(Role::Artist) || is_root).then(|| {
            let set: HashSet<&Path> = self
                .tracks
                .iter()
                .filter_map(|t| library::album_dir_of(&t.path))
                .collect();
            set.len()
        });
        let mut facts: Vec<String> = vec![];
        facts.extend(artist);
        facts.extend(year.filter(|_| role == Some(Role::Album)));
        if let Some(n) = albums.filter(|n| *n > 1) {
            facts.push(format!("{n} albums"));
        }
        if songs > 0 {
            facts.push(format!(
                "{songs} {}, {length}",
                if songs == 1 { "song" } else { "songs" }
            ));
        }

        let art = self
            .header_art
            .as_ref()
            .filter(|(d, _)| *d == dir)
            .and_then(|(_, a)| a.clone());
        // Artists and the library get a mosaic of their album covers.
        let mosaic: Vec<Arc<Image>> = if role == Some(Role::Album) {
            vec![]
        } else {
            let mut seen: Vec<&Path> = vec![];
            let mut covers = vec![];
            for t in self.tracks.iter() {
                if let Some(d) = library::album_dir_of(&t.path)
                    && !seen.contains(&d)
                {
                    seen.push(d);
                    if let Some(Some(img)) = self.row_art.get(d) {
                        covers.push(img.clone());
                        if covers.len() == 4 {
                            break;
                        }
                    }
                }
            }
            covers
        };
        let tile: AnyElement = if mosaic.len() == 4 && role != Some(Role::Album) {
            div()
                .size(px(104.))
                .flex_none()
                .rounded_lg()
                .overflow_hidden()
                .shadow_lg()
                .flex()
                .flex_wrap()
                .children(mosaic.into_iter().map(|img_| {
                    div()
                        .size(px(52.))
                        .child(img(img_).size_full().object_fit(ObjectFit::Cover))
                }))
                .into_any_element()
        } else {
            match art {
                Some(art) => div()
                    .size(px(104.))
                    .flex_none()
                    .rounded_lg()
                    .overflow_hidden()
                    .shadow_lg()
                    .border_1()
                    .border_color(theme.border.opacity(0.4))
                    .child(img(art).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element(),
                None => div()
                    .size(px(104.))
                    .flex_none()
                    .rounded_lg()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(overlay(&theme, 0.08))
                    .child(
                        Icon::empty()
                            .path(match role {
                                Some(r) => r.icon(),
                                None if is_root => "icons/library.svg",
                                None => "icons/folder.svg",
                            })
                            .size_10()
                            .text_color(theme.muted_foreground),
                    )
                    .into_any_element(),
            }
        };

        let view = cx.entity().downgrade();
        Some(
            h_flex()
                .id("folder-header")
                .flex_none()
                .px_5()
                .pt_5()
                .pb_4()
                .gap_5()
                .items_end()
                .bg(content_bg(&theme))
                .child(tile)
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.muted_foreground)
                                .child(kind),
                        )
                        .child(
                            div()
                                .text_size(px(26.))
                                .line_height(px(32.))
                                .font_weight(FontWeight::BOLD)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme.muted_foreground)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(facts.join(" · ")),
                        )
                        .child(
                            h_flex()
                                .pt_2()
                                .ml(px(-8.)) // ghost buttons have their own padding; align their text with the title
                                .gap_1()
                                .when(role == Some(Role::Album), |d| {
                                    let (view, dir) = (view.clone(), dir.clone());
                                    d.child(
                                        Button::new("header-art")
                                            .small()
                                            .ghost()
                                            .icon(
                                                Icon::empty()
                                                    .path("icons/search.svg")
                                                    .text_color(theme.muted_foreground),
                                            )
                                            .label("Find Art Online")
                                            .on_click(move |_, window, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.find_folder_art(dir.clone(), window, cx)
                                                })
                                                .ok();
                                            }),
                                    )
                                })
                                .child({
                                    let (view, dir) = (view.clone(), dir.clone());
                                    Button::new("header-reveal")
                                        .small()
                                        .ghost()
                                        .icon(
                                            Icon::empty()
                                                .path("icons/folder-search.svg")
                                                .text_color(theme.muted_foreground),
                                        )
                                        .label("Show in Finder")
                                        .on_click(move |_, _, cx| {
                                            view.update(cx, |this, _| this.reveal_in_finder(&dir))
                                                .ok();
                                        })
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Inside an album the header already shows its art; rows show art
        // only when the folder mixes albums.
        let in_album = self.current_role() == Some(Role::Album);
        let show_art = !in_album && self.tracks.iter().any(|t| t.has_cover);
        // The measured pane width once known; an estimate on the first frame.
        let measured = self.songs_pane_width.get();
        let pane = if measured > 0. {
            measured - ROW_INSET * 2.
        } else {
            (window.viewport_size().width - self.browser_width - self.inspector_width).as_f32()
                - 2.
                - ROW_INSET * 2.
        };
        let show_explicit = self.tracks.iter().any(|t| t.explicit);
        let cols = Arc::new(grid_columns(show_art, show_explicit, pane));
        let width: f32 = cols.iter().map(|(_, w)| w).sum();
        let sort = self.sort;
        let header = h_flex()
            .id("grid-header")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .h(px(34.))
            .w(px(width + ROW_INSET * 2.))
            .px(px(ROW_INSET))
            .flex_none()
            .border_b_1()
            .border_color(theme.border.opacity(0.7))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.muted_foreground)
            .children(cols.iter().map(|&(col, col_width)| {
                let arrow = match sort {
                    Some((c, true)) if c == col => Some("icons/chevron-up.svg"),
                    Some((c, false)) if c == col => Some("icons/chevron-down.svg"),
                    _ => None,
                };
                h_flex()
                    .id(SharedString::from(format!("h-{col:?}")))
                    .w(px(col_width))
                    .flex_none()
                    .h_full()
                    .px_2()
                    .gap_1()
                    .items_center()
                    .when(col.numeric(), |d| d.justify_end())
                    .when(matches!(col, Col::Cover | Col::Explicit), |d| {
                        d.justify_center()
                    })
                    .when(arrow.is_some(), |d| d.text_color(theme.foreground))
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.foreground))
                    .on_click(cx.listener(move |this, _, _, cx| this.click_header(col, cx)))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(col.title()),
                    )
                    .when_some(arrow, |d, icon| {
                        d.child(Icon::empty().path(icon).size_3().text_color(accent(&theme)))
                    })
            }));

        let accent_color = accent(&theme);
        let tracks = self.tracks.clone();
        let row_cols = cols.clone();
        let row_art: Arc<Vec<Option<Arc<Image>>>> = Arc::new(
            self.tracks
                .iter()
                .map(|t| {
                    if in_album {
                        return None;
                    }
                    library::album_dir_of(&t.path)
                        .and_then(|d| self.row_art.get(d).cloned().flatten())
                })
                .collect(),
        );
        let selected: Arc<Vec<bool>> = Arc::new(
            self.tracks
                .iter()
                .map(|t| self.selected.contains(&t.path))
                .collect(),
        );
        let count = tracks.len();
        let root = self.root.clone();
        let view = cx.entity().downgrade();
        let dragged_selection = Arc::new(self.selected_paths());
        let focused = self.grid_focus.contains_focused(window, cx);
        let row_theme = theme.clone();
        let list = uniform_list("grid", count, move |range, _window, _cx| {
            let theme = &row_theme;
            range
                .map(|ix| {
                    let t = &tracks[ix];
                    let is_sel = selected[ix];
                    // Neighbouring selected rows merge into one rounded block.
                    let sel_above = ix > 0 && selected[ix - 1];
                    let sel_below = selected.get(ix + 1).copied().unwrap_or(false);
                    let view = view.clone();
                    let on_accent = is_sel && focused;
                    let fg = if on_accent {
                        gpui_kit::white()
                    } else if t.error.is_some() {
                        theme.danger
                    } else {
                        theme.foreground
                    };
                    let quiet = if on_accent {
                        gpui_kit::white().opacity(0.75)
                    } else {
                        theme.muted_foreground
                    };
                    h_flex()
                        .id(("row", ix))
                        .test_support()
                        .h(px(ROW_HEIGHT))
                        .w(px(width))
                        .mx(px(ROW_INSET))
                        .rounded_md()
                        .when(is_sel && sel_above, |d| d.rounded_t_none())
                        .when(is_sel && sel_below, |d| d.rounded_b_none())
                        .text_size(px(13.))
                        .text_color(fg)
                        .when(!is_sel, |d| d.hover(|s| s.bg(overlay(theme, 0.06))))
                        .when(is_sel, |d| {
                            d.bg(if focused {
                                accent_color
                            } else {
                                accent_color.opacity(0.22)
                            })
                        })
                        .on_mouse_down(MouseButton::Left, {
                            let view = view.clone();
                            move |e, window, cx| {
                                let modifiers = e.modifiers;
                                view.update(cx, |this, cx| {
                                    this.row_pressed = true;
                                    this.click_row(ix, modifiers, window, cx)
                                })
                                .ok();
                            }
                        })
                        // Right-clicking an unselected song selects it first.
                        .on_mouse_down(MouseButton::Right, {
                            let view = view.clone();
                            move |_, window, cx| {
                                if !is_sel {
                                    view.update(cx, |this, cx| {
                                        this.click_row(ix, Modifiers::default(), window, cx)
                                    })
                                    .ok();
                                }
                            }
                        })
                        .on_click({
                            let view = view.clone();
                            move |_, window, cx| {
                                view.update(cx, |this, cx| this.row_clicked(ix, window, cx))
                                    .ok();
                            }
                        })
                        .on_drag(
                            // Grabbing a selected song drags the whole selection.
                            DraggedSongs(if is_sel {
                                dragged_selection.as_ref().clone()
                            } else {
                                vec![t.path.clone()]
                            }),
                            |songs, _, _, cx| {
                                let label: SharedString = match songs.0.len() {
                                    1 => file_label(&songs.0[0]).into(),
                                    n => format!("{n} songs").into(),
                                };
                                cx.new(|_| DragPreview {
                                    icon: "icons/music.svg",
                                    label,
                                })
                            },
                        )
                        .children(row_cols.iter().map(|&(col, col_width)| {
                            let cell = div()
                                .w(px(col_width))
                                .flex_none()
                                .px_2()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .when(col.secondary() && t.error.is_none(), |d| {
                                    d.text_color(quiet)
                                })
                                .when(col.numeric(), |d| d.flex().justify_end())
                                .when(matches!(col, Col::Cover | Col::Explicit), |d| {
                                    d.flex().items_center().justify_center()
                                });
                            match col {
                                // A small "E" badge, like a music app.
                                Col::Explicit => cell.when(t.explicit, |d| {
                                    d.child(
                                        div()
                                            .size(px(15.))
                                            .rounded(px(3.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(quiet.opacity(0.3))
                                            .text_size(px(9.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(fg)
                                            .child("E"),
                                    )
                                }),
                                // The song's album art (or a hint that it has some).
                                Col::Cover => match row_art[ix].clone() {
                                    Some(art) => cell.child(
                                        div()
                                            .size(px(22.))
                                            .rounded(px(4.))
                                            .overflow_hidden()
                                            .child(
                                                img(art).size_full().object_fit(ObjectFit::Cover),
                                            ),
                                    ),
                                    None => cell.when(t.has_cover && !in_album, |d| {
                                        d.child(
                                            Icon::empty()
                                                .path("icons/image.svg")
                                                .size_3p5()
                                                .text_color(quiet),
                                        )
                                    }),
                                },
                                _ => cell.child(col.text(t, root.as_deref())),
                            }
                        }))
                })
                .collect()
        })
        .flex_1()
        .track_scroll(&self.grid_scroll)
        .map(lock_scroll_axis);

        // With no rows, say why in the middle of the visible pane (the table
        // itself is wider than the pane, so its centre would be off-screen).
        let empty_message: Option<&'static str> = if !self.tracks.is_empty() {
            None
        } else if self.loading {
            Some("Loading songs…")
        } else if self.root.is_some() {
            Some("No songs in this folder.")
        } else {
            None
        };
        let has_rows = !self.tracks.is_empty();
        let body: AnyElement = if has_rows {
            list.into_any_element()
        } else {
            div().flex_1().into_any_element()
        };

        let list = div()
            .id("grid-scroll")
            .test_support()
            .key_context(GRID_CONTEXT)
            .track_focus(&self.grid_focus)
            .on_action(cx.listener(|this, _: &TrashSelection, window, cx| {
                let songs = this.selected_paths();
                this.trash_paths(songs, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAll, window, cx| {
                let all = this.tracks.iter().map(|t| t.path.clone()).collect();
                this.set_selection(all, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectNext, w, cx| this.move_cursor(1, false, w, cx)))
            .on_action(
                cx.listener(|this, _: &SelectPrev, w, cx| this.move_cursor(-1, false, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ExtendNext, w, cx| this.move_cursor(1, true, w, cx)))
            .on_action(cx.listener(|this, _: &ExtendPrev, w, cx| this.move_cursor(-1, true, w, cx)))
            // Clicking empty space below the rows clears the selection.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.grid_focus, cx);
                    // A press on a song row bubbles here too; only empty space clears.
                    if std::mem::take(&mut this.row_pressed) {
                        return;
                    }
                    this.clear_selection(window, cx);
                }),
            )
            .size_full()
            .bg(content_bg(&theme))
            .overflow_x_scroll()
            // Both nested scroll areas lock to the swipe's direction, so a
            // slightly diagonal swipe doesn't drift sideways.
            .restrict_scroll_to_axis()
            .track_scroll(&self.grid_hscroll)
            .child(
                v_flex()
                    .h_full()
                    .w(px(width + ROW_INSET * 2.))
                    .when(has_rows, |d| d.child(header))
                    .child(div().h(px(4.)).flex_none())
                    .child(body),
            )
            .context_menu({
                let view = cx.entity().downgrade();
                let dir = self.current_dir.clone();
                move |menu, _, cx| {
                    let Some(dir) = dir.clone() else { return menu };
                    // Read the selection now: right-clicking a song selects it
                    // just before the menu opens.
                    let has_selection = view
                        .upgrade()
                        .is_some_and(|v| !v.read(cx).selected.is_empty());
                    let menu = new_folder_menu(menu, &view, dir.clone());
                    let v = view.clone();
                    let menu = menu.when(has_selection, |menu| {
                        menu.item(
                            PopupMenuItem::new("New Folder with Selection")
                                .icon(Icon::empty().path("icons/folder-input.svg"))
                                .on_click(move |_, window, cx| {
                                    v.update(cx, |this, cx| {
                                        this.new_folder_with_selection(window, cx)
                                    })
                                    .ok();
                                }),
                        )
                    });
                    let v = view.clone();
                    let menu = menu.when(has_selection, |menu| {
                        let v = v.clone();
                        menu.separator().item(
                            PopupMenuItem::new("Move to Trash")
                                .icon(Icon::empty().path("icons/trash.svg"))
                                .on_click(move |_, window, cx| {
                                    v.update(cx, |this, cx| {
                                        let songs = this.selected_paths();
                                        this.trash_paths(songs, window, cx)
                                    })
                                    .ok();
                                }),
                        )
                    });
                    menu.separator().item(
                        PopupMenuItem::new("Show in Finder")
                            .icon(Icon::empty().path("icons/folder-search.svg"))
                            .on_click(move |_, _, cx| {
                                let dir = dir.clone();
                                v.update(cx, |this, _| {
                                    let target =
                                        this.selected_paths().into_iter().next().unwrap_or(dir);
                                    this.reveal_in_finder(&target);
                                })
                                .ok();
                            }),
                    )
                }
            });
        div()
            .relative()
            .size_full()
            .child(list)
            .when_some(empty_message, |d, message| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child(message),
                )
            })
    }

    fn render_tag_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.selected.len();
        let field = |this: &Self, f: Field, cx: &App| -> AnyElement {
            let fi = this.fields.iter().find(|x| x.field == f).expect("field");
            let dirty = this.field_dirty(fi, cx);
            v_flex()
                .gap(px(4.))
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_size(px(11.5))
                        .text_color(theme.muted_foreground)
                        .child(f.label())
                        .when(dirty, |d| {
                            d.child(div().size(px(6.)).rounded_full().bg(accent(&theme)))
                        }),
                )
                .child({
                    let focused = fi.state.read(cx).focus_handle(cx).is_focused(&*window);
                    Input::new(&fi.state)
                        .id(SharedString::from(format!("field-{}", f.label())))
                        .small()
                        .focus_bordered(false)
                        .bg(overlay(&theme, 0.06))
                        .border_color(overlay(&theme, 0.12))
                        .when(focused, |i| i.border_color(accent(&theme).opacity(0.6)))
                })
                .into_any_element()
        };
        let section = |title: &'static str| {
            div()
                .pt_3()
                .pb_0p5()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground.opacity(0.85))
                .child(title)
        };

        // ----- Cover art, front and centre.
        let cover = {
            let inner: AnyElement = match &self.cover.image {
                Some(image) => img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(
                        Icon::empty()
                            .path("icons/image.svg")
                            .size_8()
                            .text_color(theme.muted_foreground.opacity(0.6)),
                    )
                    .child(if self.cover.mixed {
                        "< keep >"
                    } else {
                        "Drop an image here"
                    })
                    .into_any_element(),
            };
            div()
                .id("cover")
                .w_full()
                .aspect_square()
                .rounded_xl()
                .overflow_hidden()
                .border_1()
                .border_color(if self.cover.pending.is_some() {
                    accent(&theme)
                } else {
                    theme.border.opacity(0.6)
                })
                .when(self.cover.pending.is_some(), |d| d.border_2())
                .bg(overlay(&theme, 0.06))
                .shadow_lg()
                .drag_over::<ExternalPaths>(|s, _, _, cx| {
                    s.border_color(cx.theme().ring).border_2()
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    if this.selected.is_empty() {
                        this.set_status("Select songs first.");
                    } else if let Some(p) = paths.paths().first() {
                        this.load_cover_file(&p.clone(), cx);
                    }
                    cx.notify();
                }))
                .child(inner)
        };
        let cover_actions = h_flex()
            .gap_0p5()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(self.cover.info.clone()),
            )
            .child(with_tip(
                "cover-add-tip",
                "Choose an image…",
                Button::new("cover-add")
                    .xsmall()
                    .ghost()
                    .icon(
                        Icon::empty()
                            .path("icons/image-plus.svg")
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.choose_cover_file(window, cx)),
                    ),
            ))
            .child(with_tip(
                "cover-find-tip",
                "Find art online",
                Button::new("cover-find")
                    .xsmall()
                    .ghost()
                    .icon(
                        Icon::empty()
                            .path("icons/search.svg")
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.find_cover_online(window, cx)),
                    ),
            ))
            .child(with_tip(
                "cover-remove-tip",
                "Remove cover",
                Button::new("cover-remove")
                    .xsmall()
                    .ghost()
                    .icon(
                        Icon::empty()
                            .path("icons/trash.svg")
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.remove_cover(cx))),
            ));

        // ----- What's selected, in big type.
        let sel = self.selected_tracks();
        let common = |get: fn(&Track) -> &str| -> Option<String> {
            let first = get(sel.first()?);
            (!first.is_empty() && sel.iter().all(|t| get(t) == first)).then(|| first.to_string())
        };
        let (headline, subline) = match sel.len() {
            0 => (String::new(), String::new()),
            1 => {
                let t = sel[0];
                let title = if t.title.is_empty() {
                    t.file_stem()
                } else {
                    t.title.clone()
                };
                let sub = [t.artist.as_str(), t.album.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" — ");
                (title, sub)
            }
            n => {
                let sub = [common(|t| t.artist.as_str()), common(|t| t.album.as_str())]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" — ");
                (format!("{n} songs"), sub)
            }
        };

        let directory = {
            let dirs: HashSet<_> = sel
                .iter()
                .filter_map(|t| t.path.parent().map(Path::to_path_buf))
                .collect();
            match dirs.len() {
                0 => String::new(),
                1 => {
                    let dir = dirs.into_iter().next().unwrap();
                    let root = self.root.as_ref().and_then(|r| r.parent());
                    root.and_then(|r| dir.strip_prefix(r).ok())
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| dir.display().to_string())
                }
                _ => "< multiple folders >".into(),
            }
        };

        let explicit_row = {
            let mixed = self.explicit.original.is_none() && self.explicit.pending.is_none();
            let dirty = self.explicit.change().is_some();
            h_flex()
                .pt_1()
                .gap_2()
                .items_center()
                .child(
                    h_flex()
                        .flex_1()
                        .gap_1()
                        .items_center()
                        .text_size(px(13.))
                        .child(if mixed {
                            "Explicit (mixed)"
                        } else {
                            "Explicit"
                        })
                        .when(dirty, |d| {
                            d.child(div().size(px(6.)).rounded_full().bg(accent(&theme)))
                        }),
                )
                .child(
                    Switch::new("explicit")
                        .checked(self.explicit.shown())
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.explicit.pending = Some(*checked);
                            cx.notify();
                        })),
                )
        };

        let body = if count == 0 {
            v_flex()
                .flex_1()
                .min_h(px(320.))
                .items_center()
                .justify_center()
                .gap_3()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .size(px(56.))
                        .rounded_xl()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(overlay(&theme, 0.08))
                        .child(Icon::empty().path("icons/music.svg").size_6()),
                )
                .child("Select songs to edit their tags.")
                .into_any_element()
        } else {
            v_flex()
                .gap_2()
                .child(cover)
                .child(cover_actions)
                .child(
                    v_flex()
                        .pt_1()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .line_height(px(22.))
                                .child(headline),
                        )
                        .when(!subline.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme.muted_foreground)
                                    .child(subline),
                            )
                        }),
                )
                .child(section("Song"))
                .child(field(self, Field::Title, cx))
                .child(field(self, Field::Artist, cx))
                .child(field(self, Field::Album, cx))
                .child(field(self, Field::AlbumArtist, cx))
                .child(section("Details"))
                .child(
                    h_flex()
                        .gap_2()
                        .child(field(self, Field::Year, cx))
                        .child(field(self, Field::Track, cx))
                        .child(field(self, Field::Disc, cx)),
                )
                .child(field(self, Field::Genre, cx))
                .child(field(self, Field::Composer, cx))
                .child(explicit_row)
                .child(field(self, Field::Comment, cx))
                .child(section("File"))
                .child(field(self, Field::Filename, cx))
                .child(
                    v_flex()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme.muted_foreground)
                                .child("Directory"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.foreground.opacity(0.85))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(directory),
                        ),
                )
                .into_any_element()
        };

        v_flex()
            .id("tag-panel")
            .size_full()
            .bg(panel_bg(&theme))
            .border_l_1()
            .border_color(theme.sidebar_border.opacity(0.6))
            .overflow_y_scroll()
            .child(v_flex().px_4().pt_4().pb_6().gap_2().child(body))
    }

    /// A slim bar for messages and work in progress. It's hidden when there's
    /// nothing to say (the folder header already shows counts and length).
    fn render_status(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let msg = self.busy.clone().unwrap_or_else(|| self.status.clone());
        if msg.is_empty() {
            return None;
        }
        let busy = self.busy.is_some();
        let error = self.status_error && !busy;
        Some(
            h_flex()
                .h(px(28.))
                .flex_none()
                .px_4()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(theme.border.opacity(0.5))
                .bg(chrome_bg(theme))
                .text_size(px(12.))
                .text_color(if error {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .when(busy, |d| {
                    d.child(div().size(px(6.)).rounded_full().bg(accent(theme)))
                })
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(msg),
                )
                .into_any_element(),
        )
    }
}

/// easytag's accent: a clear blue for selection and highlights (the theme's
/// "primary" is near-white in dark mode and black in light mode).
fn accent(theme: &Theme) -> Hsla {
    // macOS system blue.
    if theme.is_dark() {
        rgb(0x0a84ff).into()
    } else {
        rgb(0x007aff).into()
    }
}

// The window is frosted glass (the desktop shows through, blurred). Each
// surface is a translucent tint over it: side panels the most see-through,
// the song list mostly solid so text stays easy to read.

/// A translucent fill that lightens (dark mode) or darkens (light mode)
/// whatever is behind it, so tiles, badges, hovers and selections stay glass.
fn overlay(theme: &Theme, amount: f32) -> Hsla {
    if theme.is_dark() {
        gpui_kit::white().opacity(amount)
    } else {
        gpui_kit::black().opacity(amount)
    }
}

/// Side panels (folder browser, inspector).
fn panel_bg(theme: &Theme) -> Hsla {
    if theme.is_dark() {
        theme.background.opacity(0.55)
    } else {
        theme.sidebar.opacity(0.68)
    }
}

/// Title bar and status bar.
fn chrome_bg(theme: &Theme) -> Hsla {
    if theme.is_dark() {
        theme.background.opacity(0.62)
    } else {
        theme.background.opacity(0.72)
    }
}

/// The song list in the middle.
fn content_bg(theme: &Theme) -> Hsla {
    // The same glass as the side panels: one continuous surface.
    panel_bg(theme)
}

/// A folder column's header: its folder name and how many folders it holds.
fn column_header(theme: &Theme, title: String, count: usize, active: bool) -> Div {
    h_flex()
        .h(px(36.))
        .flex_none()
        .px_3()
        .gap_2()
        .items_center()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(title),
        )
        .child(
            div()
                .px_1p5()
                .rounded_full()
                .bg(overlay(theme, 0.10))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(count.to_string()),
        )
}

/// Make `el` accept dropped songs and folders, moving them into `dir`.
fn folder_drop_target(
    el: Stateful<Div>,
    view: &WeakEntity<MainWindow>,
    dir: PathBuf,
) -> Stateful<Div> {
    let (v1, d1) = (view.clone(), dir.clone());
    let (v2, d2) = (view.clone(), dir);
    el.drag_over::<DraggedSongs>(|s, _, _, cx| s.bg(cx.theme().drop_target))
        .drag_over::<DraggedFolder>(|s, _, _, cx| s.bg(cx.theme().drop_target))
        .on_drop(move |songs: &DraggedSongs, window, cx| {
            cx.stop_propagation();
            let (items, dest) = (songs.0.clone(), d1.clone());
            v1.update(cx, |this, cx| this.move_paths(items, dest, window, cx))
                .ok();
        })
        .on_drop(move |folder: &DraggedFolder, window, cx| {
            cx.stop_propagation();
            let (items, dest) = (vec![folder.0.clone()], d2.clone());
            v2.update(cx, |this, cx| this.move_paths(items, dest, window, cx))
                .ok();
        })
}

fn new_folder_menu(menu: PopupMenu, view: &WeakEntity<MainWindow>, dir: PathBuf) -> PopupMenu {
    let view = view.clone();
    menu.item(
        PopupMenuItem::new("New Folder")
            .icon(Icon::empty().path("icons/folder-plus.svg"))
            .on_click(move |_, window, cx| {
                let dir = dir.clone();
                view.update(cx, |this, cx| {
                    this.new_folder(dir, window, cx);
                })
                .ok();
            }),
    )
}

/// The "This folder is…" choices, shared by the right-click menu and the role icon.
fn role_items(
    menu: PopupMenu,
    view: &WeakEntity<MainWindow>,
    path: &Path,
    role: Role,
    explicit: bool,
    auto_role: Role,
) -> PopupMenu {
    let mut menu = menu.label("This folder is…");
    for r in [Role::Artist, Role::Album, Role::Neither] {
        let (view, path) = (view.clone(), path.to_path_buf());
        menu = menu.item(
            PopupMenuItem::new(r.label())
                .checked(explicit && role == r)
                .on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| {
                        this.set_folder_role(path.clone(), Some(r), cx)
                    })
                    .ok();
                }),
        );
    }
    let (view, path) = (view.clone(), path.to_path_buf());
    menu.item(
        PopupMenuItem::new(format!("Automatic ({})", auto_role.label()))
            .checked(!explicit)
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.set_folder_role(path.clone(), None, cx))
                    .ok();
            }),
    )
}

fn folder_menu(
    menu: PopupMenu,
    view: &WeakEntity<MainWindow>,
    path: &Path,
    role: Role,
    explicit: bool,
    auto_role: Role,
) -> PopupMenu {
    type Action = fn(&mut MainWindow, PathBuf, &mut Window, &mut Context<MainWindow>);
    let item = |label: &str, icon: &str, action: Action| {
        let (view, path) = (view.clone(), path.to_path_buf());
        PopupMenuItem::new(label.to_string())
            .icon(Icon::empty().path(icon.to_string()))
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| action(this, path.clone(), window, cx))
                    .ok();
            })
    };
    role_items(menu, view, path, role, explicit, auto_role)
        .separator()
        .item(item(
            "Auto-tag Folder",
            "icons/wand-sparkles.svg",
            |this, p, w, cx| this.auto_tag_folder(p, w, cx),
        ))
        .when(role == Role::Album, |menu| {
            menu.separator()
                .item(item(
                    "Set Art…",
                    "icons/image-plus.svg",
                    |this, p, w, cx| this.set_folder_art(p, w, cx),
                ))
                .item(item(
                    "Find Art Online",
                    "icons/search.svg",
                    |this, p, w, cx| this.find_folder_art(p, w, cx),
                ))
                .item(item(
                    "Remove Art…",
                    "icons/image-minus.svg",
                    |this, p, w, cx| this.remove_folder_art(p, w, cx),
                ))
        })
        .separator()
        .item(item(
            "New Folder Inside",
            "icons/folder-plus.svg",
            |this, p, w, cx| {
                this.new_folder(p, w, cx);
            },
        ))
        .item(item("Rename…", "icons/pencil.svg", |this, p, w, cx| {
            this.start_rename(p, w, cx)
        }))
        .item(item(
            "Show in Finder",
            "icons/folder-search.svg",
            |this, p, _, _| this.reveal_in_finder(&p),
        ))
        .separator()
        .item(item(
            "Move to Trash",
            "icons/trash.svg",
            |this, p, w, cx| this.trash_paths(vec![p], w, cx),
        ))
}

#[allow(clippy::too_many_arguments)]
fn folder_row(
    theme: &Theme,
    view: &WeakEntity<MainWindow>,
    col_ix: usize,
    i: usize,
    e: FolderEntry,
    role: Role,
    explicit: bool,
    is_sel: bool,
    renaming: Option<Entity<InputState>>,
    thumb: Option<Arc<Image>>,
) -> AnyElement {
    let auto_role = e.auto_role;
    let role_tip = if explicit {
        role.label().to_string()
    } else {
        format!("{} (automatic)", role.label())
    };
    let icon_color = if explicit || is_sel {
        theme.sidebar_primary
    } else {
        theme.muted_foreground
    };
    // Every row leads with the same 24px tile: album art, or the role icon.
    let role_button = if let Some(thumb) = thumb {
        // Folders with art show it; the role lives in the right-click menu.
        with_tip(
            ("role-tip", i),
            role_tip,
            div()
                .size(px(24.))
                .rounded(px(5.))
                .overflow_hidden()
                .border_1()
                .border_color(theme.border.opacity(0.5))
                .child(img(thumb).size_full().object_fit(ObjectFit::Cover)),
        )
    } else {
        let (view, path) = (view.clone(), e.path.clone());
        with_tip(
            ("role-tip", i),
            role_tip,
            div()
                .size(px(24.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .bg(overlay(theme, if is_sel { 0.12 } else { 0.08 }))
                .child(
                    Button::new(("role", i))
                        .ghost()
                        .xsmall()
                        .icon(
                            Icon::empty()
                                .path(role.icon())
                                .size_3p5()
                                .text_color(icon_color),
                        )
                        .dropdown_menu(move |menu, _, _| {
                            role_items(menu, &view, &path, role, explicit, auto_role)
                        }),
                ),
        )
        // Don't let a click on the icon also open the folder.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    };

    let label: AnyElement = match renaming {
        Some(input) => {
            let view = view.clone();
            div()
                .flex_1()
                .min_w_0()
                .capture_action(move |_: &input::Escape, window, cx| {
                    view.update(cx, |this, cx| this.cancel_rename(window, cx))
                        .ok();
                    cx.stop_propagation();
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Input::new(&input)
                        .id("rename-input")
                        .xsmall()
                        .w_full()
                        .focus_bordered(false),
                )
                .into_any_element()
        }
        None => div()
            .flex_1()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .child(e.name.clone())
            .into_any_element(),
    };

    let (menu_view, menu_path) = (view.clone(), e.path.clone());
    let drag = DraggedFolder(e.path.clone());
    let drag_name: SharedString = e.name.clone().into();
    let row = folder_drop_target(h_flex().id(("folder", i)), view, e.path.clone());
    let view = view.clone();
    row.on_drag(drag, move |_, _, _, cx| {
        let label = drag_name.clone();
        cx.new(|_| DragPreview {
            icon: "icons/folder.svg",
            label,
        })
    })
    .test_support()
    .h(px(32.))
    .w_full()
    .pl_1()
    .pr_2()
    .gap_2()
    .items_center()
    .rounded_md()
    .text_size(px(13.))
    .cursor_pointer()
    .when(is_sel, |d| {
        d.bg(overlay(theme, 0.12)).text_color(theme.foreground)
    })
    .when(!is_sel, |d| {
        d.text_color(theme.sidebar_foreground)
            .hover(|s| s.bg(overlay(theme, 0.06)))
    })
    .on_click(move |_, window, cx| {
        view.update(cx, |this, cx| this.select_folder(col_ix, i, window, cx))
            .ok();
    })
    .child(role_button)
    .child(label)
    .when(e.has_children, |d| {
        d.child(
            Icon::empty()
                .path("icons/chevron-right.svg")
                .size_3()
                .text_color(theme.muted_foreground.opacity(0.7)),
        )
    })
    .context_menu(move |menu, _, _| {
        folder_menu(menu, &menu_view, &menu_path, role, explicit, auto_role)
    })
    .into_any_element()
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.request_thumbs(cx);
        self.request_row_art(cx);
        self.request_header_art(cx);
        let theme = cx.theme().clone();
        v_flex()
            .id("easytag")
            .key_context(APP_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| this.prompt_open(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &UndoMove, window, cx| this.undo_move(window, cx)))
            .on_action(cx.listener(|this, _: &ClearSelection, window, cx| {
                // In a tag field, Escape just leaves the field.
                let in_field = this
                    .fields
                    .iter()
                    .any(|f| f.state.read(cx).focus_handle(cx).is_focused(window));
                if in_field {
                    window.focus(&this.grid_focus, cx);
                } else {
                    this.clear_selection(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Reload, window, cx| this.reload(window, cx)))
            .on_action(cx.listener(|this, _: &AutoTag, window, cx| this.auto_tag(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)),
            )
            .overflow_hidden()
            .child(self.render_toolbar(cx))
            .child(
                h_flex()
                    .id("main")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    // Dragging a divider sets that panel's width directly.
                    .on_drag_move(cx.listener(|this, e: &DragMoveEvent<Divider>, _, cx| {
                        let x = e.event.position.x;
                        match e.drag(cx) {
                            Divider::Browser => {
                                this.browser_width =
                                    (x - e.bounds.left()).clamp(px(MIN_BROWSER), px(1400.));
                            }
                            Divider::Inspector => {
                                this.inspector_width = (e.bounds.right() - x)
                                    .clamp(px(MIN_INSPECTOR), px(MAX_INSPECTOR));
                            }
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id("browser-pane")
                            .test_support()
                            .h_full()
                            .w(self.browser_width)
                            .min_w(px(MIN_BROWSER))
                            .flex_shrink(1.)
                            .child(clip(self.render_columns(cx))),
                    )
                    .child(divider(Divider::Browser, &theme))
                    .child(
                        div()
                            .id("songs-pane")
                            .test_support()
                            .h_full()
                            .flex_1()
                            .min_w(px(200.))
                            .child(
                                v_flex()
                                    .size_full()
                                    .relative()
                                    .children(self.render_folder_header(cx))
                                    .child(clip(self.render_grid(window, cx)))
                                    .child({
                                        // Measure the pane; re-lay the columns if it changed.
                                        let cell = self.songs_pane_width.clone();
                                        canvas(
                                            move |bounds, window, _| {
                                                let w = bounds.size.width.as_f32();
                                                if (cell.get() - w).abs() > 0.5 {
                                                    cell.set(w);
                                                    window.refresh();
                                                }
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .inset_0()
                                    }),
                            ),
                    )
                    .child(divider(Divider::Inspector, &theme))
                    .child(
                        div()
                            .id("inspector-pane")
                            .test_support()
                            .h_full()
                            .w(self.inspector_width)
                            .min_w(px(MIN_INSPECTOR))
                            .flex_shrink(1.)
                            .child(clip(self.render_tag_panel(window, cx))),
                    ),
            )
            .children(self.render_status(cx))
    }
}

#[cfg(test)]
mod ui_tests {
    use super::MainWindow;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
    };
    use std::path::PathBuf;

    fn library() -> PathBuf {
        let root = std::env::temp_dir().join(format!("easytag-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in [
            "Artist/Old Album/01 - Song.mp3",
            "Artist/Other/01 - Song.mp3",
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        }
        root
    }

    #[gpui_kit::test]
    fn renaming_a_folder_shows_an_editable_input(cx: &mut TestAppContext) {
        let root = library();
        cx.update(gpui_kit::init);
        let (window, view) = cx.update(|cx| {
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(1400.), px(800.)),
            };
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| MainWindow::new(window, cx)),
            )
            .unwrap()
        });
        cx.run_until_parked();

        // Open the library, then the Artist folder so its albums are listed.
        let album = root.join("Artist/Old Album");
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
                this.start_rename(album.clone(), window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();

        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let input = window.find("rename-input");
            println!("rename input: {input:?}");
            assert!(input.visible(), "rename input isn't visible: {input:?}");
            assert!(
                input.bounds().size.width > px(60.),
                "input too narrow: {input:?}"
            );
            assert!(
                input.bounds().size.height >= px(16.),
                "input too short: {input:?}"
            );
            assert_eq!(input.focused(), Some(true), "rename input isn't focused");
            assert_eq!(input.value(), Some("Old Album"));
        })
        .unwrap();

        // Typing replaces the selected name and shows up as it's typed.
        cx.update_window(window, |_, window, cx| {
            // The whole name starts selected, so typing replaces it.
            window.input("New Album", cx);
            window.draw(cx).clear(cx);
            let input = window.find("rename-input");
            assert_eq!(input.value(), Some("New Album"));
            assert!(input.visible() && input.bounds().size.width > px(60.));
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();

        assert!(
            root.join("Artist/New Album").is_dir(),
            "folder wasn't renamed on disk"
        );
        assert!(!album.exists());
        let _ = std::fs::remove_dir_all(root);
    }
    /// The real path: right-click the folder, choose Rename…, type, Enter.
    #[gpui_kit::test]
    fn rename_from_the_context_menu(cx: &mut TestAppContext) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::{TestEventExt, TestQueryExt};
        let root = library();
        cx.update(gpui_kit::init);
        let (window, view) = cx.update(|cx| {
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(1400.), px(800.)),
            };
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| MainWindow::new(window, cx)),
            )
            .unwrap()
        });
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();

        // "Old Album" is the first row of the second column.
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window
                .within(("col-list", 1usize))
                .right_click(("folder", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let item = window.find_by_label("Rename…");
            let at = item.bounds().center();
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
        })
        .unwrap();
        cx.run_until_parked();

        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let input = window.find("rename-input");
            assert!(input.visible(), "{input:?}");
            assert!(input.bounds().size.width > px(60.), "{input:?}");
            assert_eq!(
                input.focused(),
                Some(true),
                "menu closing stole focus: {input:?}"
            );
            window.input("Renamed", cx);
            assert_eq!(window.find("rename-input").value(), Some("Renamed"));
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("Artist/Renamed").is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    fn open(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<MainWindow>) {
        cx.update(gpui_kit::init);
        cx.update(super::bind_keys);
        cx.update(super::enable_glass);
        let out = cx.update(|cx| {
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(1400.), px(800.)),
            };
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| MainWindow::new(window, cx)),
            )
            .unwrap()
        });
        cx.run_until_parked();
        out
    }

    fn lib(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("easytag-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        }
        root
    }

    fn click_label(window: &mut gpui_kit::Window, label: &str, cx: &mut gpui_kit::App) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::{TestEventExt, TestQueryExt};
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    }

    /// Song rows are wider than the list (it scrolls sideways), so press
    /// near their left edge, where they're actually on screen.
    fn row_point(window: &mut gpui_kit::Window, ix: usize) -> gpui_kit::Point<gpui_kit::Pixels> {
        let b = window.find(("row", ix)).bounds();
        gpui_kit::point(b.origin.x + px(40.), b.origin.y + b.size.height / 2.)
    }

    fn click_row(window: &mut gpui_kit::Window, ix: usize, cx: &mut gpui_kit::App) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::TestEventExt;
        let at = row_point(window, ix);
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    }

    /// Drag from the centre of one element to the centre of another.
    fn drag(
        window: &mut gpui_kit::Window,
        a: gpui_kit::Point<gpui_kit::Pixels>,
        b: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut gpui_kit::App,
    ) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::TestEventExt;
        window.pointer_down(a, MouseButton::Left, cx);
        for i in 1..=8 {
            let t = i as f32 / 8.;
            let p = gpui_kit::point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            window.pointer_move(p, Some(MouseButton::Left), cx);
        }
        window.pointer_up(b, MouseButton::Left, cx);
    }

    /// An album with no subfolders gets an empty column; right-click it to
    /// make a folder, which opens straight into rename.
    #[gpui_kit::test]
    fn new_folder_in_an_empty_folder(cx: &mut TestAppContext) {
        let root = lib("newfolder", &["Ty$/song.mp3", "Other/x.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 1, window, cx); // "Ty$" (sorted after "Other")
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let leaf = window.find("leaf-column");
            assert!(
                leaf.visible() && leaf.bounds().size.width > px(100.),
                "{leaf:?}"
            );
            window.right_click("leaf-column", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_label(window, "New Folder", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("Ty$/untitled folder").is_dir());
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let input = window.find("rename-input");
            assert!(
                input.visible() && input.bounds().size.width > px(60.),
                "{input:?}"
            );
            assert_eq!(input.focused(), Some(true));
            window.input("Extras", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("Ty$/Extras").is_dir());
        // Ty$ now has a subfolder, so it became a real column listing it.
        cx.update_window(window, |_, _, cx| {
            let this = view.read(cx);
            let col = this.columns.last().unwrap();
            assert_eq!(col.dir, root.join("Ty$"));
            assert_eq!(col.entries.len(), 1);
            assert_eq!(col.entries[0].name, "Extras");
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Drag a song onto a folder, then ⌘Z puts it back.
    #[gpui_kit::test]
    fn drag_songs_into_a_folder_and_undo(cx: &mut TestAppContext) {
        let root = lib(
            "drag",
            &["Album/01 - a.mp3", "Album/02 - b.mp3", "Elsewhere/x.mp3"],
        );
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx); // "Album"
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let song = row_point(window, 0);
            let target = window
                .within(("col-list", 0usize))
                .find(("folder", 1usize))
                .bounds(); // "Elsewhere"
            drag(window, song, target.center(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("Elsewhere/01 - a.mp3").exists(),
            "song wasn't moved"
        );
        assert!(!root.join("Album/01 - a.mp3").exists());
        cx.update_window(window, |_, _, cx| {
            assert_eq!(
                view.read(cx).tracks.len(),
                1,
                "moved song should leave the list"
            );
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx); // focus the song list
            window.press("cmd-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("Album/01 - a.mp3").exists(),
            "undo didn't move it back"
        );
        assert!(!root.join("Elsewhere/01 - a.mp3").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// ⌘Z in a tag field undoes typing, never a file move.
    #[gpui_kit::test]
    fn cmd_z_in_a_text_field_does_not_undo_a_move(cx: &mut TestAppContext) {
        let root = lib(
            "undo-field",
            &["Album/01 - a.mp3", "Album/02 - b.mp3", "Elsewhere/x.mp3"],
        );
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let song = row_point(window, 0);
            let target = window
                .within(("col-list", 0usize))
                .find(("folder", 1usize))
                .bounds();
            drag(window, song, target.center(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("Elsewhere/01 - a.mp3").exists());
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            window.draw(cx).clear(cx);
            window.click("field-Title", cx);
            window.input("hello", cx);
            window.press("cmd-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("Elsewhere/01 - a.mp3").exists(),
            "⌘Z in a text field moved the file back"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Escape in a tag field doesn't clear the selection; in the list it does.
    #[gpui_kit::test]
    fn escape_only_clears_selection_outside_text_fields(cx: &mut TestAppContext) {
        let root = lib("escape", &["A/1.mp3", "A/2.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            window.draw(cx).clear(cx);
            window.click("field-Title", cx);
            window.press("escape", cx);
            assert_eq!(
                view.read(cx).selected.len(),
                1,
                "Escape in a field cleared the selection"
            );
            click_row(window, 0, cx);
            window.press("escape", cx);
            assert!(
                view.read(cx).selected.is_empty(),
                "Escape in the list should clear"
            );
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Right-click a song › New Folder with Selection: songs move into a new
    /// folder that opens in rename.
    #[gpui_kit::test]
    fn new_folder_with_selection_from_the_song_list(cx: &mut TestAppContext) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::TestEventExt;
        let root = lib("withsel", &["Mixed/1.mp3", "Mixed/2.mp3", "Mixed/3.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            window.press("shift-down", cx);
            window.draw(cx).clear(cx);
            let at = row_point(window, 1);
            window.pointer_down(at, MouseButton::Right, cx);
            window.pointer_up(at, MouseButton::Right, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_label(window, "New Folder with Selection", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let input = window.find("rename-input");
            assert!(
                input.visible() && input.bounds().size.width > px(60.),
                "{input:?}"
            );
            window.input("Disc 2", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("Mixed/Disc 2/1.mp3").exists() && root.join("Mixed/Disc 2/2.mp3").exists()
        );
        assert!(root.join("Mixed/3.mp3").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// One trackpad swipe: Started, a run of moves, Ended.
    fn swipe(
        window: &mut gpui_kit::Window,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        dx: f32,
        dy: f32,
        cx: &mut gpui_kit::App,
    ) {
        use gpui_kit::{InputEvent as _, ScrollDelta, ScrollEvent, TouchPhase};
        let send =
            |window: &mut gpui_kit::Window, phase, dx: f32, dy: f32, cx: &mut gpui_kit::App| {
                window.dispatch_event(
                    ScrollEvent {
                        position: at,
                        delta: ScrollDelta::Pixels(gpui_kit::point(px(dx), px(dy))),
                        touch_phase: phase,
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                window.draw(cx).clear(cx);
            };
        send(window, TouchPhase::Started, 0., 0., cx);
        for _ in 0..10 {
            send(window, TouchPhase::Moved, dx, dy, cx);
        }
        send(window, TouchPhase::Ended, 0., 0., cx);
    }

    /// A mostly-vertical swipe with a little sideways drift scrolls the song
    /// list down without sliding it sideways (and vice versa).
    #[gpui_kit::test]
    fn song_list_scroll_sticks_to_the_swipe_direction(cx: &mut TestAppContext) {
        let files: Vec<String> = (0..80)
            .map(|i| format!("Album/{i:02} - song.mp3"))
            .collect();
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let root = lib("scroll", &files);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let grid = window.find("grid-scroll").bounds();
            let at = gpui_kit::point(grid.origin.x + px(100.), grid.center().y);
            let offsets = |cx: &gpui_kit::App| {
                let this = view.read(cx);
                let v = this.grid_scroll.0.borrow().base_handle.offset().y;
                (this.grid_hscroll.offset().x, v)
            };

            // Down, drifting slightly right.
            swipe(window, at, -4., -30., cx);
            let (x, y) = offsets(cx);
            assert!(y < px(-100.), "should have scrolled down, y = {y:?}");
            assert_eq!(x, px(0.), "a vertical swipe slid the list sideways");

            // Sideways, drifting slightly up.
            swipe(window, at, -30., 4., cx);
            let (x2, y2) = offsets(cx);
            assert!(x2 < px(-100.), "should have scrolled sideways, x = {x2:?}");
            assert_eq!(y2, y, "a sideways swipe moved the list vertically");
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Right-click a folder › Move to Trash; ⌘Z brings it back.
    #[gpui_kit::test]
    fn trash_a_folder_and_undo(cx: &mut TestAppContext) {
        let root = lib("trash-folder", &["Keep/1.mp3", "Junk/2.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| this.open_root(root.clone(), window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            // "Junk" sorts first.
            window
                .within(("col-list", 0usize))
                .right_click(("folder", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_label(window, "Move to Trash", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!root.join("Junk").exists(), "folder wasn't trashed");
        cx.update_window(window, |_, _, cx| {
            let names: Vec<_> = view.read(cx).columns[0]
                .entries
                .iter()
                .map(|e| e.name.clone())
                .collect();
            assert_eq!(names, ["Keep"]);
        })
        .unwrap();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.press("cmd-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("Junk/2.mp3").exists(),
            "undo didn't restore the folder"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Selected songs: ⌘⌫ trashes them, the menu item does too, ⌘Z restores.
    #[gpui_kit::test]
    fn trash_songs_and_undo(cx: &mut TestAppContext) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::TestEventExt;
        let root = lib("trash-songs", &["A/1.mp3", "A/2.mp3", "A/3.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            window.press("cmd-backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!root.join("A/1.mp3").exists(), "⌘⌫ didn't trash the song");
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(view.read(cx).tracks.len(), 2);
            let at = row_point(window, 0);
            window.pointer_down(at, MouseButton::Right, cx);
            window.pointer_up(at, MouseButton::Right, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_label(window, "Move to Trash", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!root.join("A/2.mp3").exists(), "menu didn't trash the song");
        assert!(root.join("A/3.mp3").exists());
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            window.press("cmd-z", cx);
            window.press("cmd-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("A/1.mp3").exists() && root.join("A/2.mp3").exists(),
            "undo didn't restore"
        );
        cx.update_window(window, |_, _, cx| assert_eq!(view.read(cx).tracks.len(), 3))
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// Resizing the window only changes the song list in the middle; the side
    /// panels keep their width (no frame-to-frame shuffling).
    #[gpui_kit::test]
    fn resizing_the_window_keeps_side_panels_steady(cx: &mut TestAppContext) {
        let root = lib("resize", &["A/1.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| this.open_root(root.clone(), window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        let widths = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                window.draw(cx).clear(cx);
                (
                    window.find("browser-pane").bounds().size.width,
                    window.find("songs-pane").bounds().size.width,
                    window.find("inspector-pane").bounds().size.width,
                )
            })
            .unwrap()
        };
        let (b1, s1, i1) = widths(cx);
        for width in [1200., 1050., 1300.] {
            cx.simulate_window_resize(window, size(px(width), px(800.)));
            cx.run_until_parked();
            let first = widths(cx);
            let second = widths(cx);
            assert_eq!(first, second, "layout changed between frames at {width}px");
            assert_eq!(
                (first.0, first.2),
                (b1, i1),
                "side panels moved at {width}px"
            );
        }
        let (_, s2, _) = widths(cx);
        assert!(s2 != s1, "the song list should absorb the size change");
        let _ = std::fs::remove_dir_all(root);
    }

    /// The window surface stays see-through (for the frosted glass) after
    /// switching between light and dark, while the theme's colours stay solid.
    #[gpui_kit::test]
    fn window_surface_stays_clear_across_theme_changes(cx: &mut TestAppContext) {
        use gpui_kit::component::{Theme, ThemeMode};
        let (window, _view) = open(cx);
        let clear: gpui_kit::Background = gpui_kit::transparent_black().into();
        for mode in [ThemeMode::Dark, ThemeMode::Light, ThemeMode::Dark] {
            cx.update_window(window, |_, window, cx| {
                Theme::change(mode, Some(window), cx)
            })
            .unwrap();
            cx.run_until_parked();
            cx.update(|cx| {
                let theme = Theme::global(cx);
                assert_eq!(
                    theme.tokens.background.background, clear,
                    "surface painted after {mode:?}"
                );
                assert_eq!(theme.background.a, 1.0, "theme colours must stay solid");
            });
        }
    }

    /// Dragging a selected song drags the whole selection.
    #[gpui_kit::test]
    fn dragging_a_selection_moves_all_of_it(cx: &mut TestAppContext) {
        let root = lib("multi", &["A/1.mp3", "A/2.mp3", "A/3.mp3", "B/x.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            window.press("shift-down", cx);
            window.draw(cx).clear(cx);
            assert_eq!(view.read(cx).selected.len(), 2);
            let song = row_point(window, 1);
            let target = window
                .within(("col-list", 0usize))
                .find(("folder", 1usize))
                .bounds();
            drag(window, song, target.center(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("B/1.mp3").exists() && root.join("B/2.mp3").exists());
        assert!(root.join("A/3.mp3").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Drag a folder into another folder; never into itself.
    #[gpui_kit::test]
    fn drag_a_folder_into_another(cx: &mut TestAppContext) {
        let root = lib("folders", &["Artist/x.mp3", "Loose Album/y.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| this.open_root(root.clone(), window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let list = window.within(("col-list", 0usize));
            let (album, artist) = (
                list.find(("folder", 1usize)).bounds(),
                list.find(("folder", 0usize)).bounds(),
            );
            drag(window, album.center(), artist.center(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            root.join("Artist/Loose Album/y.mp3").exists(),
            "folder wasn't moved"
        );
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let list = window.within(("col-list", 0usize));
            let artist = list.find(("folder", 0usize)).bounds();
            drag(
                window,
                artist.center(),
                artist.center() + gpui_kit::point(px(30.), px(0.)),
                cx,
            ); // onto itself: nothing happens
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("Artist/x.mp3").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Clicking a song selects it; clicking empty space below clears it.
    #[gpui_kit::test]
    fn clicking_empty_space_clears_the_selection(cx: &mut TestAppContext) {
        use gpui_kit::MouseButton;
        use gpui_kit::test::TestEventExt;
        let root = lib("deselect", &["A/1.mp3", "A/2.mp3"]);
        let (window, view) = open(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            click_row(window, 0, cx);
            assert_eq!(view.read(cx).selected.len(), 1, "row click should select");
            let grid = window.find("grid-scroll").bounds();
            let empty = gpui_kit::point(grid.center().x, grid.bottom() - px(40.));
            window.pointer_down(empty, MouseButton::Left, cx);
            window.pointer_up(empty, MouseButton::Left, cx);
            assert!(
                view.read(cx).selected.is_empty(),
                "empty-space click should clear"
            );
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod tests {
    use super::{Track, lookup_album};

    /// A "my comps" folder isn't an artist: the lookup falls back to the
    /// artist in the songs' own tags.
    #[test]
    #[ignore]
    fn lookup_falls_back_to_tag_artist() {
        let t = Track {
            artist: "Kanye West".into(),
            ..Default::default()
        };
        for album in ["Yeezus", "Jesus Is King"] {
            let info = lookup_album("my comps", album, std::slice::from_ref(&t)).unwrap();
            assert!(info.cover.is_some(), "{album}");
        }
    }
}

/// Renders the real window to PNGs (Metal + macOS text) so the design can be
/// checked. Run: EASYTAG_SAMPLES=dir/with/tone.mp3 EASYTAG_SHOTS=out/dir \
///   cargo test screenshots -- --ignored --nocapture
#[cfg(test)]
mod screenshots {
    use super::MainWindow;
    use crate::library;
    use crate::tags::{self, CoverEdit, Edit, Field};
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{
        AppContext, Bounds, HeadlessAppContext, Point, WindowBounds, WindowOptions, px, size,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn art(r: u8, g: u8, b: u8) -> Vec<u8> {
        let img = image::RgbImage::from_fn(600, 600, |x, y| {
            let t = (x + y) as f32 / 1200.;
            image::Rgb([
                (r as f32 * (1. - t) + 30. * t) as u8,
                (g as f32 * (1. - t) + 30. * t) as u8,
                (b as f32 * (1. - t) + 40. * t) as u8,
            ])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    fn song(root: &Path, rel: &str, mp3: &[u8], fields: &[(Field, &str)], cover: Option<&[u8]>) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, mp3).unwrap();
        let edit = Edit {
            fields: fields.iter().map(|(f, v)| (*f, v.to_string())).collect(),
            cover: cover.map(|c| CoverEdit::Set(c.to_vec())),
            explicit: Some(rel.contains("Rich Flex") || rel.contains("Papi")),
        };
        tags::write_track(&p, &edit).unwrap();
    }

    fn library_fixture() -> PathBuf {
        let mp3 = std::fs::read(
            PathBuf::from(std::env::var("EASYTAG_SAMPLES").expect("EASYTAG_SAMPLES"))
                .join("tone.mp3"),
        )
        .unwrap();
        let root = std::env::temp_dir()
            .join(format!("easytag-shots-{}", std::process::id()))
            .join("Local Music");
        let _ = std::fs::remove_dir_all(&root);
        let clb = art(196, 140, 170);
        let hl = art(120, 90, 70);
        let nwts = art(70, 120, 170);
        let albums: [(&str, &str, &str, &[u8], &[&str]); 3] = [
            (
                "Certified Lover Boy",
                "2021",
                "Hip-Hop/Rap",
                &clb,
                &[
                    "Champagne Poetry",
                    "Papi's Home",
                    "Girls Want Girls",
                    "In The Bible",
                    "Love All",
                    "Fair Trade",
                    "Way 2 Sexy",
                    "TSU",
                    "N 2 Deep",
                    "Pipe Down",
                    "Yebba's Heartbreak",
                    "No Friends In The Industry",
                    "Knife Talk",
                    "7am On Bridle Path",
                    "Race My Mind",
                ],
            ),
            (
                "Her Loss",
                "2022",
                "Hip-Hop/Rap",
                &hl,
                &[
                    "Rich Flex",
                    "Major Distribution",
                    "On BS",
                    "BackOutsideBoyz",
                ],
            ),
            (
                "Nothing Was The Same",
                "2013",
                "Hip-Hop/Rap",
                &nwts,
                &[
                    "Tuscan Leather",
                    "Furthest Thing",
                    "Started From the Bottom",
                ],
            ),
        ];
        for (album, year, genre, cover, titles) in albums {
            library::save_folder_art(&root.join("Drake").join(album), cover).ok();
            for (i, title) in titles.iter().enumerate() {
                let n = (i + 1).to_string();
                song(
                    &root,
                    &format!("Drake/{album}/{:02} - {title}.mp3", i + 1),
                    &mp3,
                    &[
                        (Field::Title, title),
                        (Field::Artist, "Drake"),
                        (Field::Album, album),
                        (Field::AlbumArtist, "Drake"),
                        (Field::Track, &n),
                        (Field::Year, year),
                        (Field::Genre, genre),
                    ],
                    Some(cover),
                );
            }
        }
        for album in ["Iceman", "Scorpion", "Views", "For All The Dogs"] {
            std::fs::create_dir_all(root.join("Drake").join(album)).unwrap();
            std::fs::write(root.join("Drake").join(album).join("01 - Intro.mp3"), &mp3).unwrap();
        }
        let ye = art(40, 40, 40);
        song(
            &root,
            "my comps/Yeezus/01 - On Sight.mp3",
            &mp3,
            &[(Field::Title, "On Sight"), (Field::Artist, "Kanye West")],
            Some(&ye),
        );
        song(
            &root,
            "Pusha T - Blowbama/01 - Blowbama.mp3",
            &mp3,
            &[(Field::Title, "Blowbama")],
            None,
        );
        song(
            &root,
            "Ty$/01 - Paid.mp3",
            &mp3,
            &[(Field::Title, "Paid")],
            None,
        );
        root
    }

    /// The window is see-through, so show renders over a blurred, colourful
    /// stand-in for a desktop wallpaper (what the frosted glass blurs).
    fn over_wallpaper(shot: &image::RgbaImage) -> image::RgbaImage {
        let (w, h) = shot.dimensions();
        let small = image::RgbaImage::from_fn(96, 60, |x, y| {
            let (fx, fy) = (x as f32 / 96., y as f32 / 60.);
            let r = (60. + 160. * (1. - fx) * fy) as u8;
            let g = (50. + 90. * fx * (1. - fy)) as u8;
            let b = (90. + 150. * fx) as u8;
            image::Rgba([r, g, b, 255])
        });
        let small = image::imageops::blur(&small, 6.);
        let mut out = image::imageops::resize(&small, w, h, image::imageops::FilterType::Triangle);
        for (x, y, p) in shot.enumerate_pixels() {
            let a = p[3] as f32 / 255.;
            let bg = out.get_pixel_mut(x, y);
            for c in 0..3 {
                bg[c] = (p[c] as f32 * a + bg[c] as f32 * (1. - a)) as u8;
            }
        }
        out
    }

    #[test]
    #[ignore]
    fn screenshots() {
        let out = PathBuf::from(std::env::var("EASYTAG_SHOTS").expect("EASYTAG_SHOTS"));
        std::fs::create_dir_all(&out).unwrap();
        let root = library_fixture();
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(gpui_kit::assets::AllAssets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::bind_keys(cx);
            super::enable_glass(cx);
        });
        let (window, view) = cx.update(|cx| {
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(1440.), px(880.)),
            };
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    show: false,
                    focus: false,
                    ..gpui_kit::component::TitleBar::window_options()
                },
                cx,
                |window, cx| cx.new(|cx| MainWindow::new(window, cx)),
            )
            .unwrap()
        });
        let settle = |cx: &mut HeadlessAppContext| {
            for _ in 0..30 {
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                this.open_root(root.clone(), window, cx);
                this.select_folder(0, 0, window, cx); // Drake
                this.select_folder(1, 0, window, cx); // Certified Lover Boy
            });
        })
        .unwrap();
        settle(&mut cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                let first = this.tracks[1].path.clone();
                this.set_selection(std::collections::HashSet::from([first]), window, cx);
            });
        })
        .unwrap();
        settle(&mut cx);
        let shoot = |cx: &mut HeadlessAppContext, name: &str| {
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
            let raw = cx.capture_screenshot(window).expect("render");
            let image = over_wallpaper(&raw);
            let path = out.join(format!("{name}.png"));
            image.save(&path).unwrap();
            println!("saved {}", path.display());
        };
        for (name, mode) in [("light", ThemeMode::Light), ("dark", ThemeMode::Dark)] {
            cx.update_window(window, |_, window, cx| {
                Theme::change(mode, Some(window), cx);
                window.refresh();
            })
            .unwrap();
            settle(&mut cx);
            cx.update_window(window, |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();
            let image = over_wallpaper(&cx.capture_screenshot(window).expect("render"));
            let path = out.join(format!("{name}.png"));
            image.save(&path).unwrap();
            println!("saved {}", path.display());
        }
        // Several songs selected, with the list focused (accent selection).
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| {
                let some = this.tracks[3..7].iter().map(|t| t.path.clone()).collect();
                this.set_selection(some, window, cx);
                window.focus(&this.grid_focus, cx);
            });
        })
        .unwrap();
        settle(&mut cx);
        shoot(&mut cx, "dark-multi");
        // Nothing selected.
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| this.clear_selection(window, cx));
        })
        .unwrap();
        settle(&mut cx);
        shoot(&mut cx, "dark-empty");
        // An artist: songs from several albums, each with its album art.
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |this, cx| this.select_column_root(1, window, cx));
        })
        .unwrap();
        settle(&mut cx);
        shoot(&mut cx, "dark-artist");
    }
}
