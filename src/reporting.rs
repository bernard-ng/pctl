//! Machine reports are written to stdout; child diagnostics use stderr.
use crate::{
    Result,
    execution::{ExecutionReport, TaskReport, TaskStatus},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Terminal,
    Json,
    Ndjson,
    Junit,
}

pub fn render(report: &ExecutionReport, format: Format) -> Result<String> {
    match format {
        Format::Json => serde_json::to_string_pretty(report).map_err(|error| {
            crate::error::Error::from(format!("Cannot render JSON report: {error}"))
        }),
        Format::Ndjson => {
            let mut lines = Vec::new();
            for task in &report.tasks {
                lines.push(task_event(task)?);
            }
            lines.push(run_event(report)?);
            Ok(lines.join("\n"))
        }
        Format::Junit => {
            let failures = report.tasks.iter().filter(|t| t.exit_code != 0).count();
            let mut xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"pctl\" tests=\"{}\" failures=\"{failures}\">\n",
                report.tasks.len()
            );
            for task in &report.tasks {
                xml.push_str(&format!(
                    "  <testcase name=\"{}\" time=\"{:.3}\">",
                    escape(&task.id),
                    task.duration_ms as f64 / 1000.0
                ));
                if task.status == TaskStatus::Skipped {
                    xml.push_str("<skipped/>");
                }
                if task.exit_code != 0 {
                    xml.push_str(&format!(
                        "<failure message=\"Exit code {}\">{}</failure>",
                        task.exit_code,
                        escape(task.message.as_deref().unwrap_or("Task failed"))
                    ));
                }
                xml.push_str("</testcase>\n");
            }
            xml.push_str("</testsuite>");
            Ok(xml)
        }
        Format::Terminal => Ok(report
            .tasks
            .iter()
            .map(|task| {
                format!(
                    "{}: {} ({} ms){}",
                    task.id,
                    task.status,
                    task.duration_ms,
                    task.message
                        .as_ref()
                        .map(|m| format!(" - {m}"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")),
    }
}

/// One NDJSON line for a finished (or skipped) task.
pub fn task_event(task: &TaskReport) -> Result<String> {
    Ok(serde_json::to_string(
        &serde_json::json!({"event":"task_finished","task":task}),
    )?)
}

/// The closing NDJSON line carrying the run's exit code.
pub fn run_event(report: &ExecutionReport) -> Result<String> {
    Ok(serde_json::to_string(
        &serde_json::json!({"event":"run_finished","exit_code":report.exit_code}),
    )?)
}

fn escape(input: &str) -> String {
    input
        .chars()
        .filter(|c| *c >= ' ' || matches!(c, '\n' | '\r' | '\t'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
