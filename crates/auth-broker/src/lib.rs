// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_MANUAL_INPUT_PROMPT_TTL_MS: u64 = 10 * 60 * 1_000;
const DEFAULT_MANUAL_INPUT_MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProvider {
    Unicom,
    Telecom,
    Mobile,
}

impl AuthProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unicom => "unicom",
            Self::Telecom => "telecom",
            Self::Mobile => "mobile",
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unsupported auth provider: {value}")]
pub struct AuthProviderParseError {
    value: String,
}

impl FromStr for AuthProvider {
    type Err = AuthProviderParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "unicom" => Ok(Self::Unicom),
            "telecom" => Ok(Self::Telecom),
            "mobile" => Ok(Self::Mobile),
            value => Err(AuthProviderParseError {
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManualInputFieldKind {
    Text,
    PhoneNumber,
    SmsCode,
    Password,
    Captcha,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManualInputPromptCreateInput {
    pub provider: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub flow_id: Option<String>,
    #[serde(default)]
    pub input_id: Option<String>,
    pub title: String,
    pub message: String,
    pub field_label: String,
    pub field_kind: ManualInputFieldKind,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManualInputReplyInput {
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManualInputPromptStatus {
    Pending,
    Answered,
    Expired,
    Canceled,
}

impl ManualInputPromptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Answered => "answered",
            Self::Expired => "expired",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ManualInputPromptError {
    #[error("manual input prompt is not pending: {0}")]
    NotPending(String),
    #[error("manual input prompt has expired")]
    Expired,
    #[error("manual input prompt has been canceled")]
    Canceled,
    #[error("manual input prompt has already been answered")]
    Answered,
    #[error("manual input prompt answer is empty")]
    EmptyAnswer,
    #[error("manual input prompt has reached max attempts")]
    MaxAttemptsReached,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualInputPrompt {
    pub prompt_id: String,
    pub provider: String,
    pub session_id: Option<String>,
    pub flow_id: Option<String>,
    pub input_id: Option<String>,
    pub title: String,
    pub message: String,
    pub field_label: String,
    pub field_kind: ManualInputFieldKind,
    pub placeholder: Option<String>,
    pub status: ManualInputPromptStatus,
    pub attempt_count: u32,
    pub retry_count: u32,
    pub max_attempts: u32,
    pub max_retries: u32,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub answered_at_unix_ms: Option<u64>,
    pub canceled_at_unix_ms: Option<u64>,
    pub expired_at_unix_ms: Option<u64>,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub answered_by: Option<String>,
    pub canceled_by: Option<String>,
    pub last_transition_reason: Option<String>,
    pub answer_present: bool,
    pub answer_value: Option<String>,
}

impl ManualInputPrompt {
    pub fn from_input(input: ManualInputPromptCreateInput) -> Self {
        let now = current_unix_ms();
        Self {
            prompt_id: random_urlsafe_token(16),
            provider: normalize_field(Some(input.provider)).unwrap_or_default(),
            session_id: normalize_field(input.session_id),
            flow_id: normalize_field(input.flow_id),
            input_id: normalize_field(input.input_id),
            title: normalize_field(Some(input.title)).unwrap_or_default(),
            message: normalize_field(Some(input.message)).unwrap_or_default(),
            field_label: normalize_field(Some(input.field_label)).unwrap_or_default(),
            field_kind: input.field_kind,
            placeholder: normalize_field(input.placeholder),
            status: ManualInputPromptStatus::Pending,
            attempt_count: 0,
            retry_count: 0,
            max_attempts: DEFAULT_MANUAL_INPUT_MAX_ATTEMPTS,
            max_retries: DEFAULT_MANUAL_INPUT_MAX_ATTEMPTS,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            expires_at_unix_ms: now.saturating_add(DEFAULT_MANUAL_INPUT_PROMPT_TTL_MS),
            answered_at_unix_ms: None,
            canceled_at_unix_ms: None,
            expired_at_unix_ms: None,
            created_by: Some("system".to_string()),
            updated_by: Some("system".to_string()),
            answered_by: None,
            canceled_by: None,
            last_transition_reason: Some("prompt_created".to_string()),
            answer_present: false,
            answer_value: None,
        }
    }

    pub fn answer(
        &mut self,
        value: String,
        answered_by: Option<String>,
        reason: Option<String>,
    ) -> Result<(), ManualInputPromptError> {
        self.expire_if_needed();
        match self.status {
            ManualInputPromptStatus::Pending => {}
            ManualInputPromptStatus::Answered => return Err(ManualInputPromptError::Answered),
            ManualInputPromptStatus::Expired => return Err(ManualInputPromptError::Expired),
            ManualInputPromptStatus::Canceled => return Err(ManualInputPromptError::Canceled),
        }
        if self.attempt_count >= self.max_attempts {
            return Err(ManualInputPromptError::MaxAttemptsReached);
        }
        let normalized = value.trim();
        if normalized.is_empty() {
            return Err(ManualInputPromptError::EmptyAnswer);
        }
        self.attempt_count = self.attempt_count.saturating_add(1);
        self.retry_count = self.attempt_count.saturating_sub(1);
        self.answer_value = Some(normalized.to_string());
        self.answer_present = true;
        self.status = ManualInputPromptStatus::Answered;
        let now = current_unix_ms();
        self.answered_at_unix_ms = Some(now);
        self.updated_at_unix_ms = now;
        self.answered_by = normalize_field(answered_by);
        self.updated_by = self.answered_by.clone();
        self.last_transition_reason =
            normalize_field(reason).or(Some("prompt_answered".to_string()));
        Ok(())
    }

    pub fn cancel(
        &mut self,
        canceled_by: Option<String>,
        reason: Option<String>,
    ) -> Result<(), ManualInputPromptError> {
        self.expire_if_needed();
        match self.status {
            ManualInputPromptStatus::Pending => {}
            ManualInputPromptStatus::Answered => return Err(ManualInputPromptError::Answered),
            ManualInputPromptStatus::Expired => return Err(ManualInputPromptError::Expired),
            ManualInputPromptStatus::Canceled => return Err(ManualInputPromptError::Canceled),
        }
        self.status = ManualInputPromptStatus::Canceled;
        let now = current_unix_ms();
        self.canceled_at_unix_ms = Some(now);
        self.updated_at_unix_ms = now;
        self.canceled_by = normalize_field(canceled_by);
        self.updated_by = self.canceled_by.clone();
        self.last_transition_reason =
            normalize_field(reason).or(Some("prompt_canceled".to_string()));
        Ok(())
    }

    pub fn expire_if_needed(&mut self) -> bool {
        if self.status != ManualInputPromptStatus::Pending {
            return false;
        }
        let now = current_unix_ms();
        if now <= self.expires_at_unix_ms {
            return false;
        }
        self.status = ManualInputPromptStatus::Expired;
        self.expired_at_unix_ms = Some(now);
        self.updated_at_unix_ms = now;
        self.updated_by = Some("system".to_string());
        self.last_transition_reason = Some("prompt_expired".to_string());
        true
    }

    pub fn sanitized(&self) -> Self {
        let mut copy = self.clone();
        copy.answer_value = None;
        copy
    }
}

#[derive(Debug, Clone)]
pub struct AuthSession<R, M> {
    pub session_id: String,
    pub provider: String,
    pub surface: String,
    pub flow_id: String,
    pub status: String,
    pub inputs: BTreeMap<String, serde_json::Value>,
    pub runtime: BTreeMap<String, serde_json::Value>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub report: Option<R>,
    pub last_error: Option<String>,
    pub cdp_endpoint_url: Option<String>,
    pub cdp_target_selector: Option<String>,
    pub cdp_target_timeout_ms: Option<u64>,
    pub manual_challenge: Option<M>,
}

impl<R, M> AuthSession<R, M> {
    pub fn new(
        session_id: String,
        provider: String,
        surface: String,
        flow_id: String,
        inputs: BTreeMap<String, serde_json::Value>,
        runtime: BTreeMap<String, serde_json::Value>,
    ) -> Self {
        let now = current_unix_ms();
        Self {
            session_id,
            provider,
            surface,
            flow_id,
            status: "pending".to_string(),
            inputs,
            runtime,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            report: None,
            last_error: None,
            cdp_endpoint_url: None,
            cdp_target_selector: None,
            cdp_target_timeout_ms: None,
            manual_challenge: None,
        }
    }

    pub fn apply_request(
        &mut self,
        provider: String,
        surface: String,
        flow_id: String,
        inputs: BTreeMap<String, serde_json::Value>,
        runtime: BTreeMap<String, serde_json::Value>,
    ) {
        self.provider = provider;
        self.surface = surface;
        self.flow_id = flow_id;
        self.inputs.extend(inputs);
        self.runtime.extend(runtime);
        self.updated_at_unix_ms = current_unix_ms();
        if self.status == "completed" || self.status == "failed" {
            self.status = "pending".to_string();
            self.report = None;
            self.last_error = None;
        }
        self.manual_challenge = None;
    }

    pub fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
        self.updated_at_unix_ms = current_unix_ms();
    }

    pub fn set_completed(&mut self, report: R) {
        self.status = "completed".to_string();
        self.report = Some(report);
        self.last_error = None;
        self.manual_challenge = None;
        self.updated_at_unix_ms = current_unix_ms();
    }

    pub fn set_failed(&mut self, error: impl std::fmt::Display) {
        self.status = "failed".to_string();
        self.last_error = Some(error.to_string());
        self.updated_at_unix_ms = current_unix_ms();
    }
}

pub type ManualInputStore = Arc<Mutex<HashMap<String, ManualInputPrompt>>>;
pub type AuthSessionStore<R, M> = Arc<Mutex<HashMap<String, AuthSession<R, M>>>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenRefreshStatus {
    Unsupported,
    Stable,
    Refreshed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRefreshRequest {
    pub provider: AuthProvider,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRefreshOutcome {
    pub status: TokenRefreshStatus,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

impl TokenRefreshOutcome {
    pub fn unsupported(provider: AuthProvider) -> Self {
        Self {
            status: TokenRefreshStatus::Unsupported,
            message: Some(format!(
                "{} does not support token refresh via auth-broker",
                provider.as_str()
            )),
            access_token: None,
            refresh_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ManualInputEvent {
    PromptCreated {
        prompt_id: String,
        provider: String,
        session_id: Option<String>,
        flow_id: Option<String>,
    },
    PromptAnswered {
        prompt_id: String,
        provider: String,
        session_id: Option<String>,
        flow_id: Option<String>,
    },
    PromptCanceled {
        prompt_id: String,
        provider: String,
        session_id: Option<String>,
        flow_id: Option<String>,
    },
    PromptExpired {
        prompt_id: String,
        provider: String,
        session_id: Option<String>,
        flow_id: Option<String>,
    },
}

pub trait ManualInputCallback: Send + Sync {
    fn on_manual_input_event(&self, event: &ManualInputEvent);
}

fn normalize_field(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn random_urlsafe_token(len_bytes: usize) -> String {
    let mut bytes = vec![0_u8; len_bytes];
    OsRng.fill_bytes(&mut bytes);
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    bytes
        .into_iter()
        .flat_map(|byte| {
            [
                TABLE[(byte >> 2) as usize],
                TABLE[((byte & 0x03) << 4) as usize],
            ]
        })
        .take(len_bytes)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn providers_parse_and_validate() {
        assert_eq!(
            "unicom".parse::<AuthProvider>().ok(),
            Some(AuthProvider::Unicom)
        );
        assert_eq!(
            "TELECOM".parse::<AuthProvider>().ok(),
            Some(AuthProvider::Telecom)
        );
        assert_eq!(
            "mobile".parse::<AuthProvider>().ok(),
            Some(AuthProvider::Mobile)
        );
        assert!("onedrive".parse::<AuthProvider>().is_err());
    }

    #[test]
    fn prompt_lifecycle_and_sanitized_hides_answer() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: Some("auth-session-1".to_string()),
            flow_id: Some("unicom_sms_login".to_string()),
            input_id: Some("sms_code".to_string()),
            title: "验证码".to_string(),
            message: "请输入短信验证码".to_string(),
            field_label: "短信验证码".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: Some("123456".to_string()),
        });
        prompt
            .answer(
                "654321".to_string(),
                Some("operator-1".to_string()),
                Some("manual_reply".to_string()),
            )
            .expect("answer should succeed");
        let sanitized = prompt.sanitized();
        assert!(sanitized.answer_present);
        assert_eq!(sanitized.answer_value, None);
        assert_eq!(prompt.answer_value.as_deref(), Some("654321"));
        assert_eq!(prompt.status, ManualInputPromptStatus::Answered);
        assert_eq!(prompt.attempt_count, 1);
    }

    #[test]
    fn prompt_expiration_and_cancel_reject_future_answers() {
        let mut expired = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: Some("auth-session-expired".to_string()),
            flow_id: Some("unicom_sms_login".to_string()),
            input_id: Some("sms_code".to_string()),
            title: "验证码".to_string(),
            message: "请输入短信验证码".to_string(),
            field_label: "短信验证码".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: Some("123456".to_string()),
        });
        expired.expires_at_unix_ms = current_unix_ms().saturating_sub(1);
        assert!(expired.expire_if_needed());
        assert_eq!(expired.status, ManualInputPromptStatus::Expired);
        assert_eq!(
            expired.answer("999999".to_string(), None, None),
            Err(ManualInputPromptError::Expired)
        );

        let mut canceled = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "telecom".to_string(),
            session_id: Some("auth-session-canceled".to_string()),
            flow_id: Some("telecom_sms_login".to_string()),
            input_id: Some("sms_code".to_string()),
            title: "验证码".to_string(),
            message: "请输入短信验证码".to_string(),
            field_label: "短信验证码".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: Some("123456".to_string()),
        });
        canceled
            .cancel(
                Some("operator-2".to_string()),
                Some("manual_abort".to_string()),
            )
            .expect("cancel should succeed");
        assert_eq!(canceled.status, ManualInputPromptStatus::Canceled);
        assert_eq!(
            canceled.answer("888888".to_string(), None, None),
            Err(ManualInputPromptError::Canceled)
        );
    }

    #[test]
    fn duplicate_answer_is_rejected() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "mobile".to_string(),
            session_id: Some("auth-session-dup".to_string()),
            flow_id: Some("mobile_sms_login".to_string()),
            input_id: Some("sms_code".to_string()),
            title: "验证码".to_string(),
            message: "请输入短信验证码".to_string(),
            field_label: "短信验证码".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: Some("123456".to_string()),
        });
        prompt
            .answer("654321".to_string(), Some("operator-3".to_string()), None)
            .expect("first answer should succeed");
        assert_eq!(
            prompt.answer("123123".to_string(), Some("operator-3".to_string()), None),
            Err(ManualInputPromptError::Answered)
        );
    }

    #[test]
    fn carrier_sessions_apply_prompt_reply_and_complete() {
        for provider in ["unicom", "telecom", "mobile"] {
            let mut session: AuthSession<serde_json::Value, serde_json::Value> = AuthSession::new(
                format!("{provider}-session"),
                provider.to_string(),
                "carrier-web".to_string(),
                format!("{provider}_sms_login"),
                BTreeMap::new(),
                BTreeMap::new(),
            );
            session.set_status("awaiting_input");
            assert_eq!(session.status, "awaiting_input");
            session.apply_request(
                provider.to_string(),
                "carrier-web".to_string(),
                format!("{provider}_sms_login"),
                BTreeMap::from([("sms_code".to_string(), json!("123456"))]),
                BTreeMap::new(),
            );
            assert_eq!(session.inputs.get("sms_code"), Some(&json!("123456")));
            session.set_completed(json!({"ok": true}));
            assert_eq!(session.status, "completed");
        }
    }

    #[test]
    fn token_refresh_contract_reports_unsupported_for_carriers() {
        for provider in [
            AuthProvider::Unicom,
            AuthProvider::Telecom,
            AuthProvider::Mobile,
        ] {
            let outcome = TokenRefreshOutcome::unsupported(provider);
            assert_eq!(outcome.status, TokenRefreshStatus::Unsupported);
            assert!(
                outcome
                    .message
                    .unwrap_or_default()
                    .contains(provider.as_str())
            );
        }
    }

    // === Pure utility function unit tests ===

    #[test]
    fn normalize_field_none_returns_none() {
        assert_eq!(normalize_field(None), None);
    }

    #[test]
    fn normalize_field_empty_string_returns_none() {
        assert_eq!(normalize_field(Some("".to_string())), None);
    }

    #[test]
    fn normalize_field_whitespace_only_returns_none() {
        assert_eq!(normalize_field(Some("   ".to_string())), None);
    }

    #[test]
    fn normalize_field_valid_string_trimmed() {
        assert_eq!(
            normalize_field(Some("  hello  ".to_string())),
            Some("hello".to_string())
        );
    }

    #[test]
    fn normalize_field_preserves_inner_whitespace() {
        assert_eq!(
            normalize_field(Some("a  b".to_string())),
            Some("a  b".to_string())
        );
    }

    #[test]
    fn random_urlsafe_token_has_correct_length() {
        let token = random_urlsafe_token(16);
        assert_eq!(token.len(), 16);
    }

    #[test]
    fn random_urlsafe_token_contains_only_valid_chars() {
        let token = random_urlsafe_token(100);
        for ch in token.chars() {
            assert!(
                ch.is_ascii_alphanumeric() || ch == '-' || ch == '_',
                "invalid char: {ch}"
            );
        }
    }

    #[test]
    fn random_urlsafe_token_produces_unique_values() {
        let a = random_urlsafe_token(16);
        let b = random_urlsafe_token(16);
        assert_ne!(a, b);
    }

    #[test]
    fn auth_provider_as_str_values() {
        assert_eq!(AuthProvider::Unicom.as_str(), "unicom");
        assert_eq!(AuthProvider::Telecom.as_str(), "telecom");
        assert_eq!(AuthProvider::Mobile.as_str(), "mobile");
    }

    #[test]
    fn manual_input_prompt_status_as_str_values() {
        assert_eq!(ManualInputPromptStatus::Pending.as_str(), "pending");
        assert_eq!(ManualInputPromptStatus::Answered.as_str(), "answered");
        assert_eq!(ManualInputPromptStatus::Expired.as_str(), "expired");
        assert_eq!(ManualInputPromptStatus::Canceled.as_str(), "canceled");
    }

    #[test]
    fn prompt_from_input_sets_defaults() {
        let prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "Title".to_string(),
            message: "Message".to_string(),
            field_label: "Label".to_string(),
            field_kind: ManualInputFieldKind::Text,
            placeholder: None,
        });
        assert_eq!(prompt.status, ManualInputPromptStatus::Pending);
        assert_eq!(prompt.attempt_count, 0);
        assert_eq!(prompt.max_attempts, 3);
        assert_eq!(prompt.created_by.as_deref(), Some("system"));
        assert_eq!(
            prompt.last_transition_reason.as_deref(),
            Some("prompt_created")
        );
        assert!(!prompt.answer_present);
        assert!(prompt.answer_value.is_none());
        assert!(prompt.expires_at_unix_ms > prompt.created_at_unix_ms);
    }

    #[test]
    fn prompt_from_input_trims_provider() {
        let prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "  unicom  ".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::Text,
            placeholder: None,
        });
        assert_eq!(prompt.provider, "unicom");
    }

    #[test]
    fn prompt_answer_empty_string_returns_error() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        assert_eq!(
            prompt.answer("".to_string(), None, None),
            Err(ManualInputPromptError::EmptyAnswer)
        );
    }

    #[test]
    fn prompt_answer_whitespace_only_returns_error() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        assert_eq!(
            prompt.answer("   ".to_string(), None, None),
            Err(ManualInputPromptError::EmptyAnswer)
        );
    }

    #[test]
    fn prompt_answer_trims_whitespace() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        prompt
            .answer("  123456  ".to_string(), None, None)
            .expect("should succeed");
        assert_eq!(prompt.answer_value.as_deref(), Some("123456"));
    }

    #[test]
    fn prompt_answer_max_attempts_reached() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        // max_attempts is 3 by default
        prompt
            .answer("a".to_string(), None, None)
            .expect("attempt 1");
        assert_eq!(prompt.attempt_count, 1);

        // Reset to pending to allow more attempts (simulating retry flow)
        prompt.status = ManualInputPromptStatus::Pending;
        prompt
            .answer("b".to_string(), None, None)
            .expect("attempt 2");
        assert_eq!(prompt.attempt_count, 2);
        assert_eq!(prompt.retry_count, 1);

        prompt.status = ManualInputPromptStatus::Pending;
        prompt
            .answer("c".to_string(), None, None)
            .expect("attempt 3");
        assert_eq!(prompt.attempt_count, 3);

        prompt.status = ManualInputPromptStatus::Pending;
        assert_eq!(
            prompt.answer("d".to_string(), None, None),
            Err(ManualInputPromptError::MaxAttemptsReached)
        );
    }

    #[test]
    fn prompt_cancel_when_already_canceled_returns_error() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        prompt.cancel(None, None).expect("first cancel");
        assert_eq!(
            prompt.cancel(None, None),
            Err(ManualInputPromptError::Canceled)
        );
    }

    #[test]
    fn prompt_cancel_when_already_answered_returns_error() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        prompt
            .answer("123".to_string(), None, None)
            .expect("answer");
        assert_eq!(
            prompt.cancel(None, None),
            Err(ManualInputPromptError::Answered)
        );
    }

    #[test]
    fn prompt_expire_if_needed_returns_false_when_not_pending() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        prompt
            .answer("123".to_string(), None, None)
            .expect("answer");
        assert!(!prompt.expire_if_needed());
    }

    #[test]
    fn prompt_expire_if_needed_returns_false_when_not_yet_expired() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        // expires_at is ~10 minutes in the future by default
        assert!(!prompt.expire_if_needed());
        assert_eq!(prompt.status, ManualInputPromptStatus::Pending);
    }

    #[test]
    fn prompt_sanitized_removes_answer_value() {
        let mut prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: None,
            flow_id: None,
            input_id: None,
            title: "T".to_string(),
            message: "M".to_string(),
            field_label: "L".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: None,
        });
        prompt
            .answer("secret".to_string(), None, None)
            .expect("answer");
        let sanitized = prompt.sanitized();
        assert!(sanitized.answer_present);
        assert!(sanitized.answer_value.is_none());
        assert_eq!(prompt.answer_value.as_deref(), Some("secret"));
    }

    #[test]
    fn auth_session_set_failed_sets_status_and_error() {
        let mut session: AuthSession<serde_json::Value, serde_json::Value> = AuthSession::new(
            "s1".to_string(),
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        session.set_failed("something broke");
        assert_eq!(session.status, "failed");
        assert_eq!(session.last_error.as_deref(), Some("something broke"));
    }

    #[test]
    fn auth_session_apply_request_resets_from_completed() {
        let mut session: AuthSession<serde_json::Value, serde_json::Value> = AuthSession::new(
            "s1".to_string(),
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        session.set_completed(json!({"done": true}));
        assert_eq!(session.status, "completed");

        session.apply_request(
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert_eq!(session.status, "pending");
        assert!(session.report.is_none());
    }

    #[test]
    fn auth_session_apply_request_resets_from_failed() {
        let mut session: AuthSession<serde_json::Value, serde_json::Value> = AuthSession::new(
            "s1".to_string(),
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        session.set_failed("oops");
        assert_eq!(session.status, "failed");

        session.apply_request(
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert_eq!(session.status, "pending");
        assert!(session.last_error.is_none());
    }

    #[test]
    fn auth_session_apply_request_merges_inputs_and_runtime() {
        let mut session: AuthSession<serde_json::Value, serde_json::Value> = AuthSession::new(
            "s1".to_string(),
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::from([("existing".to_string(), json!("old"))]),
            BTreeMap::from([("rt1".to_string(), json!("v1"))]),
        );
        session.apply_request(
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::from([("new_key".to_string(), json!("new_val"))]),
            BTreeMap::from([("rt2".to_string(), json!("v2"))]),
        );
        assert_eq!(session.inputs.get("existing"), Some(&json!("old")));
        assert_eq!(session.inputs.get("new_key"), Some(&json!("new_val")));
        assert_eq!(session.runtime.get("rt1"), Some(&json!("v1")));
        assert_eq!(session.runtime.get("rt2"), Some(&json!("v2")));
    }

    #[test]
    fn auth_session_clears_manual_challenge_on_apply_request() {
        let mut session: AuthSession<serde_json::Value, serde_json::Value> = AuthSession::new(
            "s1".to_string(),
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        session.manual_challenge = Some(json!("challenge"));
        session.apply_request(
            "unicom".to_string(),
            "web".to_string(),
            "f1".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert!(session.manual_challenge.is_none());
    }

    #[test]
    fn auth_provider_serde_roundtrip() {
        for provider in [
            AuthProvider::Unicom,
            AuthProvider::Telecom,
            AuthProvider::Mobile,
        ] {
            let json = serde_json::to_string(&provider).unwrap();
            let decoded: AuthProvider = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, provider);
        }
    }

    #[test]
    fn manual_input_prompt_status_serde_roundtrip() {
        for status in [
            ManualInputPromptStatus::Pending,
            ManualInputPromptStatus::Answered,
            ManualInputPromptStatus::Expired,
            ManualInputPromptStatus::Canceled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: ManualInputPromptStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn manual_input_field_kind_serde_roundtrip() {
        for kind in [
            ManualInputFieldKind::Text,
            ManualInputFieldKind::PhoneNumber,
            ManualInputFieldKind::SmsCode,
            ManualInputFieldKind::Password,
            ManualInputFieldKind::Captcha,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let decoded: ManualInputFieldKind = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, kind);
        }
    }

    #[test]
    fn manual_input_event_serde_tagged_roundtrip() {
        let events = vec![
            ManualInputEvent::PromptCreated {
                prompt_id: "p1".to_string(),
                provider: "unicom".to_string(),
                session_id: Some("s1".to_string()),
                flow_id: Some("f1".to_string()),
            },
            ManualInputEvent::PromptAnswered {
                prompt_id: "p2".to_string(),
                provider: "telecom".to_string(),
                session_id: None,
                flow_id: None,
            },
            ManualInputEvent::PromptCanceled {
                prompt_id: "p3".to_string(),
                provider: "mobile".to_string(),
                session_id: Some("s3".to_string()),
                flow_id: None,
            },
            ManualInputEvent::PromptExpired {
                prompt_id: "p4".to_string(),
                provider: "unicom".to_string(),
                session_id: None,
                flow_id: Some("f4".to_string()),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let decoded: ManualInputEvent = serde_json::from_str(&json).unwrap();
            let reserialized = serde_json::to_string(&decoded).unwrap();
            assert_eq!(json, reserialized);
        }
    }

    #[test]
    fn manual_input_prompt_error_display_messages() {
        assert!(ManualInputPromptError::Expired
            .to_string()
            .contains("expired"));
        assert!(ManualInputPromptError::Canceled
            .to_string()
            .contains("canceled"));
        assert!(ManualInputPromptError::Answered
            .to_string()
            .contains("answered"));
        assert!(ManualInputPromptError::EmptyAnswer
            .to_string()
            .contains("empty"));
        assert!(ManualInputPromptError::MaxAttemptsReached
            .to_string()
            .contains("max attempts"));
        let not_pending = ManualInputPromptError::NotPending("test".to_string());
        assert!(not_pending.to_string().contains("not pending"));
    }

    #[test]
    fn token_refresh_outcome_unsupported_includes_provider_name() {
        let outcome = TokenRefreshOutcome::unsupported(AuthProvider::Unicom);
        assert!(outcome.message.unwrap().contains("unicom"));
        assert!(outcome.access_token.is_none());
        assert!(outcome.refresh_token.is_none());
    }

    #[test]
    fn manual_input_prompt_serializes_to_json() {
        let prompt = ManualInputPrompt::from_input(ManualInputPromptCreateInput {
            provider: "unicom".to_string(),
            session_id: Some("s1".to_string()),
            flow_id: None,
            input_id: None,
            title: "Title".to_string(),
            message: "Message".to_string(),
            field_label: "Label".to_string(),
            field_kind: ManualInputFieldKind::SmsCode,
            placeholder: Some("123456".to_string()),
        });
        let json = serde_json::to_string(&prompt).expect("should serialize");
        let decoded: ManualInputPrompt = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(decoded.prompt_id, prompt.prompt_id);
        assert_eq!(decoded.provider, "unicom");
        assert_eq!(decoded.status, ManualInputPromptStatus::Pending);
    }
}
