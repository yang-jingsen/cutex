use crate::model::{RunId, ServiceId};
use crate::protocol::{LogDiagnostics, LogEncoding, LogEntry, LogPage, LogStream};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, TryRecvError, TrySendError};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogBufferConfig {
    pub max_entries_per_service: usize,
    pub max_bytes_per_service: usize,
}

impl Default for LogBufferConfig {
    fn default() -> Self {
        Self {
            max_entries_per_service: 4_096,
            max_bytes_per_service: 4 * 1024 * 1024,
        }
    }
}

pub struct LogSubscription {
    receiver: Receiver<LogEntry>,
}

impl LogSubscription {
    pub fn recv(&self) -> Result<LogEntry, RecvError> {
        self.receiver.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<LogEntry, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<LogEntry, TryRecvError> {
        self.receiver.try_recv()
    }
}

pub(crate) struct LogStore {
    config: LogBufferConfig,
    state: Mutex<LogState>,
}

#[derive(Default)]
struct LogState {
    services: BTreeMap<ServiceId, ServiceLog>,
    subscribers: BTreeMap<u64, Subscriber>,
    next_subscriber_id: u64,
}

#[derive(Default)]
struct ServiceLog {
    next_sequence: u64,
    entries: VecDeque<StoredLogEntry>,
    bytes: usize,
    diagnostics: LogDiagnostics,
}

struct StoredLogEntry {
    entry: LogEntry,
    raw_bytes: usize,
}

struct Subscriber {
    service_id: ServiceId,
    run_id: Option<RunId>,
    sender: mpsc::SyncSender<LogEntry>,
}

impl LogStore {
    pub(crate) fn new(mut config: LogBufferConfig) -> Self {
        config.max_entries_per_service = config.max_entries_per_service.max(1);
        config.max_bytes_per_service = config.max_bytes_per_service.max(1);
        Self {
            config,
            state: Mutex::new(LogState::default()),
        }
    }

    pub(crate) fn append(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        timestamp_ms: u64,
        stream: LogStream,
        bytes: &[u8],
    ) -> LogEntry {
        let mut state = self.state.lock().expect("log store lock poisoned");
        let max_bytes = self.config.max_bytes_per_service;
        let accepted_len = bytes.len().min(max_bytes);
        let accepted = &bytes[..accepted_len];
        let truncated = accepted_len != bytes.len();
        let (encoding, data) = match std::str::from_utf8(accepted) {
            Ok(text) => (LogEncoding::Utf8, text.to_owned()),
            Err(_) => (LogEncoding::Base64, BASE64_STANDARD.encode(accepted)),
        };

        let service_log = state.services.entry(service_id.clone()).or_default();
        service_log.next_sequence = service_log.next_sequence.saturating_add(1);
        let entry = LogEntry {
            service_id: service_id.clone(),
            run_id: run_id.clone(),
            sequence: service_log.next_sequence,
            timestamp_ms,
            stream,
            encoding,
            data,
            truncated,
        };

        if truncated {
            service_log.diagnostics.evicted_bytes = service_log
                .diagnostics
                .evicted_bytes
                .saturating_add((bytes.len() - accepted_len) as u64);
        }
        while service_log.entries.len() >= self.config.max_entries_per_service
            || service_log.bytes.saturating_add(accepted_len) > max_bytes
        {
            let Some(evicted) = service_log.entries.pop_front() else {
                break;
            };
            service_log.bytes = service_log.bytes.saturating_sub(evicted.raw_bytes);
            service_log.diagnostics.evicted_entries =
                service_log.diagnostics.evicted_entries.saturating_add(1);
            service_log.diagnostics.evicted_bytes = service_log
                .diagnostics
                .evicted_bytes
                .saturating_add(evicted.raw_bytes as u64);
        }
        service_log.bytes = service_log.bytes.saturating_add(accepted_len);
        service_log.entries.push_back(StoredLogEntry {
            entry: entry.clone(),
            raw_bytes: accepted_len,
        });

        let mut slow_drops = 0_u64;
        state.subscribers.retain(|_, subscriber| {
            if subscriber.service_id != *service_id
                || subscriber
                    .run_id
                    .as_ref()
                    .is_some_and(|selected| selected != run_id)
            {
                return true;
            }
            match subscriber.sender.try_send(entry.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    slow_drops = slow_drops.saturating_add(1);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
        if slow_drops > 0 {
            state
                .services
                .get_mut(service_id)
                .expect("service log inserted above")
                .diagnostics
                .slow_subscriber_drops = state.services[service_id]
                .diagnostics
                .slow_subscriber_drops
                .saturating_add(slow_drops);
        }
        entry
    }

    pub(crate) fn subscribe(
        &self,
        service_id: ServiceId,
        run_id: Option<RunId>,
        capacity: usize,
    ) -> LogSubscription {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let mut state = self.state.lock().expect("log store lock poisoned");
        state.next_subscriber_id = state.next_subscriber_id.saturating_add(1);
        let subscriber_id = state.next_subscriber_id;
        state.subscribers.insert(
            subscriber_id,
            Subscriber {
                service_id,
                run_id,
                sender,
            },
        );
        LogSubscription { receiver }
    }

    pub(crate) fn read(
        &self,
        service_id: &ServiceId,
        run_id: Option<&RunId>,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> LogPage {
        let state = self.state.lock().expect("log store lock poisoned");
        let after = after_sequence.unwrap_or(0);
        let max = usize::try_from(limit.clamp(1, 10_000)).unwrap_or(10_000);
        let Some(service_log) = state.services.get(service_id) else {
            return LogPage {
                service_id: service_id.clone(),
                run_id: run_id.cloned(),
                entries: Vec::new(),
                next_sequence: after_sequence,
                diagnostics: LogDiagnostics::default(),
            };
        };
        let entries: Vec<_> = service_log
            .entries
            .iter()
            .filter(|stored| stored.entry.sequence > after)
            .filter(|stored| run_id.is_none_or(|selected| &stored.entry.run_id == selected))
            .take(max)
            .map(|stored| stored.entry.clone())
            .collect();
        let next_sequence = entries
            .last()
            .map(|entry| entry.sequence)
            .or(after_sequence);
        LogPage {
            service_id: service_id.clone(),
            run_id: run_id.cloned(),
            entries,
            next_sequence,
            diagnostics: service_log.diagnostics.clone(),
        }
    }

    pub(crate) fn report_error(&self, service_id: &ServiceId, message: String) {
        let mut state = self.state.lock().expect("log store lock poisoned");
        state
            .services
            .entry(service_id.clone())
            .or_default()
            .diagnostics
            .last_error = Some(message);
    }

    pub(crate) fn seed_sequence(&self, service_id: &ServiceId, sequence: u64) {
        let mut state = self.state.lock().expect("log store lock poisoned");
        let service = state.services.entry(service_id.clone()).or_default();
        service.next_sequence = service.next_sequence.max(sequence);
    }
}
