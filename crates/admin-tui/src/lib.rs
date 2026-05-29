// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use std::env;
use std::time::Duration;

use admin_api::{
    ADMIN_API_VERSION, AdminAlertSuppressionInput, AdminApiErrorResponse,
    OPERATOR_PROVIDER_CONTRACT, ROUTE_ALERT_SUPPRESSIONS, ROUTE_REPLICATION_RETRY_JOB,
    ROUTE_STATUS, ReplicationRetryPayload, SuppressedAdminAlertRecordList,
};
use anyhow::{Context, Result, bail};
use reqwest::{
    Url,
    header::{HeaderMap, HeaderValue},
};
use serde_json::Value;

const HEADER_API_KEY: &str = "x-api-key";
const HEADER_ADMIN_API_VERSION: &str = "x-admin-api-version";
const DEFAULT_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Summary,
    Providers,
    FailedJobs {
        limit: usize,
    },
    RetryJob {
        job_id: u64,
    },
    Alerts {
        limit: usize,
    },
    SuppressAlert {
        alert_id: String,
        title: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
    pub command: Command,
}

pub fn parse_args(args: &[String]) -> Result<CliConfig> {
    let mut base_url = None;
    let mut api_key = None;
    let mut timeout_ms = None;
    let mut positionals = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        let next = args.get(i + 1).cloned();
        match arg {
            "--base-url" => {
                base_url = Some(next.context("--base-url requires value")?);
                i += 2;
            }
            "--api-key" => {
                api_key = Some(next.context("--api-key requires value")?);
                i += 2;
            }
            "--timeout-ms" => {
                timeout_ms = Some(
                    next.context("--timeout-ms requires value")?
                        .parse::<u64>()
                        .context("invalid --timeout-ms")?,
                );
                i += 2;
            }
            _ => {
                positionals.push(args[i].clone());
                i += 1;
            }
        }
    }

    let command = parse_command(&positionals)?;
    let base_url = normalize_base_url(
        &base_url
            .or_else(|| env::var("CCBG_TUI_BASE_URL").ok())
            .or_else(|| env::var("CCBG_ADMIN_BASE_URL").ok())
            .unwrap_or_else(|| "http://127.0.0.1:61081".to_string()),
    )?;
    let api_key = api_key
        .or_else(|| env::var("CCBG_TUI_API_KEY").ok())
        .or_else(|| env::var("CCBG_CONTROL_API_KEY").ok())
        .and_then(|value| normalize_api_key(&value));
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    Ok(CliConfig {
        base_url,
        api_key,
        timeout_ms,
        command,
    })
}

fn parse_command(positionals: &[String]) -> Result<Command> {
    if positionals.is_empty() {
        return Ok(Command::Summary);
    }
    match positionals[0].as_str() {
        "summary" => {
            ensure_no_extra_args(positionals, 1, "summary")?;
            Ok(Command::Summary)
        }
        "providers" => {
            ensure_no_extra_args(positionals, 1, "providers")?;
            Ok(Command::Providers)
        }
        "failed-jobs" => {
            let limit = parse_limit_flag(positionals, 10)?;
            Ok(Command::FailedJobs { limit })
        }
        "retry-job" => {
            let job_id = positionals
                .get(1)
                .context("usage: retry-job <job_id>")?
                .parse::<u64>()
                .context("job_id must be u64")?;
            ensure_no_extra_args(positionals, 2, "retry-job")?;
            Ok(Command::RetryJob { job_id })
        }
        "alerts" => {
            let limit = parse_limit_flag(positionals, 10)?;
            Ok(Command::Alerts { limit })
        }
        "suppress-alert" => {
            let alert_id = positionals
                .get(1)
                .context("usage: suppress-alert <alert_id> [--title TITLE]")?
                .clone();
            let title = parse_title_flag(positionals)?;
            Ok(Command::SuppressAlert { alert_id, title })
        }
        other => bail!("unknown command: {other}"),
    }
}

fn parse_limit_flag(positionals: &[String], default: usize) -> Result<usize> {
    let mut limit = default;
    let mut i = 1;
    while i < positionals.len() {
        match positionals[i].as_str() {
            "--limit" => {
                limit = positionals
                    .get(i + 1)
                    .context("--limit requires value")?
                    .parse::<usize>()
                    .context("invalid --limit")?
                    .max(1);
                i += 2;
            }
            other => bail!("unexpected argument for {}: {other}", positionals[0]),
        }
    }
    Ok(limit)
}

fn parse_title_flag(positionals: &[String]) -> Result<Option<String>> {
    let mut title = None;
    let mut i = 2;
    while i < positionals.len() {
        match positionals[i].as_str() {
            "--title" => {
                title = Some(
                    positionals
                        .get(i + 1)
                        .context("--title requires value")?
                        .clone(),
                );
                i += 2;
            }
            other => bail!("unexpected argument for {}: {other}", positionals[0]),
        }
    }
    Ok(title)
}

fn ensure_no_extra_args(positionals: &[String], expected_len: usize, command: &str) -> Result<()> {
    if positionals.len() > expected_len {
        bail!(
            "unexpected argument for {command}: {}",
            positionals[expected_len]
        );
    }
    Ok(())
}

fn normalize_base_url(base_url: &str) -> Result<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        bail!("base URL cannot be empty");
    }
    let parsed = Url::parse(trimmed).context("invalid base URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("base URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("base URL must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("base URL must not contain query or fragment");
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

fn normalize_api_key(api_key: &str) -> Option<String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub async fn run(config: CliConfig) -> Result<String> {
    let client = AdminClient::new(&config)?;
    match config.command {
        Command::Summary => render_summary(&client.fetch_status().await?),
        Command::Providers => render_providers(&client.fetch_status().await?),
        Command::FailedJobs { limit } => render_failed_jobs(&client.fetch_status().await?, limit),
        Command::RetryJob { job_id } => client.retry_job(job_id).await,
        Command::Alerts { limit } => render_alerts(&client.fetch_status().await?, limit),
        Command::SuppressAlert { alert_id, title } => client.suppress_alert(alert_id, title).await,
    }
}

struct AdminClient {
    base_url: String,
    http: reqwest::Client,
}

impl AdminClient {
    fn new(config: &CliConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER_ADMIN_API_VERSION,
            HeaderValue::from_static(ADMIN_API_VERSION),
        );
        if let Some(key) = &config.api_key {
            headers.insert(
                HEADER_API_KEY,
                HeaderValue::from_str(key).context("invalid api key")?,
            );
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_millis(config.timeout_ms.max(1)))
            .build()
            .context("failed to create http client")?;
        Ok(Self {
            base_url: config.base_url.clone(),
            http,
        })
    }

    async fn fetch_status(&self) -> Result<Value> {
        let url = format!("{}{}", self.base_url, ROUTE_STATUS);
        self.read_json(self.http.get(url).send().await).await
    }

    async fn retry_job(&self, job_id: u64) -> Result<String> {
        let route = ROUTE_REPLICATION_RETRY_JOB.replace("{job_id}", &job_id.to_string());
        let url = format!("{}{}", self.base_url, route);
        let payload = self
            .read_json::<ReplicationRetryPayload>(self.http.post(url).send().await)
            .await?;
        Ok(format!(
            "retried job {}: status={} target={}",
            payload.job_id, payload.status, payload.target
        ))
    }

    async fn suppress_alert(&self, alert_id: String, title: Option<String>) -> Result<String> {
        let url = format!("{}{}", self.base_url, ROUTE_ALERT_SUPPRESSIONS);
        let body = AdminAlertSuppressionInput {
            alert_id: alert_id.clone(),
            title: title.clone(),
            suppressed: true,
        };
        let payload = self
            .read_json::<SuppressedAdminAlertRecordList>(
                self.http.post(url).json(&body).send().await,
            )
            .await?;
        Ok(format!(
            "suppressed alert {alert_id} (open suppressions={})",
            payload.len()
        ))
    }

    async fn read_json<T>(
        &self,
        response: std::result::Result<reqwest::Response, reqwest::Error>,
    ) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = response.context("request failed")?;
        let status = response.status();
        let text = response
            .text()
            .await
            .context("failed to read response body")?;
        if status.is_success() {
            return serde_json::from_str(&text)
                .context("response does not match admin-api contract");
        }
        let message = parse_error_message(&text).unwrap_or_else(|| format!("http {}", status));
        bail!("{message}");
    }
}

pub fn parse_error_message(body: &str) -> Option<String> {
    if let Ok(err) = serde_json::from_str::<AdminApiErrorResponse>(body) {
        return Some(sanitize_message(&err.error));
    }
    serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .get("error")
            .and_then(Value::as_str)
            .map(sanitize_message)
    })
}

fn sanitize_message(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let sensitive = [
        "token",
        "cookie",
        "password",
        "sms",
        "captcha",
        "browser_id",
        "authorization",
        "api_key",
        "api key",
        "access_token",
        "refresh_token",
        "secret",
        "session",
    ];
    if sensitive.iter().any(|item| lower.contains(item)) {
        "request failed with sensitive detail redacted".to_string()
    } else {
        input.to_string()
    }
}

fn render_summary(status: &Value) -> Result<String> {
    let providers = provider_lines(status);
    let monitoring = &status["monitoring"]["replication"];
    let pending = monitoring["pending_jobs"].as_u64().unwrap_or(0);
    let retry = monitoring["retry_scheduled_jobs"].as_u64().unwrap_or(0);
    let failed = monitoring["latest_failed_objects"].as_u64().unwrap_or(0);
    let open_alerts = status["alerts"].as_array().map_or(0, Vec::len);
    let failed_jobs = failed_jobs(status, 3);
    let mut lines = vec![
        format!("providers: {}", providers.join(" | ")),
        format!("replication: pending={pending} retry_scheduled={retry} latest_failed={failed}"),
        format!("open_alerts: {open_alerts}"),
        "failed_jobs_sample:".to_string(),
    ];
    if failed_jobs.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(failed_jobs);
    }
    Ok(lines.join("\n"))
}

fn render_providers(status: &Value) -> Result<String> {
    let lines = provider_lines(status);
    Ok(lines.join("\n"))
}

fn render_failed_jobs(status: &Value, limit: usize) -> Result<String> {
    let lines = failed_jobs(status, limit);
    if lines.is_empty() {
        return Ok("no failed jobs".to_string());
    }
    Ok(lines.join("\n"))
}

fn render_alerts(status: &Value, limit: usize) -> Result<String> {
    let mut lines = Vec::new();
    if let Some(alerts) = status.get("alerts").and_then(Value::as_array) {
        for alert in alerts.iter().take(limit) {
            let id = alert.get("id").and_then(Value::as_str).unwrap_or("-");
            let severity = alert.get("severity").and_then(Value::as_str).unwrap_or("-");
            let title = alert.get("title").and_then(Value::as_str).unwrap_or("-");
            lines.push(format!("{id} [{severity}] {title}"));
        }
    }
    if lines.is_empty() {
        Ok("no open alerts".to_string())
    } else {
        Ok(lines.join("\n"))
    }
}

fn provider_lines(status: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(items) = status.get("provider_health").and_then(Value::as_array) {
        for item in items {
            let provider = item.get("provider").and_then(Value::as_str).unwrap_or("-");
            if !OPERATOR_PROVIDER_CONTRACT.contains(&provider) {
                continue;
            }
            let health = item
                .get("health")
                .and_then(|health| health.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            lines.push(format!("{provider}: {health}"));
        }
    }
    lines
}

fn failed_jobs(status: &Value, limit: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(jobs) = status
        .get("replication_state")
        .and_then(|value| value.get("latest_failed_jobs"))
        .and_then(Value::as_array)
    {
        for job in jobs.iter().take(limit) {
            let job_id = job.get("job_id").and_then(Value::as_u64).unwrap_or(0);
            let target = job.get("target").and_then(Value::as_str).unwrap_or("-");
            let bucket = job
                .get("object")
                .and_then(|v| v.get("bucket"))
                .and_then(Value::as_str)
                .unwrap_or("-");
            let key = job
                .get("object")
                .and_then(|v| v.get("key"))
                .and_then(Value::as_str)
                .unwrap_or("-");
            lines.push(format!("- job_id={job_id} target={target} {bucket}/{key}"));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
    use serde_json::json;
    use std::{net::SocketAddr, sync::Arc};
    use tokio::net::TcpListener;

    #[test]
    fn summary_and_provider_filter_hide_onedrive() {
        let status = json!({
            "provider_health": [
                {"provider":"unicom","health":{"status":"healthy"}},
                {"provider":"onedrive","health":{"status":"healthy"}}
            ],
            "monitoring":{"replication":{"pending_jobs":1,"retry_scheduled_jobs":2,"latest_failed_objects":3}},
            "alerts": [{"id":"a"}],
            "replication_state":{"latest_failed_jobs":[{"job_id":7,"target":"telecom","object":{"bucket":"root","key":"x"}}]}
        });
        let providers = render_providers(&status).expect("providers");
        assert!(providers.contains("unicom"));
        assert!(!providers.contains("onedrive"));
        let summary = render_summary(&status).expect("summary");
        assert!(summary.contains("open_alerts: 1"));
    }

    #[test]
    fn parse_error_message_redacts_sensitive_fields() {
        let body = r#"{"error":"invalid token and password"}"#;
        let message = parse_error_message(body).expect("message");
        assert!(message.contains("redacted"));
    }

    #[test]
    fn parse_args_normalizes_base_url_and_rejects_unsafe_inputs() {
        let args = vec![
            "admin-tui".to_string(),
            "--base-url".to_string(),
            " http://localhost:61081/admin/ ".to_string(),
            "--api-key".to_string(),
            " key ".to_string(),
            "failed-jobs".to_string(),
            "--limit".to_string(),
            "0".to_string(),
        ];
        let config = parse_args(&args).expect("config should parse");
        assert_eq!(config.base_url, "http://localhost:61081/admin");
        assert_eq!(config.api_key.as_deref(), Some("key"));
        assert_eq!(config.command, Command::FailedJobs { limit: 1 });

        let credential_url = vec![
            "admin-tui".to_string(),
            "--base-url".to_string(),
            "http://user:pass@localhost:61081".to_string(),
        ];
        assert!(parse_args(&credential_url).is_err());

        let extra_arg = vec![
            "admin-tui".to_string(),
            "retry-job".to_string(),
            "7".to_string(),
            "--limit".to_string(),
            "1".to_string(),
        ];
        assert!(parse_args(&extra_arg).is_err());
    }

    #[tokio::test]
    async fn client_uses_headers_and_supports_retry_and_suppress() {
        #[derive(Clone)]
        struct TestState(Arc<std::sync::Mutex<Vec<String>>>);
        async fn status(headers: HeaderMap) -> Json<Value> {
            assert_eq!(
                headers
                    .get(HEADER_ADMIN_API_VERSION)
                    .and_then(|v| v.to_str().ok()),
                Some(ADMIN_API_VERSION)
            );
            Json(json!({
                "provider_health":[{"provider":"mobile","health":{"status":"degraded"}}],
                "monitoring":{"replication":{"pending_jobs":0,"retry_scheduled_jobs":0,"latest_failed_objects":0}},
                "alerts":[],
                "replication_state":{"latest_failed_jobs":[]}
            }))
        }
        async fn retry(headers: HeaderMap) -> Json<Value> {
            assert_eq!(
                headers.get(HEADER_API_KEY).and_then(|v| v.to_str().ok()),
                Some("k")
            );
            Json(json!({"job_id":1,"status":"pending","target":"mobile","bucket":"root","key":"a"}))
        }
        async fn suppress(
            State(state): State<TestState>,
            headers: HeaderMap,
            Json(input): Json<AdminAlertSuppressionInput>,
        ) -> Json<Value> {
            assert_eq!(
                headers
                    .get(HEADER_ADMIN_API_VERSION)
                    .and_then(|v| v.to_str().ok()),
                Some(ADMIN_API_VERSION)
            );
            assert_eq!(
                headers.get(HEADER_API_KEY).and_then(|v| v.to_str().ok()),
                Some("k")
            );
            assert_eq!(input.title.as_deref(), Some("T"));
            state.0.lock().expect("lock").push(input.alert_id);
            Json(json!([{"id":"x","title":"t","closed_at_unix_ms":1,"delete_after_unix_ms":2}]))
        }
        let state = TestState(Arc::new(std::sync::Mutex::new(Vec::new())));
        let app = Router::new()
            .route(ROUTE_STATUS, get(status))
            .route(ROUTE_REPLICATION_RETRY_JOB, axum::routing::post(retry))
            .route(ROUTE_ALERT_SUPPRESSIONS, axum::routing::post(suppress))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let config = CliConfig {
            base_url: format!("http://{}", addr),
            api_key: Some("k".to_string()),
            timeout_ms: 10_000,
            command: Command::Summary,
        };
        let summary = run(config.clone()).await.expect("summary");
        assert!(summary.contains("mobile: degraded"));
        let retried = run(CliConfig {
            command: Command::RetryJob { job_id: 1 },
            ..config.clone()
        })
        .await
        .expect("retry");
        assert!(retried.contains("retried job 1"));
        let suppressed = run(CliConfig {
            command: Command::SuppressAlert {
                alert_id: "x".to_string(),
                title: Some("T".to_string()),
            },
            ..config
        })
        .await
        .expect("suppress");
        assert!(suppressed.contains("suppressed alert x"));
    }
}
