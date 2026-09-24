//! Machine reports are written to stdout; child diagnostics use stderr.
use crate::{Result, execution::ExecutionReport};

pub fn render(report: &ExecutionReport, format: &str) -> Result<String> {
    match format {
        "json" => serde_json::to_string_pretty(report).map_err(|error| {
            crate::error::Error::from(format!("Cannot render JSON report: {error}"))
        }),
        "ndjson" => {
            let mut lines = Vec::new();
            for task in &report.tasks {
                lines.push(
                    serde_json::to_string(
                        &serde_json::json!({"event":"task_finished","task":task}),
                    )
                    .map_err(|e| e.to_string())?,
                );
            }
            lines.push(
                serde_json::to_string(
                    &serde_json::json!({"event":"run_finished","exit_code":report.exit_code}),
                )
                .map_err(|e| e.to_string())?,
            );
            Ok(lines.join("\n"))
        }
        "junit" => {
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
                if task.status == "skipped" {
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
        "terminal" => Ok(report
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
        _ => Err("Unknown report format".into()),
    }
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
