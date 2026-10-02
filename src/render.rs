//! Segment rendering and width fitting.
//!
//! Each segment yields spans; a span may carry its own color (the cache-rate
//! ramp or mode badges), otherwise it takes the segment's
//! text color. Styles are closed with `ESC[22m ESC[39m` / `ESC[49m` rather
//! than a full reset: the TUI wraps the line in chalk.hex(colors.text), and
//! chalk re-opens its color at each of its own close codes, whereas `ESC[0m`
//! would leave the rest of the line in the terminal's default foreground.

use crate::appearance::{Models, Palette, Rgb};
use crate::config::{AnsiColor, Config, SegmentConfig, SegmentId, StyleMode};
use crate::payload::Payload;
use crate::probe::{GitStatus, PullRequest};
use crate::session::{SessionStats, Usage};
use unicode_width::UnicodeWidthChar;

const CLOSE_FG: &str = "\x1b[22m\x1b[39m";
const CLOSE_BG: &str = "\x1b[49m";
const POWERLINE_ARROW: &str = "\u{e0b0}";

/// Everything a render needs, gathered up front so fitting can re-render
/// cheaply at each degradation stage.
pub struct Ctx {
    pub payload: Payload,
    pub config: Config,
    pub palette: Palette,
    pub models: Models,
    pub stats: Option<SessionStats>,
    pub tps: Option<crate::tps::Estimate>,
    /// effort from the wire, else the model's default_effort
    pub effort: Option<serde_json::Value>,
    pub session_created: Option<f64>,
    pub git: Option<GitStatus>,
    pub pr: Option<PullRequest>,
    pub quota: Option<crate::quota::Quota>,
    pub now: f64,
    pub color: bool,
}

struct Span {
    text: String,
    color: Option<AnsiColor>,
    bold: bool,
    /// SGR faint: dims whatever color ends up applied, so it also works on
    /// powerline backgrounds where the segment's text color wins
    dim: bool,
}

fn plain(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        color: None,
        bold: false,
        dim: false,
    }
}

fn colored(text: impl Into<String>, color: AnsiColor) -> Span {
    Span {
        text: text.into(),
        color: Some(color),
        bold: false,
        dim: false,
    }
}

fn token(name: &str) -> AnsiColor {
    AnsiColor::Named(name.into())
}

fn rgb(Rgb(r, g, b): Rgb) -> AnsiColor {
    AnsiColor::Rgb { r, g, b }
}

impl Ctx {
    fn sgr(&self, color: &AnsiColor, bg: bool) -> Option<String> {
        self.color.then(|| color.sgr(&self.palette, bg)).flatten()
    }

    /// A theme's fixed text color as drawn straight on the terminal: bright
    /// ANSI colors wash out on light schemes, and RGB / 256 colors are
    /// nudged until they read on the palette's background. Palette tokens
    /// are already chosen for it, and colors on a segment background are
    /// left to the theme.
    fn on_terminal(&self, color: &AnsiColor) -> AnsiColor {
        match color {
            AnsiColor::Named(_) => color.clone(),
            AnsiColor::Color16 { c16 } if *c16 >= 8 && self.palette.is_light() => {
                AnsiColor::Color16 { c16: c16 - 8 }
            }
            AnsiColor::Color16 { .. } => color.clone(),
            _ => match color.to_rgb(&self.palette) {
                Some(c) => rgb(self.palette.readable(c)),
                None => color.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// formatting helpers
// ---------------------------------------------------------------------------

pub fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    // 999_950 would round up to "1000.0k"; promote it to M instead
    let k = format!("{:.1}", n as f64 / 1e3);
    if n < 1_000_000 && k != "1000.0" {
        return format!("{k}k");
    }
    format!("{:.2}M", n as f64 / 1e6)
}

/// fmt_tokens without trailing zeros, for round capacities: `620k/1M`.
fn fmt_tokens_short(n: u64) -> String {
    let s = fmt_tokens(n);
    if !s.contains('.') {
        return s;
    }
    let (digits, unit) = s.split_at(s.len() - 1);
    format!(
        "{}{unit}",
        digits.trim_end_matches('0').trim_end_matches('.')
    )
}

/// An 8-cell share bar with a trailing space.
fn meter(ratio: f64, color: Option<AnsiColor>) -> Span {
    let filled = (ratio.clamp(0.0, 1.0) * 8.0).round() as usize;
    Span {
        text: format!("{}{} ", "█".repeat(filled), "░".repeat(8 - filled)),
        color,
        bold: false,
        dim: false,
    }
}

/// Modern providers sit at 95%+ almost always, so up there one decimal is
/// kept; only a true 100% stays an integer.
fn fmt_rate(r: f64) -> String {
    if r >= 100.0 {
        "100%".into()
    } else if r >= 95.0 {
        format!("{:.1}%", r.min(99.9))
    } else {
        format!("{}%", r.round() as i64)
    }
}

fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb(to(r), to(g), to(b))
}

/// (rate%, hue, saturation, lightness): brick red through amber into jade,
/// with most of the resolution spent above 95% where real sessions live.
const CACHE_STOPS: [(f64, f64, f64, f64); 9] = [
    (0.0, 2.0, 0.55, 0.52),
    (50.0, 22.0, 0.58, 0.50),
    (75.0, 38.0, 0.60, 0.49),
    (90.0, 55.0, 0.58, 0.47),
    (95.0, 80.0, 0.52, 0.46),
    (97.0, 100.0, 0.48, 0.45),
    (98.0, 112.0, 0.46, 0.45),
    (99.0, 124.0, 0.44, 0.45),
    (100.0, 140.0, 0.42, 0.45),
];

fn cache_color(rate: f64) -> Rgb {
    let rate = rate.clamp(0.0, 100.0);
    for w in CACHE_STOPS.windows(2) {
        let (r0, h0, s0, l0) = w[0];
        let (r1, h1, s1, l1) = w[1];
        if rate <= r1 {
            let t = (rate - r0) / (r1 - r0);
            return hsl_to_rgb(h0 + (h1 - h0) * t, s0 + (s1 - s0) * t, l0 + (l1 - l0) * t);
        }
    }
    let (_, h, s, l) = CACHE_STOPS[CACHE_STOPS.len() - 1];
    hsl_to_rgb(h, s, l)
}

/// Upstream shortenCwd: `~` for home, otherwise the last N segments.
pub fn shorten_cwd(path: &str, depth: usize) -> String {
    let mut path = path.replace('\\', "/");
    if let Some(home) = crate::paths::home_dir() {
        let home = home.to_string_lossy().replace('\\', "/");
        if path == home {
            return "~".into();
        }
        if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
            path = format!("~/{rest}");
        }
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if depth == 0 || segs.len() <= depth {
        return path;
    }
    format!("…/{}", segs[segs.len() - depth..].join("/"))
}

/// OSC 8 hyperlink, like upstream toTerminalHyperlink (http(s) only).
fn hyperlink(text: &str, url: &str) -> String {
    if !url.chars().any(char::is_control)
        && (url.starts_with("https://") || url.starts_with("http://"))
    {
        format!("\x1b]8;;{url}\x07{text}\x1b]8;;\x07")
    } else {
        text.to_string()
    }
}

/// The spans of one segment, or None when it has nothing to show. `compact`
/// trims labels and units for narrow terminals.
fn segment(ctx: &Ctx, seg: &SegmentConfig, compact: bool) -> Option<Vec<Span>> {
    let p = &ctx.payload;
    match seg.id {
        SegmentId::Mode => (!p.mode.is_empty()).then(|| vec![plain(&p.mode)]),
        // the currency sign moves to the icon when there is one
        SegmentId::Cost => p.cost_usd.map(|cost| {
            let sign = if icon_of(ctx, seg).is_empty() {
                "$"
            } else {
                ""
            };
            vec![plain(format!("{sign}{cost:.2}"))]
        }),
        SegmentId::Model => {
            if p.model.is_empty() {
                return None;
            }
            let mut label = ctx.models.display(&p.model);
            match &ctx.effort {
                Some(serde_json::Value::String(e)) if e != "off" => label += &format!(" {e}"),
                Some(serde_json::Value::Bool(true)) => label += " thinking",
                _ => {}
            }
            Some(vec![plain(label)])
        }
        SegmentId::OutputStyle => {
            (!p.output_style.is_empty()).then(|| vec![plain(&p.output_style)])
        }
        SegmentId::Tps => {
            let estimate = ctx.tps?;
            let stale_secs = seg.opt_int("stale_secs", 300);
            let stale = (stale_secs > 0 && ctx.now - estimate.measured_at > stale_secs as f64)
                || ctx
                    .stats
                    .as_ref()
                    .is_some_and(|s| s.state != crate::session::CollectionState::Complete);
            if stale && seg.opt_bool("hide_when_stale", false) {
                return None;
            }
            let unit = if compact { "t/s" } else { "tok/s" };
            // A summed rate needs its parallelism to be read correctly.
            let active = ctx.stats.as_ref().map_or(0, |s| s.active_logs);
            let parallel = if active > 1 {
                format!(" ×{active}")
            } else {
                String::new()
            };
            Some(vec![Span {
                text: format!("≈{:.0} {unit}{parallel}", estimate.tokens_per_sec),
                color: stale.then(|| token("text_muted")),
                bold: false,
                dim: stale,
            }])
        }
        SegmentId::Directory => {
            if p.cwd.is_empty() {
                return None;
            }
            let depth = if compact {
                1
            } else {
                seg.opt_int("depth", 3).max(0) as usize
            };
            Some(vec![plain(crate::payload::label(&shorten_cwd(
                &p.cwd, depth,
            )))])
        }
        SegmentId::Git => {
            let branch = p.git_branch.as_deref().filter(|b| !b.is_empty())?;
            let mut text = crate::payload::label(branch);
            if let Some(g) = ctx.git {
                let mut parts = Vec::new();
                if g.added > 0 || g.deleted > 0 {
                    let mut diff = Vec::new();
                    if g.added > 0 {
                        diff.push(format!("+{}", g.added));
                    }
                    if g.deleted > 0 {
                        diff.push(format!("-{}", g.deleted));
                    }
                    parts.push(diff.join(" "));
                } else if g.dirty {
                    parts.push("±".into());
                }
                if g.conflicts {
                    parts.push("⚠".into());
                }
                let mut sync = String::new();
                if g.ahead > 0 {
                    sync += &format!("↑{}", g.ahead);
                }
                if g.behind > 0 {
                    sync += &format!("↓{}", g.behind);
                }
                if !sync.is_empty() {
                    parts.push(sync);
                }
                if !parts.is_empty() {
                    if compact {
                        text += "*";
                    } else {
                        text += &format!(" [{}]", parts.join(" "));
                    }
                }
            }
            let mut spans = vec![plain(text)];
            if let Some(pr) = &ctx.pr {
                let badge = format!("[PR#{}]", pr.number);
                let badge = if ctx.color && seg.opt_bool("pr_link", true) {
                    hyperlink(&badge, &pr.url)
                } else {
                    badge
                };
                spans.push(plain(" "));
                spans.push(colored(badge, token("primary")));
            }
            Some(spans)
        }
        SegmentId::Context => {
            if p.max_context_tokens == 0 {
                return None;
            }
            let ratio = p.context_usage.clamp(0.0, 1.0);
            let pct = (ratio * 100.0).round() as u32;
            // judged on the shown number, so "85%" is never the warning color
            let tok = if pct >= 85 {
                "error"
            } else if pct >= 60 {
                "warning"
            } else {
                "success"
            };
            let color = seg.opt_bool("colorful", true).then(|| token(tok));
            let mut spans = vec![plain("ctx ")];
            if seg.opt_bool("bar", false) && !compact {
                spans.push(meter(ratio, color.clone()));
            }
            spans.push(Span {
                text: format!("{pct}%"),
                color,
                bold: false,
                dim: false,
            });
            if !compact && seg.opt_bool("show_tokens", true) {
                spans.push(colored(
                    format!(
                        " · {}/{}",
                        fmt_tokens_short(p.context_tokens),
                        fmt_tokens_short(p.max_context_tokens)
                    ),
                    token("text_muted"),
                ));
            }
            Some(spans)
        }
        SegmentId::Usage => {
            let st = ctx.stats.as_ref()?;
            if st.total.is_empty() {
                return None;
            }
            let mut spans = Vec::new();
            if !compact {
                spans.push(plain("total "));
            }
            spans.extend(usage_triple(seg, &st.total, compact));
            if st.state != crate::session::CollectionState::Complete {
                spans.insert(0, plain("≈"));
                for span in &mut spans {
                    span.dim = true;
                }
            }
            Some(spans)
        }
        SegmentId::Subagent => {
            let st = ctx.stats.as_ref()?;
            let mut models: Vec<_> = st
                .sub_by_model
                .iter()
                .filter(|(_, u)| !u.is_empty())
                .collect();
            if models.is_empty() {
                return None;
            }
            models.sort_by(|(a, ua), (b, ub)| ub.input().cmp(&ua.input()).then_with(|| a.cmp(b)));
            let mut spans = vec![plain("sub ")];
            if compact {
                // Narrow lines keep the subagent total rather than one model.
                let mut total = Usage::default();
                for (_, u) in &models {
                    total.add(u);
                }
                spans.extend(usage_triple(seg, &total, compact));
            } else {
                const LIMIT: usize = 2;
                for (i, (model, u)) in models.iter().take(LIMIT).enumerate() {
                    if i > 0 {
                        spans.push(plain("; "));
                    }
                    spans.push(plain(format!("{} ", ctx.models.display(model))));
                    spans.extend(usage_triple(seg, u, compact));
                }
                let more = models.len().saturating_sub(LIMIT);
                if more > 0 {
                    spans.push(colored(
                        if more == 1 {
                            " +1 model".into()
                        } else {
                            format!(" +{more} models")
                        },
                        token("text_muted"),
                    ));
                }
            }
            if st.state != crate::session::CollectionState::Complete {
                spans.insert(0, plain("≈"));
                for span in &mut spans {
                    span.dim = true;
                }
            }
            Some(spans)
        }
        SegmentId::Quota => quota_segment(ctx, seg, compact),
        SegmentId::Changes => (p.lines_added > 0 || p.lines_removed > 0)
            .then(|| vec![plain(format!("+{} -{}", p.lines_added, p.lines_removed))]),
        SegmentId::Session => {
            let created = ctx.session_created?;
            let secs = (ctx.now - created).max(0.0) as u64;
            Some(vec![plain(fmt_duration(secs))])
        }
    }
}

fn usage_triple(seg: &SegmentConfig, u: &Usage, compact: bool) -> Vec<Span> {
    let colorful = seg.opt_bool("colorful", true);
    let pick = |c: AnsiColor| colorful.then_some(c);
    let mut spans = vec![
        Span {
            text: format!(
                "↑{}{}",
                if compact { "" } else { " " },
                fmt_tokens(u.input())
            ),
            color: pick(AnsiColor::Color16 { c16: 4 }),
            bold: false,
            dim: false,
        },
        plain(if compact { " " } else { " · " }),
        Span {
            text: format!(
                "↓{}{}",
                if compact { "" } else { " " },
                fmt_tokens(u.output)
            ),
            color: pick(AnsiColor::Color16 { c16: 5 }),
            bold: false,
            dim: false,
        },
    ];
    if seg.opt_bool("show_cache", true) {
        if let Some(r) = u.cache_rate() {
            let label = if compact { " " } else { " cache " };
            spans.push(Span {
                text: format!("{label}{}", fmt_rate(r)),
                color: pick(rgb(cache_color(r))),
                bold: false,
                dim: false,
            });
        }
    }
    spans
}

/// `5h 42% ↻1h20m · 7d 13% ↻Mon 08:00`: used share of each plan window
/// plus when it resets. Within a day the reset reads as a countdown, further
/// out as a local weekday + time.
fn quota_segment(ctx: &Ctx, seg: &SegmentConfig, compact: bool) -> Option<Vec<Span>> {
    let q = ctx.quota.as_ref()?;
    let windows = [
        ("5h", q.limit_5h.as_ref(), seg.opt_bool("show_5h", true)),
        ("7d", q.limit_7d.as_ref(), seg.opt_bool("show_7d", true)),
        ("spend", q.spend.as_ref(), seg.opt_bool("show_spend", true)),
    ];
    let colorful = seg.opt_bool("colorful", true);
    // reset times survive compaction: they are what the segment is for
    let show_reset = seg.opt_bool("show_reset", true);
    let bar = seg.opt_bool("bar", false) && !compact;
    let mut spans = Vec::new();
    for (label, entry, wanted) in windows {
        let Some(e) = entry.filter(|_| wanted) else {
            continue;
        };
        if !spans.is_empty() {
            spans.push(plain(if compact { " " } else { " · " }));
        }
        let ratio = e.used_ratio.max(0.0);
        // ceil like upstream usagePercent: any use shows at least 1%; the
        // epsilon keeps 0.07 * 100 = 7.000000000000001 from reading as 8%
        let pct = (ratio * 100.0 - 1e-9).ceil().max(0.0) as u32;
        let tok = if pct >= 85 {
            "error"
        } else if pct >= 50 {
            "warning"
        } else {
            "success"
        };
        spans.push(plain(format!("{label} ")));
        if bar {
            spans.push(meter(ratio, colorful.then(|| token(tok))));
        }
        spans.push(Span {
            text: format!("{pct}%"),
            color: colorful.then(|| token(tok)),
            bold: false,
            dim: false,
        });
        if show_reset {
            if let Some(reset) = e
                .reset_at
                .as_deref()
                .and_then(|r| fmt_reset(r, ctx.now, compact))
            {
                spans.push(plain(format!(" ↻{reset}")));
            }
        }
    }
    (!spans.is_empty()).then_some(spans)
}

fn fmt_reset(rfc3339: &str, now: f64, compact: bool) -> Option<String> {
    use chrono::{DateTime, Local};
    let at = DateTime::parse_from_rfc3339(rfc3339).ok()?;
    let secs = at.timestamp() as f64 - now;
    if secs <= 0.0 {
        return Some("now".into());
    }
    if secs < 86_400.0 {
        let m = (secs / 60.0).ceil() as u64;
        return Some(if m >= 60 {
            format!("{}h{:02}m", m / 60, m % 60)
        } else {
            format!("{m}m")
        });
    }
    if compact {
        // "3d", "6d": the day count is enough when space is short
        return Some(format!("{}d", (secs / 86_400.0).round() as u64));
    }
    Some(at.with_timezone(&Local).format("%a %H:%M").to_string())
}

// ---------------------------------------------------------------------------
// painting
// ---------------------------------------------------------------------------

struct Rendered<'a> {
    seg: &'a SegmentConfig,
    text: String,
}

fn icon_of<'a>(ctx: &Ctx, seg: &'a SegmentConfig) -> &'a str {
    match ctx.config.style.mode {
        StyleMode::Plain => &seg.icon.plain,
        StyleMode::NerdFont | StyleMode::Powerline => &seg.icon.nerd_font,
    }
}

/// Paint one segment: icon, then spans in their own color or the segment's
/// text color. A background (powerline) segment gets one-space padding and
/// leaves its background open; the joiner closes it.
fn paint_segment(ctx: &Ctx, seg: &SegmentConfig, spans: Vec<Span>) -> String {
    let bg = seg
        .colors
        .background
        .as_ref()
        .and_then(|c| ctx.sgr(c, true));
    let bold = seg.styles.text_bold;
    let mut out = String::new();
    if let Some(bg) = &bg {
        out += &format!("\x1b[{bg}m ");
    }
    let fit = |c: Option<&AnsiColor>| match (c, &bg) {
        (Some(c), None) => Some(ctx.on_terminal(c)),
        (c, _) => c.cloned(),
    };
    let icon = icon_of(ctx, seg);
    if !icon.is_empty() {
        out += &paint(ctx, icon, fit(seg.colors.icon.as_ref()).as_ref(), false);
        out.push(' ');
    }
    for span in spans {
        // on a background, per-span accents (badges, ramps) would fight it:
        // the segment's own text color, chosen for that background, wins
        let color = if bg.is_some() {
            seg.colors.text.as_ref().or(span.color.as_ref())
        } else {
            span.color.as_ref().or(seg.colors.text.as_ref())
        };
        let color = fit(color);
        out += &paint_styled(ctx, &span.text, color.as_ref(), bold || span.bold, span.dim);
    }
    if bg.is_some() {
        out.push(' ');
    }
    out
}

fn paint(ctx: &Ctx, text: &str, color: Option<&AnsiColor>, bold: bool) -> String {
    paint_styled(ctx, text, color, bold, false)
}

fn paint_styled(ctx: &Ctx, text: &str, color: Option<&AnsiColor>, bold: bool, dim: bool) -> String {
    if text.is_empty() || !ctx.color {
        return text.to_string();
    }
    let mut codes = Vec::new();
    if let Some(c) = color.and_then(|c| ctx.sgr(c, false)) {
        codes.push(c);
    }
    // bold and faint share SGR 22 as their reset, which CLOSE_FG emits
    if dim {
        codes.push("2".into());
    } else if bold {
        codes.push("1".into());
    }
    if codes.is_empty() {
        text.to_string()
    } else {
        format!("\x1b[{}m{text}{CLOSE_FG}", codes.join(";"))
    }
}

fn join(ctx: &Ctx, parts: &[Rendered]) -> String {
    let style = &ctx.config.style;
    let powerline = style.separator == POWERLINE_ARROW;
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let bg = part.seg.colors.background.as_ref();
        if i > 0 {
            let prev_bg = parts[i - 1].seg.colors.background.as_ref();
            if powerline && (prev_bg.is_some() || bg.is_some()) {
                out += &arrow(ctx, prev_bg, bg);
            } else {
                if prev_bg.is_some() && ctx.color {
                    out += CLOSE_BG;
                }
                let sep_color = style.separator_color.clone().unwrap_or(token("text_muted"));
                out += &paint(ctx, &style.separator, Some(&sep_color), false);
            }
        }
        out += &part.text;
    }
    if let Some(last) = parts.last() {
        if let Some(bg) = last.seg.colors.background.as_ref() {
            if powerline {
                out += &arrow(ctx, Some(bg), None);
            } else if ctx.color {
                out += CLOSE_BG;
            }
        }
    }
    out
}

/// Powerline transition: the arrow takes the previous background as its
/// foreground, drawn over the next segment's background.
fn arrow(ctx: &Ctx, prev: Option<&AnsiColor>, next: Option<&AnsiColor>) -> String {
    if !ctx.color {
        return POWERLINE_ARROW.into();
    }
    let mut out = String::new();
    match next.and_then(|c| ctx.sgr(c, true)) {
        Some(bg) => out += &format!("\x1b[{bg}m"),
        None => out += CLOSE_BG,
    }
    match prev.and_then(|c| ctx.sgr(c, false)) {
        Some(fg) => out += &format!("\x1b[{fg}m{POWERLINE_ARROW}{CLOSE_FG}"),
        None => out += POWERLINE_ARROW,
    }
    out
}

fn compose(ctx: &Ctx, compact: bool, dropped: &[SegmentId]) -> String {
    let parts: Vec<Rendered> = ctx
        .config
        .segments
        .iter()
        .filter(|s| s.enabled && !dropped.contains(&s.id))
        .filter_map(|seg| {
            let spans = segment(ctx, seg, compact)?;
            Some(Rendered {
                seg,
                text: paint_segment(ctx, seg, spans),
            })
        })
        .collect();
    join(ctx, &parts)
}

/// Render, then degrade until the line fits `width`: compact labels first,
/// then drop segments from least to most useful, and only as a last resort
/// cut with an ellipsis. `None` width returns the full line.
pub fn render(ctx: &Ctx, width: Option<usize>) -> String {
    if width == Some(0) {
        return String::new();
    }
    let full = compose(ctx, false, &[]);
    let Some(width) = width else { return full };
    if visible_width(&full) <= width {
        return full;
    }
    use SegmentId::*;
    const DROP_ORDER: [SegmentId; 11] = [
        Session,
        Changes,
        Git,
        Directory,
        Subagent,
        // the compact rate is short and the only live signal of parallel work
        Tps,
        OutputStyle,
        Cost,
        Context,
        Quota,
        Mode,
    ];
    let mut dropped = Vec::new();
    let mut line = compose(ctx, true, &dropped);
    for id in DROP_ORDER {
        if visible_width(&line) <= width {
            return line;
        }
        dropped.push(id);
        line = compose(ctx, true, &dropped);
    }
    if visible_width(&line) <= width {
        return line;
    }
    truncate(&line, width, ctx.color)
}

fn ansi_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.first() != Some(&0x1b) {
        return None;
    }
    match b.get(1) {
        // CSI: ESC [ params final-byte
        Some(b'[') => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map(|i| i + 3),
        // OSC: ESC ] ... BEL | ESC \
        Some(b']') => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return Some(i + 1);
                }
                if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return Some(i + 2);
                }
                i += 1;
            }
            Some(b.len())
        }
        _ => Some(1),
    }
}

/// (bytes, columns) of the character at the start of `s` together with the
/// zero-width marks that follow it, so a cut never strands a variation
/// selector. VS16 asks for emoji presentation, which terminals draw 2 wide
/// (🛡️ is U+1F6E1 U+FE0F and U+1F6E1 alone is 1 column).
fn cluster(s: &str) -> (usize, usize) {
    let mut chars = s.chars();
    let Some(base) = chars.next() else {
        return (0, 0);
    };
    let mut len = base.len_utf8();
    let mut w = base.width().unwrap_or(0);
    for ch in chars {
        if ch.width() != Some(0) {
            break;
        }
        if ch == '\u{fe0f}' {
            w = w.max(2);
        }
        len += ch.len_utf8();
    }
    (len, w)
}

/// Terminal columns of a rendered line: escapes are zero-width, East Asian
/// wide characters and emoji presentation count 2.
pub fn visible_width(s: &str) -> usize {
    let mut w = 0;
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = ansi_len(&s[i..]) {
            i += n;
            continue;
        }
        let (n, cw) = cluster(&s[i..]);
        w += cw;
        i += n;
    }
    w
}

pub(crate) fn truncate(s: &str, width: usize, color: bool) -> String {
    let mut out = String::new();
    let mut used = 0;
    let mut i = 0;
    let mut link_open = false;
    while i < s.len() {
        if let Some(n) = ansi_len(&s[i..]) {
            let seq = &s[i..i + n];
            if let Some(rest) = seq.strip_prefix("\x1b]8;") {
                // `ESC]8;;BEL` closes a link; anything with a URI opens one
                link_open = !matches!(rest, ";\x07" | ";\x1b\\");
            }
            out.push_str(seq);
            i += n;
            continue;
        }
        let (n, w) = cluster(&s[i..]);
        if used + w + 1 > width {
            break;
        }
        out.push_str(&s[i..i + n]);
        used += w;
        i += n;
    }
    out.push('…');
    if link_open {
        out.push_str("\x1b]8;;\x07");
    }
    if color {
        out.push_str(CLOSE_FG);
        out.push_str(CLOSE_BG);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_presentation_is_two_columns_and_kept_whole() {
        assert_eq!(visible_width("🛡️ x"), 4);
        assert_eq!(visible_width("é"), 1); // e + combining acute
                                           // the selector is never cut off its base
        assert_eq!(truncate("🛡️🛡️", 3, false), "🛡️…");
        assert_eq!(truncate("🛡️🛡️", 2, false), "…");
    }

    #[test]
    fn estimated_tps_compacts_and_marks_idle_measurements() {
        use ansi_to_tui::IntoText;
        let is_dim = |line: &str| {
            line.into_text()
                .unwrap()
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| {
                    span.style
                        .add_modifier
                        .contains(ratatui::style::Modifier::DIM)
                })
        };
        let mut config = crate::themes::builtin("claude").unwrap();
        config.segments.retain(|s| s.id == SegmentId::Tps);
        let mut ctx = Ctx {
            payload: Payload::default(),
            config,
            palette: crate::appearance::DARK,
            models: Models::default(),
            stats: None,
            tps: Some(crate::tps::Estimate {
                tokens_per_sec: 42.0,
                measured_at: 100.0,
            }),
            effort: None,
            session_created: None,
            git: None,
            pr: None,
            quota: None,
            now: 110.0,
            color: true,
        };
        let fresh = render(&ctx, None);
        assert!(fresh.contains("≈42 tok/s") && !is_dim(&fresh));
        let compact = render(&ctx, Some(7));
        assert!(compact.contains("≈42 t/s"));
        assert_eq!(visible_width(&compact), 7);
        ctx.stats = Some(SessionStats {
            state: crate::session::CollectionState::Partial,
            ..Default::default()
        });
        assert!(
            is_dim(&render(&ctx, None)),
            "incomplete input dims even a recent estimate"
        );
        ctx.stats = None;
        ctx.now = 500.0;
        ctx.config.segments[0].styles.text_bold = true;
        let idle = render(&ctx, None);
        assert!(idle.contains("≈42 tok/s") && is_dim(&idle));
        ctx.config.segments[0]
            .options
            .insert("hide_when_stale".into(), true.into());
        assert_eq!(render(&ctx, None), "");
    }

    #[test]
    fn tokens_and_rates() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(12_345), "12.3k");
        assert_eq!(fmt_tokens(1_234_567), "1.23M");
        assert_eq!(fmt_tokens(999_949), "999.9k");
        assert_eq!(fmt_tokens(999_950), "1.00M");
        assert_eq!(fmt_tokens_short(1_000_000), "1M");
        assert_eq!(fmt_tokens_short(620_000), "620k");
        assert_eq!(fmt_tokens_short(1_500_000), "1.5M");
        assert_eq!(fmt_tokens_short(12_345), "12.3k");
        assert_eq!(fmt_tokens_short(999), "999");
        assert_eq!(fmt_duration(3_900), "1h05m");
        assert_eq!(fmt_rate(97.84), "97.8%");
        assert_eq!(fmt_rate(99.99), "99.9%");
        assert_eq!(fmt_rate(100.0), "100%");
        assert_eq!(fmt_rate(42.4), "42%");
    }

    #[test]
    fn width_ignores_escapes_and_counts_cjk() {
        assert_eq!(visible_width("\x1b[38;2;1;2;3mab\x1b[22m\x1b[39m"), 2);
        assert_eq!(visible_width("缓存"), 4);
        assert_eq!(visible_width("\x1b]8;;https://x\x07PR\x1b]8;;\x07"), 2);
        assert_eq!(
            visible_width(&truncate("\x1b[34mabcdef\x1b[39m", 4, false)),
            4
        );
    }

    #[test]
    fn truncation_closes_a_cut_hyperlink() {
        let line = "ab \x1b]8;;https://x/1\x07[PR#1]\x1b]8;;\x07";
        let cut = truncate(line, 6, false);
        assert!(cut.ends_with("…\x1b]8;;\x07"), "{cut:?}");
        assert!(!truncate("abcdef", 4, false).contains("\x1b]8"));
    }

    #[test]
    fn quota_percent_is_not_inflated_by_float_error() {
        for (used, shown) in [(7.0, "7%"), (56.0, "56%"), (0.2, "1%"), (0.0, "0%")] {
            let mut config = crate::themes::builtin("claude").unwrap();
            config.segments.retain(|s| s.id == SegmentId::Quota);
            config.segments[0]
                .options
                .insert("show_reset".into(), false.into());
            let ctx = Ctx {
                payload: Payload::default(),
                config,
                palette: crate::appearance::DARK,
                models: Models::default(),
                stats: None,
                tps: None,
                effort: None,
                session_created: None,
                git: None,
                pr: None,
                quota: Some(crate::quota::from_payload(
                    &serde_json::json!({"five_hour":{"used_percentage":used}}),
                )),
                now: 0.0,
                color: false,
            };
            assert!(
                render(&ctx, None).ends_with(&format!("5h {shown}")),
                "{used}"
            );
        }
    }

    #[test]
    fn cost_sign_moves_to_the_icon() {
        for (theme, shown) in [
            ("claude", "$1.23"),
            ("minimal", "$ 1.23"),
            ("cometix", "\u{f155} 1.23"),
        ] {
            let mut config = crate::themes::builtin(theme).unwrap();
            config.segments.retain(|s| s.id == SegmentId::Cost);
            let ctx = Ctx {
                payload: Payload {
                    cost_usd: Some(1.23),
                    ..Payload::default()
                },
                config,
                palette: crate::appearance::DARK,
                models: Models::default(),
                stats: None,
                tps: None,
                effort: None,
                session_created: None,
                git: None,
                pr: None,
                quota: None,
                now: 0.0,
                color: false,
            };
            assert_eq!(render(&ctx, None).trim(), shown, "{theme}");
        }
    }

    #[test]
    fn cwd_shortening() {
        assert_eq!(shorten_cwd("/a/b/c/d/e", 3), "…/c/d/e");
        assert_eq!(shorten_cwd("/a/b", 3), "/a/b");
    }

    #[test]
    fn cache_ramp_endpoints() {
        let Rgb(r, g, _) = cache_color(0.0);
        assert!(r > g);
        let Rgb(r, g, _) = cache_color(100.0);
        assert!(g > r);
    }

    #[test]
    fn reset_formatting() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-11T16:00:00Z")
            .unwrap()
            .timestamp() as f64;
        assert_eq!(
            fmt_reset("2026-09-11T18:00:00Z", now, false).unwrap(),
            "2h00m"
        );
        assert_eq!(
            fmt_reset("2026-09-11T16:30:00Z", now, false).unwrap(),
            "30m"
        );
        assert_eq!(
            fmt_reset("2026-09-11T15:00:00Z", now, false).unwrap(),
            "now"
        );
        assert!(fmt_reset("2026-09-15T00:00:00Z", now, false)
            .unwrap()
            .contains(':'));
        assert_eq!(fmt_reset("2026-09-15T00:00:00Z", now, true).unwrap(), "3d");
    }
}
