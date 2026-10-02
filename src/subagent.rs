//! Claude's subagentStatusLine protocol is separate from the main statusLine:
//! one tasks array arrives on stdin and one JSON object is emitted per row.
//! Rows degrade by dropping fields, never by cutting the context share off.
use crate::{
    appearance::{self, Models, Palette},
    config::{AnsiColor, Config},
    payload::{self, Payload},
    render, session, tps,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Shares Claude's 5 s tick with transcript accounting; never block a row.
const DEADLINE: Duration = Duration::from_millis(300);

struct Row {
    name: String,
    model: Option<String>,
    status: Option<String>,
    running: bool,
    tps: Option<tps::Estimate>,
    ctx: Option<(u64, u64)>,
}

struct Style {
    palette: Palette,
    color: bool,
    now: f64,
}

impl Style {
    fn paint(&self, text: &str, token: &str, dim: bool) -> String {
        if !self.color || text.is_empty() {
            return text.into();
        }
        let mut codes: Vec<String> = AnsiColor::Named(token.into())
            .sgr(&self.palette, false)
            .into_iter()
            .collect();
        if dim {
            codes.push("2".into());
        }
        format!("\x1b[{}m{text}\x1b[22;39m", codes.join(";"))
    }

    fn ctx(&self, used: u64, max: u64, tokens: bool) -> String {
        let ratio = (used as f64 / max as f64).min(1.0);
        let token = if ratio >= 0.85 {
            "error"
        } else if ratio >= 0.6 {
            "warning"
        } else {
            "success"
        };
        let mut text = format!("{:.0}%", ratio * 100.0);
        if tokens {
            text += &format!(
                " ({}/{})",
                render::fmt_tokens(used),
                render::fmt_tokens(max)
            );
        }
        self.paint(&text, token, false)
    }
}

impl Row {
    /// Candidate layouts, most detailed first. Context outlives every other
    /// field; the name is cut only in the last layout.
    fn layouts(&self, s: &Style) -> Vec<String> {
        let sep = s.paint(" · ", "text_muted", false);
        let label = "ctx ";
        let tps = self.tps.filter(|_| self.running).map(|e| {
            let idle = s.now - e.measured_at > 60.0;
            s.paint(&format!("≈{:.0} t/s", e.tokens_per_sec), "accent", idle)
        });
        let ctx = |tokens: bool, labeled: bool| match self.ctx {
            Some((used, max)) => {
                (if labeled { label } else { "" }).to_string() + &s.ctx(used, max, tokens)
            }
            None => s.paint(
                &format!("{}?", if labeled { label } else { "" }),
                "text_muted",
                false,
            ),
        };
        let name = s.paint(&self.name, "text", false);
        let model = self.model.as_deref().map(|m| s.paint(m, "primary", false));
        let status = self
            .status
            .as_deref()
            .map(|st| s.paint(st, "text_muted", !self.running));
        let join =
            |parts: Vec<Option<String>>| parts.into_iter().flatten().collect::<Vec<_>>().join(&sep);
        vec![
            join(vec![
                Some(name.clone()),
                model.clone(),
                status.clone(),
                tps.clone(),
                Some(ctx(true, true)),
            ]),
            join(vec![
                Some(name.clone()),
                model.clone(),
                tps.clone(),
                Some(ctx(false, true)),
            ]),
            join(vec![
                Some(name.clone()),
                model,
                tps.clone(),
                Some(ctx(false, false)),
            ]),
            join(vec![Some(name.clone()), tps, Some(ctx(false, false))]),
            format!("{name} {}", ctx(false, false)),
        ]
    }

    /// The share with a name shortened in front of it, for the narrowest panel.
    fn squeeze(&self, s: &Style, width: usize) -> String {
        if width == 0 {
            return String::new();
        }
        let ctx = match self.ctx {
            Some((used, max)) => s.ctx(used, max, false),
            None => s.paint("?", "text_muted", false),
        };
        let room = width.saturating_sub(render::visible_width(&ctx) + 1);
        if room >= 2 {
            let name = render::truncate(&self.name, room, false);
            return format!("{} {ctx}", s.paint(&name, "text", false));
        }
        render::truncate(&ctx, width, s.color)
    }
}

pub fn render_rows(bytes: &[u8], config: &Config, width: Option<usize>) -> Vec<String> {
    let Ok(input) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    let Some(tasks) = input.get("tasks").and_then(Value::as_array) else {
        return Vec::new();
    };
    let width = width
        .or_else(|| {
            input
                .get("columns")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
        })
        .or_else(|| (config.style.width > 0).then_some(config.style.width))
        .unwrap_or(80);
    let now = crate::paths::now_secs();
    // The hook input carries the parent session; agent logs live beside it.
    let session = Payload {
        session_id: payload::label(
            input
                .get("session_id")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        transcript_path: input
            .get("transcript_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        ..Default::default()
    };
    let stats = session::collect(&session, Instant::now() + DEADLINE, now);
    let models = Models::load();
    let style = Style {
        palette: appearance::palette(
            (!config.style.palette.is_empty()).then_some(config.style.palette.as_str()),
        ),
        color: std::env::var_os("CC_STATUSLINE_NO_COLOR").is_none()
            && std::env::var_os("NO_COLOR").is_none(),
        now,
    };
    let rows: Vec<(&str, Row)> = tasks
        .iter()
        .filter_map(|task| {
            let id = task.get("id")?.as_str()?;
            let field = |name| {
                task.get(name)
                    .and_then(Value::as_str)
                    .map(payload::label)
                    .filter(|s| !s.is_empty())
            };
            let status = field("status");
            let row = Row {
                name: field("name")
                    .or_else(|| field("label"))
                    .unwrap_or_else(|| payload::label(id)),
                model: field("model").map(|m| payload::label(&models.display(&m))),
                running: status.as_deref().is_none_or(|s| s == "running"),
                status,
                tps: stats.as_ref().and_then(|s| s.agent_tps.get(id)).copied(),
                ctx: task.get("tokenCount").and_then(Value::as_u64).zip(
                    task.get("contextWindowSize")
                        .and_then(Value::as_u64)
                        .filter(|n| *n > 0),
                ),
            };
            Some((id, row))
        })
        .collect();
    let layouts: Vec<_> = rows.iter().map(|(_, row)| row.layouts(&style)).collect();
    // One level for the whole panel keeps the columns' meaning aligned.
    let level = (0..layouts.first().map_or(0, Vec::len)).find(|&i| {
        layouts
            .iter()
            .all(|l| render::visible_width(&l[i]) <= width)
    });
    rows.iter()
        .zip(layouts)
        .map(|((id, row), layouts)| {
            let content = match level {
                Some(i) if width > 0 => layouts[i].clone(),
                _ if layouts
                    .last()
                    .is_some_and(|l| render::visible_width(l) <= width) =>
                {
                    layouts.last().unwrap().clone()
                }
                _ => row.squeeze(&style, width),
            };
            json!({"id":id,"content":content}).to_string()
        })
        .collect()
}
