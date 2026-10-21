//! `clankmon export`: requests or daily totals as JSON lines or CSV.

use std::io::Write;
use std::sync::Arc;

use chrono::{Duration, Utc};
use color_eyre::eyre::{Result, bail};

use crate::collect::jsonl::Tailer;
use crate::config::Paths;
use crate::model::AppEvent;
use crate::pricing::Pricing;
use crate::state::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Jsonl,
    Csv,
    Daily,
}

/// "7d", "12h", "30m" -> Duration.
pub fn parse_since(s: &str) -> Result<Duration> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num.parse().map_err(|_| color_eyre::eyre::eyre!("bad duration `{s}`"))?;
    Ok(match unit {
        "d" | "" => Duration::days(n),
        "h" => Duration::hours(n),
        "m" => Duration::minutes(n),
        "w" => Duration::weeks(n),
        _ => bail!("bad duration unit in `{s}` (use m, h, d or w)"),
    })
}

/// Load transcripts synchronously into a fresh state.
pub fn load(paths: &Paths, days: u32, pricing: Arc<Pricing>) -> State {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tailer = Tailer::new(paths.projects(), days, pricing.clone(), tx);
    let handle = std::thread::spawn(move || tailer.scan_only());
    let mut st = State::new(pricing);
    while let Some(ev) = rx.blocking_recv() {
        let done = matches!(ev, AppEvent::ScanDone { .. });
        st.apply(ev);
        if done {
            break;
        }
    }
    let _ = handle.join();
    st.ensure_sorted();
    crate::mem::release();
    st
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

pub fn run(paths: &Paths, pricing: Arc<Pricing>, format: Format, since: &str, output: Option<&std::path::Path>) -> Result<()> {
    let span = parse_since(since)?;
    let days = (span.num_days() + 1).clamp(1, 3650) as u32;
    let st = load(paths, days, pricing);
    let cutoff = Utc::now() - span;
    let mut out: Box<dyn Write> = match output {
        Some(p) => Box::new(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => Box::new(std::io::BufWriter::new(std::io::stdout().lock())),
    };
    match format {
        Format::Jsonl => {
            for r in st.timeline().filter(|r| r.ts >= cutoff) {
                writeln!(out, "{}", serde_json::to_string(r)?)?;
            }
        }
        Format::Csv => {
            writeln!(out, "ts,request_id,session_id,project,model,input,cache_read,cache_write_5m,cache_write_1h,output,thinking,cost_usd,stop_reason,subagent,tools")?;
            for r in st.timeline().filter(|r| r.ts >= cutoff) {
                let project = r.session_id.as_ref().and_then(|s| st.sessions.get(s)).and_then(|s| s.cwd.clone()).unwrap_or_default();
                writeln!(
                    out,
                    "{},{},{},{},{},{},{},{},{},{},{},{:.6},{},{},{}",
                    r.ts.to_rfc3339(),
                    csv_field(&r.key),
                    csv_field(r.session_id.as_deref().unwrap_or("")),
                    csv_field(&project),
                    csv_field(&r.model),
                    r.usage.input,
                    r.usage.cache_read,
                    r.usage.cache_write_5m,
                    r.usage.cache_write_1h,
                    r.usage.output,
                    r.usage.thinking,
                    r.cost_usd,
                    csv_field(r.stop_reason.as_deref().unwrap_or("")),
                    r.sidechain,
                    csv_field(&r.tools.join(" ")),
                )?;
            }
        }
        Format::Daily => {
            writeln!(out, "date,requests,errors,input,cache_read,cache_write,output,cost_usd,top_model")?;
            let first = cutoff.with_timezone(&chrono::Local).date_naive();
            for (d, a) in st.days.range(first..) {
                let top = a.by_model.iter().max_by(|x, y| x.1.1.total_cmp(&y.1.1)).map(|x| x.0.clone()).unwrap_or_default();
                writeln!(
                    out,
                    "{d},{},{},{},{},{},{},{:.4},{}",
                    a.requests,
                    a.errors,
                    a.usage.input,
                    a.usage.cache_read,
                    a.usage.cache_write(),
                    a.usage.output,
                    a.cost,
                    csv_field(&top)
                )?;
            }
        }
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_since("7d").unwrap(), Duration::days(7));
        assert_eq!(parse_since("12h").unwrap(), Duration::hours(12));
        assert_eq!(parse_since("2w").unwrap(), Duration::weeks(2));
        assert_eq!(parse_since("3").unwrap(), Duration::days(3));
        assert!(parse_since("x").is_err());
        assert!(parse_since("5y").is_err());
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("plain"), "plain");
    }
}
