//! Desktop service install/status helpers for ET-Gui.

use anyhow::Context;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceOptions {
    pub(super) config_dir: String,
    pub(super) rpc_portal: String,
    pub(super) file_log_level: String,
    pub(super) file_log_dir: String,
    pub(super) config_server: Option<String>,
    #[serde(default)]
    pub(super) secure_mode: bool,
}
impl ServiceOptions {
    fn to_args_vec(&self) -> Vec<std::ffi::OsString> {
        let mut args = vec![
            "--config-dir".into(),
            self.config_dir.clone().into(),
            "--rpc-portal".into(),
            self.rpc_portal.clone().into(),
            "--file-log-level".into(),
            self.file_log_level.clone().into(),
            "--file-log-dir".into(),
            self.file_log_dir.clone().into(),
            "--daemon".into(),
        ];

        if let Some(config_server) = &self.config_server {
            args.push("--config-server".into());
            args.push(config_server.clone().into());
            if self.secure_mode {
                // Reused by core as the config-server encrypted-tunnel requirement
                // when --config-server is set (no network-name, so not merged into configs).
                args.push("--secure-mode".into());
            }
        }

        args
    }
}

#[cfg(target_os = "macos")]
fn service_environment() -> Option<Vec<(String, String)>> {
    // System LaunchDaemons run as root but launchd does not provide HOME.
    Some(vec![("HOME".to_string(), "/var/root".to_string())])
}

#[cfg(not(target_os = "macos"))]
fn service_environment() -> Option<Vec<(String, String)>> {
    None
}

pub fn install(opts: ServiceOptions) -> anyhow::Result<()> {
    let service = easytier::service_manager::Service::new("ET-Gui".to_string())?;
    let options = easytier::service_manager::ServiceInstallOptions {
        program: crate::get_exe_path().into(),
        args: opts.to_args_vec(),
        work_directory: std::env::current_dir()?,
        environment: service_environment(),
        disable_autostart: false,
        description: Some("ET Gui Service".to_string()),
        display_name: Some("ET Gui Service".to_string()),
        disable_restart_on_failure: false,
    };
    service
        .install(&options)
        .with_context(|| "Failed to install service")?;
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let service = easytier::service_manager::Service::new("ET-Gui".to_string())?;
    service.uninstall()?;
    Ok(())
}

pub fn set_status(enable: bool) -> anyhow::Result<()> {
    use easytier::service_manager::*;
    let service = Service::new("ET-Gui".to_string())?;
    let status = service.status()?;
    if enable && status != ServiceStatus::Running {
        service.start().with_context(|| "Failed to start service")?;
    } else if !enable && status == ServiceStatus::Running {
        service.stop().with_context(|| "Failed to stop service")?;
    } else if status == ServiceStatus::NotInstalled {
        return Err(anyhow::anyhow!("Service not installed"));
    }
    Ok(())
}

pub fn status() -> anyhow::Result<easytier::service_manager::ServiceStatus> {
    let service = easytier::service_manager::Service::new("ET-Gui".to_string())?;
    service.status()
}

#[cfg(test)]
mod tests {
    #[test]
    fn service_environment_matches_platform() {
        #[cfg(target_os = "macos")]
        assert_eq!(
            super::service_environment(),
            Some(vec![("HOME".to_string(), "/var/root".to_string())])
        );

        #[cfg(not(target_os = "macos"))]
        assert_eq!(super::service_environment(), None);
    }
}
