// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings, recent files, favorites, bookmarks and the open-tab session, kept as tab-separated text.
//! Windows paths cannot contain tabs or line breaks, so no escaping is needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECENT: usize = 40;

/// Where reading stopped: a page and how far down it the viewport top was.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub page: usize,
    pub fy: f32,
}

/// The pdf.js zoom choices. `Custom` holds the factor, 1.0 being 100%.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Zoom {
    #[default]
    Auto,
    Actual,
    Fit,
    Width,
    Custom(f32),
}

impl Zoom {
    /// The index the toolbar list uses: automatic, actual size, page fit, page width, a percentage.
    pub fn index(self) -> i32 {
        match self {
            Zoom::Auto => 0,
            Zoom::Actual => 1,
            Zoom::Fit => 2,
            Zoom::Width => 3,
            Zoom::Custom(_) => 4,
        }
    }

    pub fn from_index(i: i32, factor: f32) -> Zoom {
        match i {
            1 => Zoom::Actual,
            2 => Zoom::Fit,
            3 => Zoom::Width,
            4 => Zoom::Custom(factor),
            _ => Zoom::Auto,
        }
    }

    fn to_text(self) -> String {
        match self {
            Zoom::Auto => "auto".into(),
            Zoom::Actual => "actual".into(),
            Zoom::Fit => "page".into(),
            Zoom::Width => "width".into(),
            Zoom::Custom(f) => format!("{f:.4}"),
        }
    }

    fn from_text(s: &str) -> Option<Zoom> {
        Some(match s {
            "auto" => Zoom::Auto,
            "actual" => Zoom::Actual,
            "page" => Zoom::Fit,
            // Sessions written before the zoom list kept fit width as "fit".
            "width" | "fit" => Zoom::Width,
            f => Zoom::Custom(f.parse().ok()?),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Recent {
    pub path: PathBuf,
    pub opened: u64,
    pub spot: Spot,
}

#[derive(Clone, Debug)]
pub struct SessionTab {
    pub path: PathBuf,
    pub spot: Spot,
    pub zoom: Zoom,
    /// Clockwise quarter turns.
    pub turns: u8,
    pub pinned: bool,
}

/// Outer position and inner size in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: Option<usize>,
    pub window: Option<Placement>,
}

#[derive(Clone, Debug)]
pub struct Settings {
    /// 0 follows Windows, 1 light, 2 dark.
    pub theme: i32,
    pub restore_tabs: bool,
    pub auto_reload: bool,
    pub default_zoom: Zoom,
    pub sidebar: bool,
    /// 0 normal, 1 dark, 2 sepia.
    pub tone: i32,
    pub scroll_mode: i32,
    pub spread_mode: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: 0,
            restore_tabs: false,
            auto_reload: true,
            default_zoom: Zoom::Auto,
            sidebar: false,
            tone: 0,
            scroll_mode: 0,
            spread_mode: 0,
        }
    }
}

/// `BACA_DATA_DIR` lets tests run against a throwaway folder. A `data` folder next to the program
/// makes it portable: settings and history stay with the program instead of in AppData.
pub fn data_dir() -> Option<PathBuf> {
    let portable = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("data"))).filter(|p| p.is_dir());
    let dir = match (std::env::var_os("BACA_DATA_DIR"), portable) {
        (Some(d), _) => PathBuf::from(d),
        (None, Some(p)) => p,
        (None, None) => PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("io.github.zakkyhidayat.bacapdf"),
    };
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn read_lines(name: &str) -> Vec<String> {
    data_dir()
        .and_then(|d| fs::read_to_string(d.join(name)).ok())
        .map(|s| s.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Writes to a temporary file first so a crash never leaves half a file behind.
fn write_lines(name: &str, lines: &[String]) {
    let Some(dir) = data_dir() else { return };
    let tmp = dir.join(format!("{name}.tmp"));
    if fs::write(&tmp, lines.join("\n")).is_ok() {
        let _ = fs::rename(&tmp, dir.join(name));
    }
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

pub fn load_settings() -> Settings {
    let mut s = Settings::default();
    for line in read_lines("settings.tsv") {
        let Some((key, value)) = line.split_once('\t') else { continue };
        match key {
            "theme" => s.theme = value.parse().unwrap_or(0).clamp(0, 2),
            "restore_tabs" => s.restore_tabs = value == "1",
            "auto_reload" => s.auto_reload = value == "1",
            "default_zoom" => s.default_zoom = Zoom::from_text(value).unwrap_or_default(),
            "sidebar" => s.sidebar = value == "1",
            "tone" => s.tone = value.parse().unwrap_or(0).clamp(0, 2),
            "scroll_mode" => s.scroll_mode = value.parse().unwrap_or(0).clamp(0, 3),
            "spread_mode" => s.spread_mode = value.parse().unwrap_or(0).clamp(0, 2),
            _ => {}
        }
    }
    s
}

pub fn save_settings(s: &Settings) {
    let flag = |b: bool| if b { "1" } else { "0" };
    write_lines(
        "settings.tsv",
        &[
            format!("theme\t{}", s.theme),
            format!("restore_tabs\t{}", flag(s.restore_tabs)),
            format!("auto_reload\t{}", flag(s.auto_reload)),
            format!("default_zoom\t{}", s.default_zoom.to_text()),
            format!("sidebar\t{}", flag(s.sidebar)),
            format!("tone\t{}", s.tone),
            format!("scroll_mode\t{}", s.scroll_mode),
            format!("spread_mode\t{}", s.spread_mode),
        ],
    );
}

pub fn load_recents() -> Vec<Recent> {
    read_lines("recent.tsv")
        .iter()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            Some(Recent {
                path: PathBuf::from(f.first()?),
                opened: f.get(1)?.parse().ok()?,
                spot: Spot { page: f.get(2)?.parse().ok()?, fy: f.get(3)?.parse().ok()? },
            })
        })
        .collect()
}

pub fn save_recents(recents: &[Recent]) {
    let lines: Vec<String> = recents
        .iter()
        .map(|r| format!("{}\t{}\t{}\t{:.4}", r.path.display(), r.opened, r.spot.page, r.spot.fy))
        .collect();
    write_lines("recent.tsv", &lines);
}

/// Moves `path` to the front, keeping the list length bounded.
pub fn touch_recent(recents: &mut Vec<Recent>, path: &Path, spot: Option<Spot>) {
    let pos = recents.iter().position(|r| same_file(&r.path, path));
    let mut entry = match pos {
        Some(i) => recents.remove(i),
        None => Recent { path: path.to_path_buf(), opened: 0, spot: Spot::default() },
    };
    entry.opened = now();
    if let Some(spot) = spot {
        entry.spot = spot;
    }
    recents.insert(0, entry);
    recents.truncate(MAX_RECENT);
}

pub fn load_favorites() -> Vec<PathBuf> {
    read_lines("favorites.tsv").into_iter().filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

pub fn save_favorites(favorites: &[PathBuf]) {
    let lines: Vec<String> = favorites.iter().map(|p| p.display().to_string()).collect();
    write_lines("favorites.tsv", &lines);
}

/// Bookmarked pages (1-based, sorted) for each file.
pub fn load_bookmarks() -> Vec<(PathBuf, Vec<usize>)> {
    read_lines("bookmarks.tsv")
        .iter()
        .filter_map(|line| {
            let (path, pages) = line.split_once('\t')?;
            let mut pages: Vec<usize> = pages.split(',').filter_map(|p| p.parse().ok()).collect();
            pages.sort_unstable();
            pages.dedup();
            (!pages.is_empty()).then(|| (PathBuf::from(path), pages))
        })
        .collect()
}

pub fn save_bookmarks(bookmarks: &[(PathBuf, Vec<usize>)]) {
    let lines: Vec<String> = bookmarks
        .iter()
        .filter(|(_, pages)| !pages.is_empty())
        .map(|(path, pages)| {
            let list: Vec<String> = pages.iter().map(|p| p.to_string()).collect();
            format!("{}\t{}", path.display(), list.join(","))
        })
        .collect();
    write_lines("bookmarks.tsv", &lines);
}

pub fn load_session() -> Session {
    let lines = read_lines("session.tsv");
    let mut session = Session::default();
    let mut rows = lines.iter();
    session.active = rows.next().and_then(|l| l.strip_prefix("active\t")).and_then(|v| v.parse().ok());
    for line in rows {
        let f: Vec<&str> = line.split('\t').collect();
        if f.first() == Some(&"window") {
            let n = |i: usize| f.get(i).and_then(|v| v.parse::<i64>().ok());
            if let (Some(x), Some(y), Some(w), Some(h)) = (n(1), n(2), n(3), n(4)) {
                session.window = Some(Placement {
                    x: x as i32,
                    y: y as i32,
                    width: w.max(1) as u32,
                    height: h.max(1) as u32,
                    maximized: f.get(5) == Some(&"max"),
                });
            }
            continue;
        }
        if f.first() == Some(&"prefs") {
            continue;
        }
        let parsed = (|| {
            Some(SessionTab {
                path: PathBuf::from(f.first()?),
                spot: Spot { page: f.get(1)?.parse().ok()?, fy: f.get(2)?.parse().ok()? },
                zoom: Zoom::from_text(f.get(3)?)?,
                turns: f.get(4).and_then(|t| t.parse().ok()).unwrap_or(0) % 4,
                pinned: f.get(5) == Some(&"pin"),
            })
        })();
        session.tabs.extend(parsed);
    }
    if session.active.is_some_and(|a| a >= session.tabs.len()) {
        session.active = None;
    }
    session
}

pub fn save_session(session: &Session) {
    let mut lines = vec![match session.active {
        Some(a) => format!("active\t{a}"),
        None => "active\thome".into(),
    }];
    if let Some(w) = session.window {
        let state = if w.maximized { "max" } else { "normal" };
        lines.push(format!("window\t{}\t{}\t{}\t{}\t{state}", w.x, w.y, w.width, w.height));
    }
    for t in &session.tabs {
        lines.push(format!("{}\t{}\t{:.4}\t{}\t{}\t{}", t.path.display(), t.spot.page, t.spot.fy, t.zoom.to_text(), t.turns, if t.pinned { "pin" } else { "-" }));
    }
    write_lines("session.tsv", &lines);
}

/// Appends one line to error.log in the settings folder.
pub fn log_error(message: &str) {
    use std::io::Write;
    let Some(dir) = data_dir() else { return };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("error.log")) {
        let _ = writeln!(file, "{}\t{}", describe_stamp(now()), message.replace('\n', " "));
    }
}

pub fn error_log_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("error.log")).filter(|p| p.exists())
}

/// "Just now", "Today 14:05", "Yesterday", "3 days ago", or a date for anything older than a week.
pub fn describe_time(then: u64, now: u64) -> String {
    if now.saturating_sub(then) < 60 {
        return "Just now".into();
    }
    let offset = local_offset_secs();
    let day = |t: u64| (t as i64 + offset).div_euclid(86_400);
    let days = day(now) - day(then);
    let local = then as i64 + offset;
    match days {
        i64::MIN..=0 => {
            let mins = local.rem_euclid(86_400) / 60;
            format!("Today {:02}:{:02}", mins / 60, mins % 60)
        }
        1 => "Yesterday".into(),
        2..=6 => format!("{days} days ago"),
        _ => describe_date(local),
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn describe_date(local: i64) -> String {
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    format!("{} {} {}", d, MONTHS[(m - 1) as usize], y)
}

fn describe_stamp(t: u64) -> String {
    let local = t as i64 + local_offset_secs();
    let mins = local.rem_euclid(86_400) / 60;
    format!("{} {:02}:{:02}", describe_date(local), mins / 60, mins % 60)
}

/// A PDF date ("D:20240131093000+07'00'") as "31 Jan 2024 09:30".
pub fn describe_pdf_date(raw: &str) -> String {
    let digits: String = raw.trim_start_matches("D:").chars().take_while(|c| c.is_ascii_digit()).collect();
    let part = |a: usize, b: usize| digits.get(a..b).and_then(|s| s.parse::<i64>().ok());
    match (part(0, 4), part(4, 6), part(6, 8)) {
        (Some(y), Some(m), Some(d)) if (1..=12).contains(&m) => {
            let time = match (part(8, 10), part(10, 12)) {
                (Some(h), Some(min)) => format!(" {h:02}:{min:02}"),
                _ => String::new(),
            };
            format!("{d} {} {y}{time}", MONTHS[(m - 1) as usize])
        }
        _ => raw.to_string(),
    }
}

fn local_offset_secs() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    let kind = unsafe { GetTimeZoneInformation(&mut tz) };
    // 2 = TIME_ZONE_ID_DAYLIGHT
    let bias = tz.Bias + if kind == 2 { tz.DaylightBias } else { tz.StandardBias };
    -(bias as i64) * 60
}

/// Days since 1970-01-01 to (year, month, day), from Howard Hinnant's civil calendar algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Remembers which files had which modification time, for reloading changed files.
pub fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
