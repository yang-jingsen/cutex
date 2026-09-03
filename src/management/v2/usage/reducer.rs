use serde_json::Map;
use serde_json::Value;

use crate::management::v2::model::EventEnvelope;
use crate::management::v2::model::EventSource;
use crate::management::v2::model::NativeMessageKind;
use crate::management::v2::model::MAX_SAFE_SEQUENCE;

use super::model::normalize_label;
use super::model::parse_timestamp;
use super::model::validate_non_empty;
use super::model::EventPosition;
use super::model::ResetWindowKind;
use super::model::ResetWindowState;
use super::model::TokenUsageBreakdown;
use super::model::UsageAttribution;
use super::model::UsageLedgerEntry;
use super::model::UsageResetBoundary;
use super::model::UsageSample;
use super::model::UsageStateFile;
use super::model::UsageTokenSource;
use super::store::UsageMutation;

#[derive(Debug)]
pub(super) enum UsageEvent {
    Token {
        thread_id: String,
        turn_id: String,
        total: TokenUsageBreakdown,
        last: Option<TokenUsageBreakdown>,
        model_context_window: Option<u64>,
    },
    Settings {
        thread_id: String,
        patch: AttributionPatch,
    },
    ModelRerouted {
        thread_id: String,
        turn_id: String,
        to_model: String,
    },
    ResetWindows {
        limit_id: Option<String>,
        windows: Vec<(ResetWindowKind, ResetWindowPatch)>,
    },
}

#[derive(Debug, Default)]
pub(super) struct AttributionPatch {
    provider: Option<Option<String>>,
    model: Option<Option<String>>,
    reasoning_effort: Option<Option<String>>,
    service_tier: Option<Option<String>>,
}

impl AttributionPatch {
    fn apply(self, attribution: &mut UsageAttribution) -> bool {
        let mut changed = false;
        for (field, patch) in [
            (&mut attribution.provider, self.provider),
            (&mut attribution.model, self.model),
            (&mut attribution.reasoning_effort, self.reasoning_effort),
            (&mut attribution.service_tier, self.service_tier),
        ] {
            if let Some(value) = patch {
                changed |= *field != value;
                *field = value;
            }
        }
        changed
    }
}

#[derive(Debug, Default)]
pub(super) struct ResetWindowPatch {
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

pub(super) fn parse_usage_event(envelope: &EventEnvelope) -> anyhow::Result<Option<UsageEvent>> {
    if envelope.source != EventSource::AppServer {
        return Ok(None);
    }
    let Some(native) = envelope.native.as_ref() else {
        return Ok(None);
    };
    if native.kind != NativeMessageKind::Notification {
        return Ok(None);
    }
    let Some(method) = native.message.get("method").and_then(Value::as_str) else {
        return Ok(None);
    };
    let params = native
        .message
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("{method} usage event requires object params"))?;
    match method {
        "thread/tokenUsage/updated" => {
            let token_usage = params
                .get("tokenUsage")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    anyhow::anyhow!("thread/tokenUsage/updated requires tokenUsage object")
                })?;
            let total = parse_breakdown(
                token_usage.get("total").ok_or_else(|| {
                    anyhow::anyhow!("thread/tokenUsage/updated requires tokenUsage.total")
                })?,
                "tokenUsage.total",
            )?;
            let last = token_usage
                .get("last")
                .filter(|value| !value.is_null())
                .map(|value| parse_breakdown(value, "tokenUsage.last"))
                .transpose()?;
            Ok(Some(UsageEvent::Token {
                thread_id: required_string(params, "threadId", method)?,
                turn_id: required_string(params, "turnId", method)?,
                total,
                last,
                model_context_window: optional_u64(
                    token_usage,
                    "modelContextWindow",
                    "tokenUsage",
                )?,
            }))
        }
        "thread/settings/updated" => {
            let settings = params
                .get("threadSettings")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    anyhow::anyhow!("thread/settings/updated requires threadSettings object")
                })?;
            Ok(Some(UsageEvent::Settings {
                thread_id: required_string(params, "threadId", method)?,
                patch: AttributionPatch {
                    provider: string_patch(settings, "modelProvider")?,
                    model: string_patch(settings, "model")?,
                    reasoning_effort: string_patch(settings, "effort")?,
                    service_tier: string_patch(settings, "serviceTier")?,
                },
            }))
        }
        "model/rerouted" => Ok(Some(UsageEvent::ModelRerouted {
            thread_id: required_string(params, "threadId", method)?,
            turn_id: required_string(params, "turnId", method)?,
            to_model: required_string(params, "toModel", method)?,
        })),
        "account/rateLimits/updated" => {
            let rate_limits = params
                .get("rateLimits")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    anyhow::anyhow!("account/rateLimits/updated requires rateLimits object")
                })?;
            let mut windows = Vec::new();
            if let Some(window) = parse_reset_window_patch(rate_limits, "primary")? {
                windows.push((ResetWindowKind::Primary, window));
            }
            if let Some(window) = parse_reset_window_patch(rate_limits, "secondary")? {
                windows.push((ResetWindowKind::Secondary, window));
            }
            if windows.is_empty() {
                return Ok(None);
            }
            Ok(Some(UsageEvent::ResetWindows {
                limit_id: optional_string(rate_limits, "limitId", "rateLimits")?,
                windows,
            }))
        }
        _ => Ok(None),
    }
}

pub(super) fn reduce_usage_event(
    state: &mut UsageStateFile,
    envelope: &EventEnvelope,
    launched_profile: Option<&str>,
    event: UsageEvent,
) -> anyhow::Result<UsageMutation> {
    let profile = normalize_label(launched_profile);
    match event {
        UsageEvent::Token {
            thread_id,
            turn_id,
            total,
            last,
            model_context_window,
        } => reduce_token(
            state,
            envelope,
            profile,
            thread_id,
            turn_id,
            total,
            last,
            model_context_window,
        ),
        UsageEvent::Settings { thread_id, patch } => {
            reduce_settings(state, envelope, profile, thread_id, patch)
        }
        UsageEvent::ModelRerouted {
            thread_id,
            turn_id,
            to_model,
        } => reduce_reroute(state, envelope, profile, thread_id, turn_id, to_model),
        UsageEvent::ResetWindows { limit_id, windows } => {
            reduce_reset_windows(state, envelope, profile, limit_id, windows)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reduce_token(
    state: &mut UsageStateFile,
    envelope: &EventEnvelope,
    profile: Option<String>,
    thread_id: String,
    turn_id: String,
    total: TokenUsageBreakdown,
    last: Option<TokenUsageBreakdown>,
    model_context_window: Option<u64>,
) -> anyhow::Result<UsageMutation> {
    total.validate()?;
    if let Some(last) = &last {
        last.validate()?;
    }
    if model_context_window.is_some_and(|value| value == 0 || value > MAX_SAFE_SEQUENCE) {
        anyhow::bail!("tokenUsage.modelContextWindow is outside the JSON-safe range");
    }
    let position = EventPosition::from_envelope(envelope)?;
    position.validate()?;

    let sample = {
        let session = state
            .sessions
            .entry(envelope.cutex_session_id.clone())
            .or_default();
        let counter = session.threads.entry(thread_id.clone()).or_default();
        if position_is_stale(counter.position.as_ref(), &position)? {
            return Ok(UsageMutation::default());
        }

        counter.profile = profile.clone();
        counter.position = Some(position.clone());
        let Some(previous) = counter.previous_total.clone() else {
            counter.previous_total = Some(total);
            counter.counter_epoch = 1;
            return Ok(UsageMutation {
                changed: true,
                entries: Vec::new(),
            });
        };
        if total == previous {
            return Ok(UsageMutation {
                changed: true,
                entries: Vec::new(),
            });
        }
        if total.has_regression_from(&previous) {
            counter.previous_total = Some(total);
            counter.counter_epoch = counter
                .counter_epoch
                .checked_add(1)
                .filter(|epoch| *epoch <= MAX_SAFE_SEQUENCE)
                .ok_or_else(|| anyhow::anyhow!("usage counter epoch exhausted"))?;
            return Ok(UsageMutation {
                changed: true,
                entries: Vec::new(),
            });
        }

        let cumulative_delta = total.checked_difference(&previous)?;
        counter.previous_total = Some(total.clone());
        if cumulative_delta.detailed_is_zero() {
            return Ok(UsageMutation {
                changed: true,
                entries: Vec::new(),
            });
        }
        let (tokens, token_source) = match last.filter(|value| !value.detailed_is_zero()) {
            Some(last) => (last, UsageTokenSource::NativeLast),
            None => (cumulative_delta, UsageTokenSource::CumulativeDelta),
        };
        let mut attribution = counter.attribution.clone();
        if let Some(model) = counter.rerouted_models.get(&turn_id) {
            attribution.model = Some(model.clone());
        }
        UsageSample {
            entry_id: format!("usage:{}", envelope.event_id),
            event_id: envelope.event_id.clone(),
            observed_at: envelope.received_at.clone(),
            host_id: envelope.host_id.clone(),
            cutex_session_id: envelope.cutex_session_id.clone(),
            thread_id,
            turn_id,
            runtime_generation: position.runtime_generation,
            profile,
            attribution,
            token_source,
            model_context_window,
            tokens,
            cumulative_total: total,
        }
    };
    let entry = UsageLedgerEntry::Usage(Box::new(sample));
    entry.validate()?;
    Ok(UsageMutation {
        changed: true,
        entries: vec![entry],
    })
}

fn reduce_settings(
    state: &mut UsageStateFile,
    envelope: &EventEnvelope,
    profile: Option<String>,
    thread_id: String,
    patch: AttributionPatch,
) -> anyhow::Result<UsageMutation> {
    let position = EventPosition::from_envelope(envelope)?;
    position.validate()?;
    let session = state
        .sessions
        .entry(envelope.cutex_session_id.clone())
        .or_default();
    let counter = session.threads.entry(thread_id).or_default();
    if position_is_stale(counter.position.as_ref(), &position)? {
        return Ok(UsageMutation::default());
    }
    counter.profile = profile;
    let attribution_changed = patch.apply(&mut counter.attribution);
    let position_changed = counter.position.as_ref() != Some(&position);
    counter.position = Some(position);
    Ok(UsageMutation {
        changed: attribution_changed || position_changed,
        entries: Vec::new(),
    })
}

fn reduce_reroute(
    state: &mut UsageStateFile,
    envelope: &EventEnvelope,
    profile: Option<String>,
    thread_id: String,
    turn_id: String,
    to_model: String,
) -> anyhow::Result<UsageMutation> {
    let position = EventPosition::from_envelope(envelope)?;
    position.validate()?;
    let session = state
        .sessions
        .entry(envelope.cutex_session_id.clone())
        .or_default();
    let counter = session.threads.entry(thread_id).or_default();
    if position_is_stale(counter.position.as_ref(), &position)? {
        return Ok(UsageMutation::default());
    }
    counter.profile = profile;
    let reroute_changed = counter.rerouted_models.get(&turn_id) != Some(&to_model);
    let position_changed = counter.position.as_ref() != Some(&position);
    // App-server runs one active turn per thread. Keeping only its override
    // prevents historical reroutes from growing the durable reducer state.
    counter.rerouted_models.clear();
    counter.rerouted_models.insert(turn_id, to_model);
    counter.position = Some(position);
    Ok(UsageMutation {
        changed: reroute_changed || position_changed,
        entries: Vec::new(),
    })
}

fn reduce_reset_windows(
    state: &mut UsageStateFile,
    envelope: &EventEnvelope,
    profile: Option<String>,
    limit_id: Option<String>,
    windows: Vec<(ResetWindowKind, ResetWindowPatch)>,
) -> anyhow::Result<UsageMutation> {
    let observed_at = parse_timestamp(&envelope.received_at)?;
    let mut changed = false;
    let mut entries = Vec::new();
    for (window_kind, patch) in windows {
        let index = state.reset_windows.iter().position(|window| {
            window.profile == profile
                && window.limit_id == limit_id
                && window.window_kind == window_kind
        });
        let window = if let Some(index) = index {
            &mut state.reset_windows[index]
        } else {
            state.reset_windows.push(ResetWindowState {
                profile: profile.clone(),
                limit_id: limit_id.clone(),
                window_kind,
                window_duration_mins: None,
                resets_at: None,
                emitted_window_duration_mins: None,
                emitted_resets_at: None,
                last_observed_at: None,
            });
            changed = true;
            state
                .reset_windows
                .last_mut()
                .ok_or_else(|| anyhow::anyhow!("reset window disappeared"))?
        };
        if window
            .last_observed_at
            .as_deref()
            .map(parse_timestamp)
            .transpose()?
            .is_some_and(|current| observed_at < current)
        {
            continue;
        }
        if let Some(value) = patch.window_duration_mins {
            changed |= window.window_duration_mins != Some(value);
            window.window_duration_mins = Some(value);
        }
        if let Some(value) = patch.resets_at {
            changed |= window.resets_at != Some(value);
            window.resets_at = Some(value);
        }
        if patch.window_duration_mins.is_some() || patch.resets_at.is_some() {
            window.last_observed_at = Some(envelope.received_at.clone());
        }
        let Some(window_duration_mins) = window.window_duration_mins else {
            continue;
        };
        let Some(resets_at) = window.resets_at else {
            continue;
        };
        if window.emitted_window_duration_mins == Some(window_duration_mins)
            && window.emitted_resets_at == Some(resets_at)
        {
            continue;
        }
        window.emitted_window_duration_mins = Some(window_duration_mins);
        window.emitted_resets_at = Some(resets_at);
        changed = true;
        let entry = UsageLedgerEntry::ResetBoundary(UsageResetBoundary {
            entry_id: format!(
                "reset:{}:{}",
                envelope.event_id,
                match window_kind {
                    ResetWindowKind::Primary => "primary",
                    ResetWindowKind::Secondary => "secondary",
                }
            ),
            event_id: envelope.event_id.clone(),
            observed_at: envelope.received_at.clone(),
            host_id: envelope.host_id.clone(),
            profile: profile.clone(),
            limit_id: limit_id.clone(),
            window_kind,
            window_duration_mins,
            resets_at,
        });
        entry.validate()?;
        entries.push(entry);
    }
    Ok(UsageMutation { changed, entries })
}

fn position_is_stale(
    current: Option<&EventPosition>,
    candidate: &EventPosition,
) -> anyhow::Result<bool> {
    current
        .map(|current| candidate.is_newer_than(current).map(|newer| !newer))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse_breakdown(value: &Value, label: &str) -> anyhow::Result<TokenUsageBreakdown> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{label} must be an object"))?;
    Ok(TokenUsageBreakdown {
        total_tokens: required_u64(object, "totalTokens", label)?,
        input_tokens: required_u64(object, "inputTokens", label)?,
        cached_input_tokens: required_u64(object, "cachedInputTokens", label)?,
        cache_write_input_tokens: optional_u64(object, "cacheWriteInputTokens", label)?
            .unwrap_or(0),
        output_tokens: required_u64(object, "outputTokens", label)?,
        reasoning_output_tokens: required_u64(object, "reasoningOutputTokens", label)?,
    })
}

fn parse_reset_window_patch(
    object: &Map<String, Value>,
    key: &str,
) -> anyhow::Result<Option<ResetWindowPatch>> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let window = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("rateLimits.{key} must be an object or null"))?;
    let patch = ResetWindowPatch {
        window_duration_mins: optional_i64(
            window,
            "windowDurationMins",
            &format!("rateLimits.{key}"),
        )?,
        resets_at: optional_i64(window, "resetsAt", &format!("rateLimits.{key}"))?,
    };
    if patch.window_duration_mins.is_some_and(|value| value <= 0) {
        anyhow::bail!("rateLimits.{key}.windowDurationMins must be positive");
    }
    if patch.resets_at.is_some_and(|value| value <= 0) {
        anyhow::bail!("rateLimits.{key}.resetsAt must be positive");
    }
    if patch.window_duration_mins.is_none() && patch.resets_at.is_none() {
        return Ok(None);
    }
    Ok(Some(patch))
}

fn required_string(object: &Map<String, Value>, key: &str, label: &str) -> anyhow::Result<String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{label} requires string {key}"))?;
    validate_non_empty(&format!("{label}.{key}"), value)?;
    Ok(value.to_string())
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    label: &str,
) -> anyhow::Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        Some(Value::String(_)) => anyhow::bail!("{label}.{key} must not be empty"),
        Some(_) => anyhow::bail!("{label}.{key} must be a string or null"),
    }
}

fn string_patch(object: &Map<String, Value>, key: &str) -> anyhow::Result<Option<Option<String>>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(Some(value.clone()))),
        Some(Value::String(_)) => anyhow::bail!("threadSettings.{key} must not be empty"),
        Some(_) => anyhow::bail!("threadSettings.{key} must be a string or null"),
    }
}

fn required_u64(object: &Map<String, Value>, key: &str, label: &str) -> anyhow::Result<u64> {
    optional_u64(object, key, label)?.ok_or_else(|| anyhow::anyhow!("{label} requires {key}"))
}

fn optional_u64(
    object: &Map<String, Value>,
    key: &str,
    label: &str,
) -> anyhow::Result<Option<u64>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|value| *value <= MAX_SAFE_SEQUENCE)
            .map(Some)
            .ok_or_else(|| {
                anyhow::anyhow!("{label}.{key} must be a non-negative JSON-safe integer")
            }),
    }
}

fn optional_i64(
    object: &Map<String, Value>,
    key: &str,
    label: &str,
) -> anyhow::Result<Option<i64>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("{label}.{key} must be an integer or null")),
    }
}
