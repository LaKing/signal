//! Several windows over the screens (ADR 0032), without a window: the key
//! that describes the connected screens, the layout of one screen
//! configuration as the project's `ui/windows.toml` holds it, where each
//! window opens (kept inside its screen's visible area), the default state
//! (one main window on the main screen holding every card), and how a
//! panel card moves from one window to another, or to a new one (its
//! context menu).
//!
//! The layout is presentation, as `ui/schematic.toml` is: it changes
//! nothing the project plays. It is project text (rule 4), under a key per
//! screen configuration, so a project carried to another desk, a missing
//! screen, another resolution or another monitor finds no entry for its
//! key and opens in the default state.

use std::time::{Duration, Instant};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

/// What a window shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowView {
    /// The app's page (toolbar, tabs, the project's panel, the schematic).
    #[default]
    Main,
    /// The schematic page alone.
    Schematic,
    /// A panel window: the cards of the project's main panel it holds.
    Panel,
}

impl WindowView {
    /// Every view.
    pub const ALL: [WindowView; 3] = [WindowView::Main, WindowView::Schematic, WindowView::Panel];

    /// Its name in `ui/windows.toml`.
    pub fn name(self) -> &'static str {
        match self {
            WindowView::Main => "main",
            WindowView::Schematic => "schematic",
            WindowView::Panel => "panel",
        }
    }

    /// The view named `name`.
    pub fn from_name(name: &str) -> Option<WindowView> {
        WindowView::ALL.into_iter().find(|v| v.name() == name)
    }
}

/// A monitor as tao reports it: its name (on macOS "Monitor #<model>", the
/// monitor's type), size and top-left corner in physical pixels, and its
/// scale factor.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// Its name.
    pub name: String,
    /// Its width, in pixels.
    pub width: u32,
    /// Its height, in pixels.
    pub height: u32,
    /// Points to pixels.
    pub scale: f64,
    /// Its left edge, in pixels (tao: the corner in points times its scale).
    pub x: i32,
    /// Its top edge, in pixels.
    pub y: i32,
}

/// A screen in points (the desktop's coordinates, from the main screen's
/// top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    /// Its left edge.
    pub x: f64,
    /// Its top edge.
    pub y: f64,
    /// Its width.
    pub width: f64,
    /// Its height.
    pub height: f64,
}

/// What [`Screen::visible`] leaves out at the top of every screen, in
/// points: the menu bar (macOS shows it on each screen; 37 points on a
/// display with a camera notch). tao reports no visible frame and the
/// Dock's place is not known, so only the menu bar is left out.
pub const MENU_BAR_INSET: f64 = 40.0;

/// A window's title bar, in points: a frame is its content and this.
pub const TITLE_BAR: f64 = 28.0;

impl Screen {
    /// Its visible area: `(x, y, width, height)` below the menu bar.
    pub fn visible(&self) -> (f64, f64, f64, f64) {
        let inset = MENU_BAR_INSET.min(self.height / 2.0);
        (self.x, self.y + inset, self.width, self.height - inset)
    }

    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// The connected screens: their key, the screens in the key's order, and
/// which is the main one (with the menu bar).
#[derive(Clone, Debug, PartialEq)]
pub struct Screens {
    /// [`screen_key`] of them.
    pub key: String,
    /// In the key's order, in points.
    pub screens: Vec<Screen>,
    /// The main screen's index in `screens`.
    pub primary: usize,
}

/// The key of a screen configuration: each monitor's name, size in pixels,
/// scale factor and top-left corner in pixels, by position (left to right,
/// then top to bottom), joined by ` | `. For example
/// `Monitor #41234 3840x2160@2 +0+0 | Monitor #16711 2560x1440@1 -2560+0`.
/// Another resolution, scale, arrangement or monitor is another key.
pub fn screen_key(monitors: &[Monitor]) -> String {
    if monitors.is_empty() {
        return "no screen".to_owned();
    }
    order(monitors)
        .iter()
        .filter_map(|&i| monitors.get(i))
        .map(|m| {
            format!(
                "{} {}x{}@{} {:+}{:+}",
                m.name.trim(),
                m.width,
                m.height,
                m.scale,
                m.x,
                m.y
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The monitors' indices in the key's order.
fn order(monitors: &[Monitor]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..monitors.len()).collect();
    order.sort_by_key(|&i| monitors.get(i).map(|m| (m.x, m.y, m.name.clone())));
    order
}

/// The screens of `monitors`, `primary` the main one's index among them.
pub fn screens_of(monitors: &[Monitor], primary: Option<usize>) -> Screens {
    let order = order(monitors);
    let screens = order
        .iter()
        .filter_map(|&i| monitors.get(i))
        .map(|m| {
            let scale = if m.scale > 0.0 { m.scale } else { 1.0 };
            Screen {
                x: f64::from(m.x) / scale,
                y: f64::from(m.y) / scale,
                width: f64::from(m.width) / scale,
                height: f64::from(m.height) / scale,
            }
        })
        .collect();
    Screens {
        key: screen_key(monitors),
        screens,
        primary: primary
            .and_then(|p| order.iter().position(|&i| i == p))
            .unwrap_or(0),
    }
}

impl Screens {
    /// The screen at `index`, else the main one; `None` without screens.
    pub fn get(&self, index: usize) -> Option<&Screen> {
        self.screens
            .get(index)
            .or_else(|| self.screens.get(self.primary))
            .or_else(|| self.screens.first())
    }

    /// The screen holding the point `at`, else the main one's index.
    pub fn at(&self, at: (f64, f64)) -> usize {
        self.screens
            .iter()
            .position(|s| s.contains(at))
            .unwrap_or(self.primary)
    }
}

/// A window of a layout.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutWindow {
    /// What it shows.
    pub view: WindowView,
    /// Its screen, an index in the key's order.
    pub screen: usize,
    /// Its frame's top-left corner from its screen's, in points.
    pub x: f64,
    /// See `x`.
    pub y: f64,
    /// Its content's width, in points.
    pub width: f64,
    /// Its content's height, in points.
    pub height: f64,
    /// A panel window's cards, in order (card ids, `ui-controls.md`
    /// "Cards"); a main window's are the order of the cards it shows first
    /// (the rest follow in the panel's order).
    pub cards: Vec<String>,
    /// Whether it is full screen (on its screen); its place and size are
    /// then what it returns to.
    pub fullscreen: bool,
}

/// One screen configuration's windows, in the order they were opened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// Its windows.
    pub windows: Vec<LayoutWindow>,
}

/// A main window's size without a Setup's `[window]`, in points.
pub const DEFAULT_SIZE: (f64, f64) = (1200.0, 800.0);

/// The default state: one main window, centred on the main screen and no
/// larger than its visible area, holding every card; `size` is the Setup's
/// `[window]`, when it has one.
pub fn default_layout(screens: &Screens, size: Option<(f64, f64)>) -> Layout {
    let (width, height) = size.unwrap_or(DEFAULT_SIZE);
    let (x, y, width, height) = match screens.get(screens.primary) {
        Some(screen) => {
            let (vx, vy, vw, vh) = screen.visible();
            let width = width.min(vw);
            let height = height.min(vh - TITLE_BAR).max(1.0);
            let x = vx + (vw - width) / 2.0 - screen.x;
            let y = vy + ((vh - height - TITLE_BAR) / 2.0).max(0.0) - screen.y;
            (x.round(), y.round(), width, height)
        }
        None => (0.0, MENU_BAR_INSET, width, height),
    };
    Layout {
        windows: vec![LayoutWindow {
            view: WindowView::Main,
            screen: screens.primary,
            x,
            y,
            width,
            height,
            cards: Vec::new(),
            fullscreen: false,
        }],
    }
}

/// Where a window opens, in the desktop's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The frame's top-left corner.
    pub x: f64,
    /// See `x`.
    pub y: f64,
    /// The content's width.
    pub width: f64,
    /// The content's height.
    pub height: f64,
}

/// Where `window` opens: on its screen (the main one when the index names
/// none), at its place there, moved (and made smaller only when larger)
/// so that its frame is inside the screen's visible area.
pub fn place(window: &LayoutWindow, screens: &Screens) -> Placement {
    let Some(screen) = screens.get(window.screen) else {
        return Placement {
            x: window.x,
            y: window.y,
            width: window.width,
            height: window.height,
        };
    };
    let (vx, vy, vw, vh) = screen.visible();
    let width = window.width.min(vw).max(1.0);
    let height = window.height.min(vh - TITLE_BAR).max(1.0);
    let x = (screen.x + window.x).clamp(vx, (vx + vw - width).max(vx));
    let y = (screen.y + window.y).clamp(vy, (vy + vh - height - TITLE_BAR).max(vy));
    Placement {
        x,
        y,
        width,
        height,
    }
}

/// A window as the layout records it: `view` with `cards`, its frame's
/// corner `at` and content `size` in the desktop's points, on the screen
/// holding its centre (else the main one); rounded to whole points.
pub fn record(
    view: WindowView,
    cards: Vec<String>,
    at: (f64, f64),
    size: (f64, f64),
    screens: &Screens,
) -> LayoutWindow {
    let centre = (at.0 + size.0 / 2.0, at.1 + size.1 / 2.0);
    let index = screens.at(centre);
    let origin = screens.get(index).map_or((0.0, 0.0), |s| (s.x, s.y));
    LayoutWindow {
        view,
        screen: index,
        x: (at.0 - origin.0).round(),
        y: (at.1 - origin.1).round(),
        width: size.0.round().max(1.0),
        height: size.1.round().max(1.0),
        cards,
        fullscreen: false,
    }
}

/// Whether layouts `a` and `b` are the same but for a few points of place
/// or size (a window the system placed a little off what was asked): the
/// same windows, views, screens and cards.
pub fn similar(a: &Layout, b: &Layout) -> bool {
    let near = |x: f64, y: f64| (x - y).abs() <= 2.0;
    a.windows.len() == b.windows.len()
        && a.windows.iter().zip(&b.windows).all(|(a, b)| {
            a.view == b.view
                && a.fullscreen == b.fullscreen
                && a.screen == b.screen
                && a.cards == b.cards
                && near(a.x, b.x)
                && near(a.y, b.y)
                && near(a.width, b.width)
                && near(a.height, b.height)
        })
}

/// How many polls in a row must see another screen configuration before
/// it is taken (displays reconfiguring, waking, or reporting a scale of 1
/// for a moment list otherwise).
pub const KEY_POLLS: u32 = 3;

/// After a layout is applied, or the screens were unsettled, the windows'
/// moves and resizes for this long are the system's (placing,
/// constraining, migrating windows), not the person's.
pub const GUARD: Duration = Duration::from_secs(2);

/// How long the layout stays unchanged before it is written.
pub const SETTLE: Duration = Duration::from_secs(1);

/// What changed a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// It moved or was resized (also by the system).
    Geometry,
    /// A window opened or closed, or a card moved: always the person's.
    Structure,
}

/// What a poll of the screens leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The same configuration.
    Same,
    /// Another one (or none listed), not yet taken: nothing is applied and
    /// nothing is written meanwhile.
    Waiting,
    /// Another configuration, seen [`KEY_POLLS`] times in a row: its layout
    /// applies.
    Switch,
}

/// When the layout is written (ADR 0032), without a window: the screen
/// configuration's key, taken only once it is stable; the person's changes
/// since the last write; and the guard that tells the system's own moves
/// from the person's. A change while the screens are unsettled is never
/// written, under either key, so a screen unplugged, asleep or
/// reconfiguring never overwrites the layout it had.
#[derive(Clone, Debug)]
pub struct Tracker {
    key: String,
    candidate: Option<(String, u32)>,
    changed: Option<Instant>,
    guard_until: Instant,
    /// The last poll that saw the stable configuration.
    confirmed: Option<Instant>,
}

impl Tracker {
    /// On screen configuration `key`, a layout just applied at `now`.
    pub fn new(key: &str, now: Instant) -> Tracker {
        Tracker {
            key: key.to_owned(),
            candidate: None,
            changed: None,
            guard_until: now + GUARD,
            confirmed: Some(now),
        }
    }

    /// The configuration taken.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// A layout was applied at `now`: what it was is what is written, and
    /// the system's moves of its windows follow.
    pub fn applied(&mut self, now: Instant) {
        self.changed = None;
        self.guard_until = now + GUARD;
    }

    /// A window changed at `now`; whether it counts as the person's: a
    /// move or resize under the guard does not, and nothing does while the
    /// screens are unsettled.
    pub fn change(&mut self, now: Instant, change: Change) -> bool {
        if self.candidate.is_some() || (change == Change::Geometry && now < self.guard_until) {
            return false;
        }
        self.changed = Some(now);
        true
    }

    /// A poll at `now` listed the screens of `key` (`None`: no screen
    /// listed, as while every display sleeps).
    pub fn screens(&mut self, now: Instant, key: Option<&str>) -> Seen {
        let Some(key) = key else {
            self.unsettled(now);
            return Seen::Waiting;
        };
        if key == self.key {
            if self.candidate.take().is_some() {
                // Back as it was: the windows may move back too.
                self.guard_until = now + GUARD;
            }
            self.confirmed = Some(now);
            return Seen::Same;
        }
        self.unsettled(now);
        let count = match &self.candidate {
            Some((seen, n)) if seen == key => n + 1,
            _ => 1,
        };
        if count >= KEY_POLLS {
            self.key = key.to_owned();
            self.candidate = None;
            self.confirmed = Some(now);
            Seen::Switch
        } else {
            self.candidate = Some((key.to_owned(), count));
            Seen::Waiting
        }
    }

    /// The screens are not as taken: what changed since is dropped, and
    /// the windows' moves for a while are the system's.
    fn unsettled(&mut self, now: Instant) {
        self.changed = None;
        self.confirmed = None;
        self.guard_until = now + GUARD;
    }

    /// Whether to write now: a change of the person's is pending, the
    /// screens are settled, and it has settled (a poll after it saw the
    /// same screens, and [`SETTLE`] passed); `force` (Quit, opening another
    /// project) does not wait for it to settle, but still writes nothing
    /// of the system's.
    pub fn due(&self, now: Instant, force: bool) -> bool {
        let Some(at) = self.changed else {
            return false;
        };
        if self.candidate.is_some() {
            return false;
        }
        force
            || (self.confirmed.is_some_and(|c| c >= at)
                && now.saturating_duration_since(at) >= SETTLE)
    }

    /// When the change pending was made.
    pub fn pending(&self) -> Option<Instant> {
        self.changed
    }

    /// The layout as it was at `asked` was written: a later change stays
    /// pending.
    pub fn written(&mut self, asked: Instant) {
        if self.changed.is_some_and(|at| at <= asked) {
            self.changed = None;
        }
    }

    /// The write asked at `asked` failed at `now`: tried again once
    /// settled anew (unless a later change is pending already).
    pub fn failed(&mut self, asked: Instant, now: Instant) {
        if self.changed.is_some_and(|at| at <= asked) {
            self.changed = Some(now);
        }
    }
}

/// The smallest window opened for cards, in points.
pub const MIN_SIZE: (f64, f64) = (320.0, 160.0);

/// A window of `view` and content `size` with its frame's top-left corner
/// at `at` (in the desktop's points), on the screen holding
/// that point, moved (and made smaller only when larger) to lie wholly in
/// its visible area.
pub fn at_point(
    view: WindowView,
    at: (f64, f64),
    size: (f64, f64),
    screens: &Screens,
) -> LayoutWindow {
    let index = screens.at(at);
    let width = size.0.max(MIN_SIZE.0);
    let height = size.1.max(MIN_SIZE.1);
    let Some(screen) = screens.get(index) else {
        return LayoutWindow {
            view,
            screen: 0,
            x: at.0,
            y: at.1,
            width,
            height,
            cards: Vec::new(),
            fullscreen: false,
        };
    };
    let wanted = LayoutWindow {
        view,
        screen: index,
        x: at.0 - screen.x,
        y: at.1 - screen.y,
        width,
        height,
        cards: Vec::new(),
        fullscreen: false,
    };
    let placed = place(&wanted, screens);
    LayoutWindow {
        x: (placed.x - screen.x).round(),
        y: (placed.y - screen.y).round(),
        width: placed.width.round(),
        height: placed.height.round(),
        ..wanted
    }
}

/// A window of `view` and content `size`, centred in the visible area of
/// screen `screen` (the main one when it names none).
pub fn centred(
    view: WindowView,
    size: (f64, f64),
    screen: usize,
    screens: &Screens,
) -> LayoutWindow {
    let index = if screen < screens.screens.len() {
        screen
    } else {
        screens.primary
    };
    let Some(s) = screens.get(index) else {
        return at_point(view, (0.0, MENU_BAR_INSET), size, screens);
    };
    let (vx, vy, vw, vh) = s.visible();
    let width = size.0.min(vw);
    let height = size.1.min(vh - TITLE_BAR);
    let corner = (
        vx + (vw - width) / 2.0,
        vy + ((vh - height - TITLE_BAR) / 2.0).max(0.0),
    );
    at_point(view, corner, (width, height), screens)
}

/// The content height a window (laid out as `window`) takes to fit
/// content `height` points high: at least [`MIN_SIZE`]'s, and no lower than
/// its screen's visible area.
pub fn fit_height(window: &LayoutWindow, height: f64, screens: &Screens) -> f64 {
    let room = screens.get(window.screen).map_or(height, |s| {
        let (_, vy, _, vh) = s.visible();
        vy + vh - (s.y + window.y) - TITLE_BAR
    });
    height.max(MIN_SIZE.1).min(room.max(MIN_SIZE.1)).round()
}

/// Each window's name, as the cards' menus and placeholders say it: "Main
/// window" ("Main window 2" when there are several), "Panel window 1
/// (screen 2)", "Schematic window".
pub fn labels(layout: &Layout) -> Vec<String> {
    let mains = layout
        .windows
        .iter()
        .filter(|w| w.view == WindowView::Main)
        .count();
    let (mut main, mut schematic, mut panel) = (0usize, 0usize, 0usize);
    layout
        .windows
        .iter()
        .map(|w| {
            let count = match w.view {
                WindowView::Main => &mut main,
                WindowView::Schematic => &mut schematic,
                WindowView::Panel => &mut panel,
            };
            *count += 1;
            let k = *count;
            match w.view {
                WindowView::Main if mains > 1 => format!("Main window {k}"),
                WindowView::Main => "Main window".to_owned(),
                WindowView::Schematic if k > 1 => format!("Schematic window {k}"),
                WindowView::Schematic => "Schematic window".to_owned(),
                WindowView::Panel => format!("Panel window {k} (screen {})", w.screen + 1),
            }
        })
        .collect()
}

/// The panel window holding `card`, if one does.
pub fn holder(layout: &Layout, card: &str) -> Option<usize> {
    layout
        .windows
        .iter()
        .position(|w| w.view == WindowView::Panel && w.cards.iter().any(|c| c == card))
}

/// Where a card is moved (its menu, a placeholder's Bring back).
#[derive(Clone, Debug, PartialEq)]
pub enum CardMove {
    /// Into window `0` (an index of the layout), at its end.
    Into(usize),
    /// Into a new panel window laid out so.
    New(LayoutWindow),
}

/// The cards panel windows hold: the main windows' panels do not show them.
pub fn held(layout: &Layout) -> Vec<String> {
    layout
        .windows
        .iter()
        .filter(|w| w.view == WindowView::Panel)
        .flat_map(|w| w.cards.iter().cloned())
        .collect()
}

/// Moves `card` to `to`: out of every window that had it, into a panel
/// window (which then holds it) or a main window (which then shows it,
/// placed last). Whether anything changed; a move into a schematic window
/// or a window the layout does not have changes nothing.
pub fn move_card(layout: &mut Layout, card: &str, to: CardMove) -> bool {
    if !is_card_id(card) {
        return false;
    }
    if let CardMove::Into(window) = &to
        && !layout
            .windows
            .get(*window)
            .is_some_and(|w| w.view != WindowView::Schematic)
    {
        return false;
    }
    let before = layout.clone();
    for window in &mut layout.windows {
        window.cards.retain(|c| c != card);
    }
    match to {
        CardMove::Into(window) => {
            if let Some(target) = layout.windows.get_mut(window) {
                target.cards.push(card.to_owned());
            }
        }
        CardMove::New(mut window) => {
            window.view = WindowView::Panel;
            window.cards = vec![card.to_owned()];
            layout.windows.push(window);
        }
    }
    *layout != before
}

/// Window `window`'s cards in the order `order` (Move up, Move down): a
/// panel window keeps the cards it holds, those `order` names first; a
/// main window's order becomes `order`, less the cards panel windows hold.
/// Whether anything changed.
pub fn reorder(layout: &mut Layout, window: usize, order: &[String]) -> bool {
    let held = held(layout);
    let Some(target) = layout.windows.get_mut(window) else {
        return false;
    };
    let mut cards: Vec<String> = match target.view {
        WindowView::Schematic => return false,
        WindowView::Panel => order
            .iter()
            .filter(|c| target.cards.contains(c))
            .cloned()
            .collect(),
        WindowView::Main => order
            .iter()
            .filter(|c| is_card_id(c) && !held.contains(c))
            .cloned()
            .collect(),
    };
    cards.dedup();
    if target.view == WindowView::Panel {
        for card in &target.cards {
            if !cards.contains(card) {
                cards.push(card.clone());
            }
        }
    }
    if cards == target.cards {
        return false;
    }
    target.cards = cards;
    true
}

/// Whether `id` may be a card's id: the ids `ui/app/lib/cards.js` gives
/// (an `id` attribute, or a heading's slug), letters, digits, `-`, `_`,
/// `.` and `:`, at most 128 bytes.
pub fn is_card_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// The layout of screen configuration `key` in `ui/windows.toml`'s `text`:
/// `Ok(None)` when it has none (the default state), `Err` with the reason
/// when the file or the entry does not read (also the default state, and
/// the reason is told).
pub fn read_layout(text: &str, key: &str) -> Result<Option<Layout>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("not valid TOML ({})", e.to_string().trim()))?;
    let Some(entry) = doc
        .get("layout")
        .and_then(Item::as_table_like)
        .and_then(|t| t.get(key))
    else {
        return Ok(None);
    };
    let windows = entry
        .as_table_like()
        .and_then(|t| t.get("windows"))
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| format!("layout \"{key}\": needs [[layout.\"…\".windows]]"))?;
    let mut layout = Layout::default();
    for (n, table) in windows.iter().enumerate() {
        let bad = |reason: &str| format!("layout \"{key}\": window {}: {reason}", n + 1);
        for (name, _) in table.iter() {
            if !matches!(
                name,
                "view" | "screen" | "x" | "y" | "width" | "height" | "cards" | "fullscreen"
            ) {
                return Err(bad(&format!("unknown key '{name}'")));
            }
        }
        let number = |name: &str| {
            let item = table.get(name);
            item.and_then(Item::as_float)
                .or_else(|| item.and_then(Item::as_integer).map(|i| i as f64))
                .filter(|v| v.is_finite() && v.abs() <= 1e7)
                .ok_or_else(|| bad(&format!("`{name}` must be a number")))
        };
        let view = table
            .get("view")
            .and_then(Item::as_str)
            .and_then(WindowView::from_name)
            .ok_or_else(|| bad("`view` must be \"main\", \"schematic\" or \"panel\""))?;
        let screen = table
            .get("screen")
            .and_then(Item::as_integer)
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| bad("`screen` must be a screen's index"))?;
        let (width, height) = (number("width")?, number("height")?);
        if width <= 0.0 || height <= 0.0 {
            return Err(bad("`width` and `height` must be positive"));
        }
        let cards = match table.get("cards") {
            None => Vec::new(),
            Some(item) => item
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .map(|c| c.as_str().filter(|c| is_card_id(c)).map(str::to_owned))
                        .collect::<Option<Vec<String>>>()
                })
                .ok_or_else(|| bad("`cards` must be a list of card ids"))?,
        };
        let fullscreen = match table.get("fullscreen") {
            None => false,
            Some(item) => item
                .as_bool()
                .ok_or_else(|| bad("`fullscreen` must be true or false"))?,
        };
        layout.windows.push(LayoutWindow {
            view,
            screen,
            x: number("x")?,
            y: number("y")?,
            width,
            height,
            cards,
            fullscreen,
        });
    }
    Ok(Some(layout))
}

/// `ui/windows.toml`'s `text` with screen configuration `key`'s layout set
/// to `layout` (removed with `None`, as Reset Layout does); every other
/// key's entry, and the file's comments, are kept as they were.
pub fn write_layout(text: &str, key: &str, layout: Option<&Layout>) -> Result<String, String> {
    if layout.is_none() && read_layout(text, key).is_ok_and(|l| l.is_none()) {
        return Ok(text.to_owned());
    }
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("not valid TOML ({})", e.to_string().trim()))?;
    if text.trim().is_empty() {
        doc.decor_mut().set_prefix(
            "# Signal.app's windows on each screen configuration (ADR 0032): presentation\n\
             # only. Written by the app; Window > Reset Layout removes the current entry.\n",
        );
    }
    if !doc.contains_key("layout") {
        let mut table = Table::new();
        table.set_implicit(true);
        doc.insert("layout", Item::Table(table));
    }
    let layouts = doc
        .get_mut("layout")
        .and_then(Item::as_table_mut)
        .ok_or("`layout` is not a table")?;
    match layout {
        None => {
            layouts.remove(key);
        }
        Some(layout) => {
            let mut windows = ArrayOfTables::new();
            for window in &layout.windows {
                let mut table = Table::new();
                table.insert("view", value(window.view.name()));
                table.insert("screen", value(i64::try_from(window.screen).unwrap_or(0)));
                for (name, number) in [
                    ("x", window.x),
                    ("y", window.y),
                    ("width", window.width),
                    ("height", window.height),
                ] {
                    table.insert(name, points(number));
                }
                if window.view != WindowView::Schematic {
                    let cards: toml_edit::Array = window.cards.iter().map(String::as_str).collect();
                    table.insert("cards", value(cards));
                }
                if window.fullscreen {
                    table.insert("fullscreen", value(true));
                }
                windows.push(table);
            }
            let mut entry = Table::new();
            entry.set_implicit(true);
            entry.insert("windows", Item::ArrayOfTables(windows));
            layouts.insert(key, Item::Table(entry));
        }
    }
    Ok(doc.to_string())
}

/// A number of points: whole ones written as integers.
fn points(number: f64) -> Item {
    if number.fract() == 0.0 && number.abs() <= 1e7 {
        value(number as i64)
    } else {
        value(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, size: (u32, u32), scale: f64, at: (i32, i32)) -> Monitor {
        Monitor {
            name: name.to_owned(),
            width: size.0,
            height: size.1,
            scale,
            x: at.0,
            y: at.1,
        }
    }

    fn desk() -> Vec<Monitor> {
        vec![
            monitor("Monitor #41234", (3024, 1964), 2.0, (0, 0)),
            monitor("Monitor #16711", (2560, 1440), 1.0, (-2560, -200)),
        ]
    }

    /// ADR 0032: the key names each screen's type, resolution, scale and
    /// place, by position; any change is another key.
    #[test]
    fn the_key_describes_the_screens() {
        let key = screen_key(&desk());
        assert_eq!(
            key,
            "Monitor #16711 2560x1440@1 -2560-200 | Monitor #41234 3024x1964@2 +0+0"
        );
        let mut reversed = desk();
        reversed.reverse();
        assert_eq!(screen_key(&reversed), key, "the order tao lists them in");
        let mut other = desk();
        other[1].width = 1920;
        assert_ne!(screen_key(&other), key, "another resolution");
        other = desk();
        other[1].name = "Monitor #9".to_owned();
        assert_ne!(screen_key(&other), key, "another monitor");
        other = desk();
        other[1].x = 3024;
        assert_ne!(screen_key(&other), key, "another arrangement");
        assert_ne!(screen_key(&desk()[..1]), key, "a screen missing");
        other = desk();
        other[0].scale = 1.5;
        assert_ne!(screen_key(&other), key, "another scale");
        assert_eq!(screen_key(&[]), "no screen");

        let screens = screens_of(&desk(), Some(0));
        assert_eq!(screens.primary, 1, "the built-in, second by position");
        assert_eq!(
            screens.screens,
            [
                Screen {
                    x: -2560.0,
                    y: -200.0,
                    width: 2560.0,
                    height: 1440.0
                },
                Screen {
                    x: 0.0,
                    y: 0.0,
                    width: 1512.0,
                    height: 982.0
                }
            ]
        );
    }

    /// ADR 0032: the default is one main window on the main screen, centred
    /// in its visible area and no larger, holding every card.
    #[test]
    fn the_default_is_one_main_window_on_the_main_screen() {
        let screens = screens_of(&desk(), Some(0));
        let layout = default_layout(&screens, Some((900.0, 640.0)));
        assert_eq!(layout.windows.len(), 1);
        let main = &layout.windows[0];
        assert_eq!(main.view, WindowView::Main);
        assert_eq!(main.screen, 1);
        assert!(main.cards.is_empty());
        assert_eq!((main.width, main.height), (900.0, 640.0));
        assert_eq!(main.x, 306.0);
        let big = default_layout(&screens, Some((4000.0, 3000.0)));
        let placed = place(&big.windows[0], &screens);
        assert_eq!(placed.width, 1512.0);
        assert_eq!(placed.height, 982.0 - MENU_BAR_INSET - TITLE_BAR);
        assert_eq!((placed.x, placed.y), (0.0, MENU_BAR_INSET));
        let none = default_layout(&screens_of(&[], None), None);
        assert_eq!(
            (none.windows[0].width, none.windows[0].height),
            DEFAULT_SIZE
        );
    }

    fn window(view: WindowView, screen: usize, at: (f64, f64), size: (f64, f64)) -> LayoutWindow {
        LayoutWindow {
            view,
            screen,
            x: at.0,
            y: at.1,
            width: size.0,
            height: size.1,
            cards: Vec::new(),
            fullscreen: false,
        }
    }

    /// ADR 0032: a window opens where it was on its screen; one partly off
    /// it, or larger than it, is kept inside its visible area; a screen
    /// index the configuration does not have is the main screen.
    #[test]
    fn a_window_is_placed_inside_its_screen() {
        let screens = screens_of(&desk(), Some(0));
        let left = window(WindowView::Panel, 0, (100.0, 300.0), (800.0, 600.0));
        assert_eq!(
            place(&left, &screens),
            Placement {
                x: -2460.0,
                y: 100.0,
                width: 800.0,
                height: 600.0
            }
        );
        let off = window(WindowView::Panel, 1, (1400.0, -30.0), (800.0, 600.0));
        assert_eq!(
            place(&off, &screens),
            Placement {
                x: 712.0,
                y: MENU_BAR_INSET,
                width: 800.0,
                height: 600.0
            }
        );
        let lost = window(WindowView::Schematic, 7, (10.0, 50.0), (400.0, 300.0));
        assert_eq!(
            place(&lost, &screens),
            Placement {
                x: 10.0,
                y: 50.0,
                width: 400.0,
                height: 300.0
            },
            "on the main screen"
        );
        // Recorded on the screen holding its centre, from that screen's corner.
        let again = record(
            WindowView::Panel,
            vec!["kick".to_owned()],
            (-2460.0, 100.4),
            (800.0, 600.0),
            &screens,
        );
        assert_eq!(
            again,
            LayoutWindow {
                cards: vec!["kick".to_owned()],
                ..left
            }
        );
        let main = record(
            WindowView::Main,
            Vec::new(),
            (10.0, 50.0),
            (400.0, 300.0),
            &screens,
        );
        assert_eq!(main.screen, 1);
        let nowhere = record(
            WindowView::Main,
            Vec::new(),
            (9000.0, 50.0),
            (400.0, 300.0),
            &screens,
        );
        assert_eq!(nowhere.screen, 1, "off every screen: the main one");
    }

    fn cards(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| (*c).to_owned()).collect()
    }

    /// ADR 0032: a card moves between windows: a panel window holds it, the
    /// main window shows the rest, a new panel window holds the card moved
    /// into it, a schematic takes none; Move up and down reorder.
    #[test]
    fn cards_move_between_windows() {
        let screens = screens_of(&desk(), Some(0));
        let mut layout = default_layout(&screens, None);
        layout
            .windows
            .push(window(WindowView::Schematic, 0, (0.0, 0.0), (900.0, 700.0)));
        let fresh = window(WindowView::Panel, 0, (1000.0, 100.0), (400.0, 300.0));
        assert!(move_card(&mut layout, "kick", CardMove::New(fresh)));
        assert_eq!(layout.windows.len(), 3);
        assert_eq!(layout.windows[2].view, WindowView::Panel);
        assert_eq!(held(&layout), ["kick"]);
        assert!(move_card(&mut layout, "output", CardMove::Into(2)));
        assert!(move_card(&mut layout, "scope", CardMove::Into(2)));
        assert_eq!(layout.windows[2].cards, cards(&["kick", "output", "scope"]));
        assert!(
            !move_card(&mut layout, "scope", CardMove::Into(2)),
            "already there"
        );
        // Move up and down: the window's order.
        assert!(reorder(
            &mut layout,
            2,
            &cards(&["output", "kick", "scope"])
        ));
        assert_eq!(layout.windows[2].cards, cards(&["output", "kick", "scope"]));
        assert!(!reorder(
            &mut layout,
            2,
            &cards(&["output", "kick", "scope"])
        ));
        assert!(
            reorder(&mut layout, 2, &cards(&["scope", "intro"])),
            "only its own"
        );
        assert_eq!(layout.windows[2].cards, cards(&["scope", "output", "kick"]));
        // Back to the main window: shown there, last; its order.
        assert!(move_card(&mut layout, "kick", CardMove::Into(0)));
        assert_eq!(held(&layout), ["scope", "output"]);
        assert_eq!(layout.windows[0].cards, ["kick"]);
        assert!(reorder(&mut layout, 0, &cards(&["intro", "kick", "scope"])));
        assert_eq!(
            layout.windows[0].cards,
            cards(&["intro", "kick"]),
            "not the held"
        );
        let unchanged = layout.clone();
        assert!(
            !move_card(&mut layout, "scope", CardMove::Into(1)),
            "a schematic"
        );
        assert!(!move_card(&mut layout, "scope", CardMove::Into(9)));
        assert!(!move_card(&mut layout, "a b", CardMove::Into(2)));
        assert!(!reorder(&mut layout, 1, &cards(&["scope"])));
        assert!(!reorder(&mut layout, 9, &cards(&["scope"])));
        assert_eq!(layout, unchanged);
    }

    fn seconds(start: Instant, s: f64) -> Instant {
        start + Duration::from_secs_f64(s)
    }

    /// Review 1: a screen unplugged: macOS migrates the windows (moves),
    /// the next polls see another key; nothing is written, under the old
    /// key or the new, and the new one applies after KEY_POLLS polls.
    #[test]
    fn an_unplugged_screen_never_overwrites_its_layout() {
        let t = Instant::now();
        let mut tracker = Tracker::new("A | B", t);
        // The person moves a window, settled and written.
        assert!(tracker.change(seconds(t, 5.0), Change::Geometry));
        assert_eq!(tracker.screens(seconds(t, 5.5), Some("A | B")), Seen::Same);
        assert!(!tracker.due(seconds(t, 5.6), false), "not settled yet");
        assert_eq!(tracker.screens(seconds(t, 6.5), Some("A | B")), Seen::Same);
        assert!(tracker.due(seconds(t, 6.5), false));
        tracker.written(seconds(t, 6.5));
        assert!(!tracker.due(seconds(t, 6.6), true));
        // Unplugged at 10: the system's migration moves, then the polls.
        assert!(
            tracker.change(seconds(t, 10.05), Change::Geometry),
            "not known yet"
        );
        assert_eq!(tracker.screens(seconds(t, 10.5), Some("A")), Seen::Waiting);
        assert!(
            !tracker.due(seconds(t, 12.0), true),
            "the migration is dropped"
        );
        assert!(!tracker.change(seconds(t, 10.6), Change::Geometry));
        assert!(
            !tracker.change(seconds(t, 10.7), Change::Structure),
            "unsettled"
        );
        assert_eq!(tracker.screens(seconds(t, 11.5), Some("A")), Seen::Waiting);
        assert_eq!(tracker.screens(seconds(t, 12.5), Some("A")), Seen::Switch);
        assert_eq!(tracker.key(), "A");
        tracker.applied(seconds(t, 12.5));
        assert!(!tracker.due(seconds(t, 20.0), true), "nothing to write");
        // An unsettled poll before the settle: the change is dropped too.
        assert!(tracker.change(seconds(t, 30.0), Change::Structure));
        assert_eq!(
            tracker.screens(seconds(t, 30.5), Some("A | C")),
            Seen::Waiting
        );
        assert!(!tracker.due(seconds(t, 32.0), true));
    }

    /// Review 2: a key that comes and goes within the debounce does
    /// nothing; no screen listed (asleep) waits; a new key must be the same
    /// for KEY_POLLS polls in a row.
    #[test]
    fn the_key_is_taken_only_once_stable() {
        let t = Instant::now();
        let mut tracker = Tracker::new("A@2 | B@1", t);
        // A scale of 1 for a moment, then back.
        assert_eq!(
            tracker.screens(seconds(t, 1.0), Some("A@1 | B@1")),
            Seen::Waiting
        );
        assert_eq!(
            tracker.screens(seconds(t, 2.0), Some("A@1 | B@1")),
            Seen::Waiting
        );
        assert_eq!(
            tracker.screens(seconds(t, 3.0), Some("A@2 | B@1")),
            Seen::Same
        );
        assert_eq!(tracker.key(), "A@2 | B@1");
        // Two candidates alternating never reach KEY_POLLS.
        for (i, key) in ["A", "B", "A", "B", "A"].into_iter().enumerate() {
            assert_eq!(
                tracker.screens(seconds(t, 4.0 + i as f64), Some(key)),
                Seen::Waiting
            );
        }
        assert_eq!(
            tracker.screens(seconds(t, 10.0), Some("A@2 | B@1")),
            Seen::Same
        );
        // Every display asleep: no screen listed, for as long as it lasts.
        for i in 0..100 {
            assert_eq!(
                tracker.screens(seconds(t, 11.0 + f64::from(i)), None),
                Seen::Waiting
            );
        }
        assert_eq!(tracker.key(), "A@2 | B@1");
        // Awake: the same key, nothing applied; the windows moving back
        // are the system's.
        assert_eq!(
            tracker.screens(seconds(t, 111.0), Some("A@2 | B@1")),
            Seen::Same
        );
        assert!(!tracker.change(seconds(t, 111.5), Change::Geometry));
        assert!(!tracker.due(seconds(t, 120.0), true));
        // A new key, KEY_POLLS times in a row (empty polls between reset nothing).
        assert_eq!(tracker.screens(seconds(t, 200.0), Some("A")), Seen::Waiting);
        assert_eq!(tracker.screens(seconds(t, 201.0), Some("A")), Seen::Waiting);
        assert_eq!(tracker.screens(seconds(t, 202.0), Some("A")), Seen::Switch);
    }

    /// Review 3, 4: the guard is per event: the system's moves just after
    /// an apply are never written, even by Quit (force); the person's
    /// later moves, and a structure change at any time, are.
    #[test]
    fn the_guard_ignores_only_the_system_s_moves() {
        let t = Instant::now();
        let mut tracker = Tracker::new("A", t);
        assert!(!tracker.change(seconds(t, 0.1), Change::Geometry));
        assert!(
            !tracker.due(seconds(t, 0.2), true),
            "Quit at once writes nothing"
        );
        assert!(
            tracker.change(seconds(t, 0.5), Change::Structure),
            "a window opened"
        );
        assert!(tracker.due(seconds(t, 0.6), true), "Quit writes it");
        tracker.written(seconds(t, 0.6));
        assert!(tracker.change(seconds(t, GUARD.as_secs_f64() + 0.1), Change::Geometry));
        assert_eq!(tracker.screens(seconds(t, 3.0), Some("A")), Seen::Same);
        assert!(tracker.due(seconds(t, 3.2), false));
        // A change made while the write was under way stays pending; a
        // failed write is tried again once settled anew.
        let asked = seconds(t, 3.2);
        assert!(tracker.change(seconds(t, 3.3), Change::Geometry));
        tracker.written(asked);
        assert_eq!(tracker.pending(), Some(seconds(t, 3.3)));
        tracker.failed(seconds(t, 3.4), seconds(t, 5.0));
        assert_eq!(tracker.pending(), Some(seconds(t, 5.0)));
        assert_eq!(tracker.screens(seconds(t, 5.5), Some("A")), Seen::Same);
        assert!(!tracker.due(seconds(t, 5.6), false));
        assert_eq!(tracker.screens(seconds(t, 6.5), Some("A")), Seen::Same);
        assert!(tracker.due(seconds(t, 6.5), false));
    }

    /// The owner's three screens (2026-10-22): 3840x2160 at scale 1 side by
    /// side, x up to 11520.
    fn three() -> Screens {
        let m = |name: &str, x| monitor(name, (3840, 2160), 1.0, (x, 0));
        screens_of(
            &[
                m("Monitor #41253", 3840),
                m("Monitor #41252", 7680),
                m("Monitor #41253", 0),
            ],
            Some(2),
        )
    }

    /// Review 6: placement on three same-scale screens at large
    /// coordinates; the y axis is tao's (top left of the main screen, down),
    /// so the window at y = 1740 of the owner's file is near the bottom of
    /// screen 1, not flipped.
    #[test]
    fn three_screens_side_by_side() {
        let screens = three();
        assert_eq!(
            screens.key,
            "Monitor #41253 3840x2160@1 +0+0 | Monitor #41253 3840x2160@1 +3840+0 | Monitor #41252 3840x2160@1 +7680+0"
        );
        assert_eq!(screens.primary, 0);
        assert_eq!(screens.at((11519.0, 2159.0)), 2);
        assert_eq!(screens.at((7680.0, 0.0)), 2);
        assert_eq!(screens.at((3839.9, 100.0)), 0);
        assert_eq!(
            screens.at((20000.0, 100.0)),
            0,
            "off every screen: the main one"
        );
        let owner = window(WindowView::Panel, 0, (705.0, 1740.0), (480.0, 360.0));
        let placed = place(&owner, &screens);
        assert_eq!((placed.x, placed.y), (705.0, 1740.0));
        assert!(
            placed.y + placed.height + TITLE_BAR <= 2160.0,
            "inside, near the bottom"
        );
        // A window cornered near screen 3's bottom-right lies wholly on it.
        let cornered = at_point(
            WindowView::Panel,
            (11400.0, 2100.0),
            (1200.0, 700.0),
            &screens,
        );
        assert_eq!(cornered.screen, 2);
        let p = place(&cornered, &screens);
        assert!(p.x >= 7680.0 && p.x + p.width <= 11520.0, "{p:?}");
        assert!(
            p.y >= MENU_BAR_INSET && p.y + p.height + TITLE_BAR <= 2160.0,
            "{p:?}"
        );
        assert_eq!((p.width, p.height), (1200.0, 700.0), "the card's size");
        assert_eq!(
            (cornered.x, cornered.y),
            (3840.0 - 1200.0, 2160.0 - 700.0 - TITLE_BAR)
        );
    }

    /// Owner, 2026-10-22: a card's new window takes its size (at least
    /// MIN_SIZE), New Panel Window opens centred on the front window's
    /// screen, and a window fits its content's height within its screen.
    #[test]
    fn panel_windows_fit_their_cards() {
        let screens = three();
        let small = at_point(WindowView::Panel, (100.0, 100.0), (50.0, 20.0), &screens);
        assert_eq!((small.width, small.height), MIN_SIZE);
        let new = centred(WindowView::Panel, (600.0, 400.0), 1, &screens);
        assert_eq!(new.screen, 1);
        assert_eq!(new.x, (3840.0 - 600.0) / 2.0);
        assert_eq!(
            new.y,
            MENU_BAR_INSET + ((2160.0 - MENU_BAR_INSET - 400.0 - TITLE_BAR) / 2.0).round()
        );
        assert_eq!(
            centred(WindowView::Panel, (600.0, 400.0), 9, &screens).screen,
            0
        );
        let w = window(WindowView::Panel, 0, (100.0, 1000.0), (600.0, 400.0));
        assert_eq!(fit_height(&w, 700.0, &screens), 700.0);
        assert_eq!(
            fit_height(&w, 5000.0, &screens),
            2160.0 - 1000.0 - TITLE_BAR,
            "not past the screen"
        );
        assert_eq!(fit_height(&w, 10.0, &screens), MIN_SIZE.1);
    }

    /// Owner, 2026-10-22: "every instrument's surface disappeared": the
    /// placeholders and menus name each window; closing a panel window gives
    /// its cards back to the main window.
    #[test]
    fn windows_are_named_and_closing_one_gives_its_cards_back() {
        let screens = three();
        let mut layout = default_layout(&screens, None);
        let a = window(WindowView::Panel, 1, (0.0, 0.0), (600.0, 400.0));
        let b = window(WindowView::Panel, 2, (0.0, 0.0), (600.0, 400.0));
        move_card(&mut layout, "mono-synth", CardMove::New(a));
        move_card(&mut layout, "rhythm-composer", CardMove::New(b));
        layout
            .windows
            .push(window(WindowView::Schematic, 0, (0.0, 0.0), (900.0, 700.0)));
        assert_eq!(
            labels(&layout),
            [
                "Main window",
                "Panel window 1 (screen 2)",
                "Panel window 2 (screen 3)",
                "Schematic window"
            ]
        );
        assert_eq!(holder(&layout, "rhythm-composer"), Some(2));
        assert_eq!(holder(&layout, "tape-echo"), None, "in the main window");
        layout.windows.remove(1);
        assert_eq!(
            held(&layout),
            ["rhythm-composer"],
            "the synth is back in main"
        );
        assert_eq!(holder(&layout, "mono-synth"), None);
        layout.windows.push(LayoutWindow {
            view: WindowView::Main,
            ..window(WindowView::Main, 0, (0.0, 0.0), (9.0, 9.0))
        });
        assert_eq!(labels(&layout)[0], "Main window 1");
        assert_eq!(labels(&layout)[3], "Main window 2");
    }

    #[test]
    fn layouts_a_point_or_two_apart_are_similar() {
        let screens = screens_of(&desk(), Some(0));
        let layout = default_layout(&screens, None);
        let mut moved = layout.clone();
        moved.windows[0].x += 1.5;
        moved.windows[0].height -= 2.0;
        assert!(similar(&layout, &moved));
        moved.windows[0].x += 1.0;
        assert!(!similar(&layout, &moved));
        let mut full = layout.clone();
        full.windows[0].fullscreen = true;
        assert!(!similar(&layout, &full));
        let mut carded = layout.clone();
        carded.windows[0].cards.push("kick".to_owned());
        assert!(!similar(&layout, &carded));
        assert!(!similar(&layout, &Layout::default()));
    }

    /// ADR 0032: `ui/windows.toml` keeps one entry per key; writing one
    /// keeps the others (and the comments) as written; Reset removes it.
    #[test]
    fn the_layout_file_round_trips_per_key() {
        let screens = screens_of(&desk(), Some(0));
        let mut layout = default_layout(&screens, Some((900.0, 640.0)));
        let fresh = window(WindowView::Panel, 0, (100.0, 300.0), (800.0, 600.0));
        move_card(&mut layout, "kick-2", CardMove::New(fresh));
        layout.windows.push(window(
            WindowView::Schematic,
            0,
            (900.0, 0.0),
            (1600.0, 1000.0),
        ));
        let other = "# mine\n[layout.\"Monitor #1 1920x1080@1 +0+0\"]\n# kept\n[[layout.\"Monitor #1 1920x1080@1 +0+0\".windows]]\nview = \"main\"\nscreen = 0\nx = 0\ny = 40\nwidth = 1000\nheight = 700\ncards = []\n";
        let text = write_layout(other, &screens.key, Some(&layout)).unwrap();
        assert!(text.starts_with(other), "{text}");
        assert!(text.contains("view = \"schematic\""), "{text}");
        assert!(text.contains("cards = [\"kick-2\"]"), "{text}");
        assert_eq!(read_layout(&text, &screens.key), Ok(Some(layout.clone())));
        let first = read_layout(&text, "Monitor #1 1920x1080@1 +0+0")
            .unwrap()
            .unwrap();
        assert_eq!(first.windows[0].height, 700.0, "integers read as numbers");
        assert_eq!(read_layout(&text, "Monitor #2 1920x1080@1 +0+0"), Ok(None));
        let reset = write_layout(&text, &screens.key, None).unwrap();
        assert_eq!(reset, other, "reset removes the key's entry only");
        assert_eq!(read_layout("", &screens.key), Ok(None));
        assert_eq!(
            write_layout("", &screens.key, None),
            Ok(String::new()),
            "nothing to remove"
        );
        let new = write_layout("", &screens.key, Some(&layout)).unwrap();
        assert!(new.starts_with("# Signal.app's windows"), "{new}");
        assert_eq!(read_layout(&new, &screens.key), Ok(Some(layout)));

        for (bad, reason) in [
            ("layout = [", "not valid TOML"),
            ("[layout.k]\nwindows = 3\n", "needs [[layout"),
            (
                "[[layout.k.windows]]\nview = \"mixer\"\nscreen = 0\nx = 0\ny = 0\nwidth = 1\nheight = 1\n",
                "`view`",
            ),
            (
                "[[layout.k.windows]]\nview = \"main\"\nscreen = -1\nx = 0\ny = 0\nwidth = 1\nheight = 1\n",
                "`screen`",
            ),
            (
                "[[layout.k.windows]]\nview = \"main\"\nscreen = 0\nx = 0\ny = 0\nwidth = 0\nheight = 1\n",
                "positive",
            ),
            (
                "[[layout.k.windows]]\nview = \"main\"\nscreen = 0\ny = 0\nwidth = 1\nheight = 1\n",
                "`x`",
            ),
            (
                "[[layout.k.windows]]\nview = \"panel\"\nscreen = 0\nx = 0\ny = 0\nwidth = 1\nheight = 1\ncards = [\"a b\"]\n",
                "`cards`",
            ),
            (
                "[[layout.k.windows]]\nview = \"main\"\nscreen = 0\nx = 0\ny = 0\nwidth = 1\nheight = 1\ncolour = 1\n",
                "unknown key 'colour'",
            ),
        ] {
            let error = read_layout(bad, "k").unwrap_err();
            assert!(error.contains(reason), "{bad}: {error}");
        }
    }
}
