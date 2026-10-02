/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Luis Liu. All rights reserved.
 *  Licensed under the MIT License. See License in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

use super::Command;
use anyhow::Result;
use std::mem;
use std::os::windows::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, w};

/// The implementation of state check and elevated executing varies on each platform
impl Command {
    /// Check the state the current program running
    ///
    /// Return `true` if the program is running as root, otherwise false
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use elevated_command::Command;
    ///
    /// fn main() {
    ///     let is_elevated = Command::is_elevated();
    ///
    /// }
    /// ```
    pub fn is_elevated() -> bool {
        // Thanks to https://stackoverflow.com/a/8196291
        unsafe {
            let mut current_token: HANDLE = HANDLE::default();
            let mut token_elevation = TOKEN_ELEVATION::default();
            let mut size = 0u32;

            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut current_token).is_ok() {
                if GetTokenInformation(
                    current_token,
                    TokenElevation,
                    Some((&raw mut token_elevation).cast()),
                    mem::size_of::<TOKEN_ELEVATION>() as u32,
                    &mut size,
                )
                .is_ok()
                {
                    return token_elevation.TokenIsElevated != 0;
                }
            }
        }
        false
    }

    /// Prompting the user with a graphical OS dialog for the root password,
    /// excuting the command with escalated privileges, and return the output
    ///
    /// On Windows, according to https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew#return-value,
    /// Output.status.code() shoudl be greater than 32 if the function succeeds,
    /// otherwise the value indicates the cause of the failure
    ///
    /// On Windows, Output.stdout and Output.stderr will always be empty as of now
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use elevated_command::Command;
    /// use std::process::Command as StdCommand;
    ///
    /// fn main() {
    ///     let mut cmd = StdCommand::new("path to the application");
    ///     let elevated_cmd = Command::new(cmd);
    ///     let output = elevated_cmd.output().unwrap();
    /// }
    /// ```
    pub fn output(&self) -> Result<Output> {
        let args = self
            .cmd
            .get_args()
            .map(|c| c.to_str().unwrap().to_string())
            .collect::<Vec<String>>();
        let parameters = if args.is_empty() {
            HSTRING::new()
        } else {
            let arg_str = args.join(" ");
            HSTRING::from(arg_str)
        };

        // according to https://stackoverflow.com/a/38034535
        // the cwd always point to %SystemRoot%\System32 and cannot be changed by settting lpdirectory param
        let r = unsafe {
            ShellExecuteW(
                None,
                w!("runas"),
                &HSTRING::from(self.cmd.get_program()),
                &HSTRING::from(parameters),
                PCWSTR::null(),
                SW_HIDE,
            )
        };
        Ok(Output {
            status: ExitStatus::from_raw(r.0 as u32),
            stdout: Vec::<u8>::new(),
            stderr: Vec::<u8>::new(),
        })
    }
}
