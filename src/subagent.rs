//! Claude's subagentStatusLine protocol is separate from the main statusLine:
//! one tasks array arrives on stdin and one JSON object is emitted per row.
use crate::{
    appearance::Models,
    config::{Config, Lang},
    payload, render,
};
use serde_json::{json, Value};

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
    let models = Models::load();
    let zh = config.style.lang == Lang::Zh;
    tasks
        .iter()
        .filter_map(|task| {
            let id = task.get("id")?.as_str()?;
            let field = |name| {
                task.get(name)
                    .and_then(Value::as_str)
                    .map(payload::label)
                    .filter(|s| !s.is_empty())
            };
            let name = field("name")
                .or_else(|| field("label"))
                .unwrap_or_else(|| payload::label(id));
            let mut parts = vec![name];
            if let Some(model) = field("model") {
                parts.push(payload::label(&models.display(&model)));
            }
            if let Some(status) = field("status") {
                parts.push(status);
            }
            let tokens = task.get("tokenCount").and_then(Value::as_u64);
            let capacity = task
                .get("contextWindowSize")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0);
            let label = if zh { "上下文" } else { "ctx" };
            parts.push(match (tokens, capacity) {
                (Some(used), Some(max)) => format!(
                    "{label} {:.0}% ({}/{})",
                    (used as f64 / max as f64 * 100.0).min(100.0),
                    render::fmt_tokens(used),
                    render::fmt_tokens(max)
                ),
                _ => format!("{label} ?"),
            });
            let content = parts.join(" · ");
            let content = if width == 0 {
                String::new()
            } else if render::visible_width(&content) > width {
                render::truncate(&content, width, false)
            } else {
                content
            };
            Some(json!({"id":id,"content":content}).to_string())
        })
        .collect()
}
