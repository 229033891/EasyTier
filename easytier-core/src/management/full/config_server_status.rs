//! Process-wide config-server connection status for local UI / RPC queries.

use parking_lot::RwLock;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigServerStatusSnapshot {
    pub enabled: bool,
    pub connected: bool,
    pub last_error: Option<String>,
}

static STATUS: RwLock<ConfigServerStatusSnapshot> = RwLock::new(ConfigServerStatusSnapshot {
    enabled: false,
    connected: false,
    last_error: None,
});

pub fn snapshot() -> ConfigServerStatusSnapshot {
    STATUS.read().clone()
}

pub fn mark_enabled() {
    let mut status = STATUS.write();
    status.enabled = true;
    status.connected = false;
    status.last_error = None;
}

pub fn mark_connected() {
    let mut status = STATUS.write();
    status.enabled = true;
    status.connected = true;
    status.last_error = None;
}

pub fn mark_disconnected() {
    let mut status = STATUS.write();
    if status.enabled {
        status.connected = false;
    }
}

pub fn mark_error(error: impl Into<String>) {
    let mut status = STATUS.write();
    status.enabled = true;
    status.connected = false;
    status.last_error = Some(error.into());
}

pub fn clear() {
    *STATUS.write() = ConfigServerStatusSnapshot::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_transitions_clear_error_on_connect() {
        clear();
        mark_enabled();
        mark_error("boom");
        assert_eq!(
            snapshot(),
            ConfigServerStatusSnapshot {
                enabled: true,
                connected: false,
                last_error: Some("boom".into()),
            }
        );
        mark_connected();
        assert_eq!(
            snapshot(),
            ConfigServerStatusSnapshot {
                enabled: true,
                connected: true,
                last_error: None,
            }
        );
        clear();
    }
}
