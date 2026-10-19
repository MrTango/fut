//! Server-side configured commands share the local popup and process runners.
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::{
    domain::{MouseButton as DomainButton, TerminalSize},
    extensions::ExtensionCommandExecution,
    protocol::{
        ExtensionCatalog, RemoteClipboardCopy, RemoteCommandOperation, RemoteCommandReply,
        RemoteCommandSpec, RemoteMouseKind, SelectedTarget,
    },
    resources::TrustedProjectConfig,
    terminal::TerminalConfig,
};

use super::{
    config::{self, PaletteCommand},
    temporary_command::{
        CommandSpawnContext, ExtensionCommandContext, TemporaryCommandSurface,
        TemporaryCommandUpdate, foreground_process_id, process_cwd, run_background_process_at,
    },
};

pub(crate) struct PreparedCommand {
    command: PaletteCommand,
    context: Option<ExtensionCommandContext>,
    focused: SelectedTarget,
    cwd: PathBuf,
    socket: PathBuf,
    form: Option<BTreeMap<String, String>>,
}

impl PreparedCommand {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare(
        spec: RemoteCommandSpec,
        location: config::ConfigLocation,
        catalog: ExtensionCatalog,
        focused: SelectedTarget,
        root: PathBuf,
        project: Option<TrustedProjectConfig>,
        socket: PathBuf,
        form: Option<BTreeMap<String, String>>,
    ) -> Result<Self> {
        let target = focused.clone();
        let workspace_root = root.clone();
        let (command, context) = tokio::task::spawn_blocking(move || -> Result<_> {
            let ui = if spec.extension.is_some() {
                config::stage_location(&location)?.materialize(&catalog)?
            } else {
                config::UiConfig::default()
            };
            let command = resolve_command(spec, &ui)?;
            let context = command
                .extension
                .as_ref()
                .map(|_| {
                    ExtensionCommandContext::resolve(
                        &ui,
                        &command,
                        &target,
                        &root,
                        project.as_ref(),
                    )
                })
                .transpose()?;
            Ok((command, context))
        })
        .await??;
        let cwd = if context.is_some() {
            Some(workspace_root.clone())
        } else {
            process_cwd(foreground_process_id(focused.child_pid).await).await
        };
        Ok(Self {
            command,
            context,
            focused,
            cwd: cwd.unwrap_or(workspace_root),
            socket,
            form,
        })
    }

    pub(crate) fn defaults(&self) -> Result<BTreeMap<String, String>> {
        self.command
            .fields
            .iter()
            .map(|field| {
                let value = match (&field.default_config, &self.context) {
                    (Some(key), Some(context)) => {
                        context.configured_field_default(key)?.unwrap_or_default()
                    }
                    _ => field.default.clone().unwrap_or_default(),
                };
                Ok((field.name.clone(), value))
            })
            .collect()
    }

    pub(crate) fn background(&self) -> bool {
        matches!(
            self.command.execution,
            ExtensionCommandExecution::Background
        )
    }

    pub(crate) async fn run_background(&self) -> RemoteCommandReply {
        let (exit_code, failure_output) = run_background_process_at(
            &self.command,
            self.context.as_ref(),
            &self.cwd,
            &self.socket,
            self.form.as_ref(),
            Some(&self.focused),
        )
        .await;
        RemoteCommandReply::Done {
            exit_code,
            activated_target: None,
            failure_output,
        }
    }

    pub(crate) async fn spawn(
        &self,
        terminal: TerminalConfig,
        size: TerminalSize,
    ) -> Result<CommandSurface> {
        Ok(CommandSurface(
            TemporaryCommandSurface::spawn_in(
                terminal,
                &self.command,
                size,
                CommandSpawnContext {
                    cwd: self.cwd.clone(),
                    socket: &self.socket,
                    extension: self.context.as_ref(),
                    form: self.form.as_ref(),
                    target: Some(&self.focused),
                },
            )
            .await?,
        ))
    }
}

fn resolve_command(spec: RemoteCommandSpec, ui: &config::UiConfig) -> Result<PaletteCommand> {
    if let Some(identity) = spec.extension {
        let mut command = ui
            .bindings
            .extension_command(&identity.id, &identity.command)
            .context("remote extension command is not configured")?
            .clone();
        if spec.extension_args_override {
            command.args = spec.args;
        }
        return Ok(command);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let program =
        config::expand_home_path(&spec.program, home.as_deref(), "remote command program")?;
    Ok(PaletteCommand {
        title: spec.title,
        program,
        args: spec.args,
        extension_args_override: false,
        execution: spec.execution,
        extension: None,
        binding: None,
        hotkey: None,
        fields: Vec::new(),
    })
}

pub(crate) struct CommandSurface(TemporaryCommandSurface);

impl Drop for CommandSurface {
    fn drop(&mut self) {
        self.0.close_on_disconnect();
    }
}

impl CommandSurface {
    pub(crate) fn screen(&self) -> RemoteCommandReply {
        RemoteCommandReply::Screen {
            screen: self.0.screen.clone(),
        }
    }

    pub(crate) fn ready(&self) -> RemoteCommandReply {
        RemoteCommandReply::Ready {
            screen: self.0.screen.clone(),
        }
    }

    pub(crate) async fn update(&mut self) -> (RemoteCommandReply, bool) {
        match self.0.update().await {
            TemporaryCommandUpdate::Screen => (
                RemoteCommandReply::Screen {
                    screen: self.0.screen.clone(),
                },
                false,
            ),
            TemporaryCommandUpdate::Exited(exit_code) => {
                let failure_output = if exit_code == Some(0) {
                    None
                } else {
                    self.0.failure_output().await.ok()
                };
                (
                    RemoteCommandReply::Done {
                        exit_code,
                        activated_target: self.0.activated_target().ok().flatten(),
                        failure_output,
                    },
                    true,
                )
            }
            TemporaryCommandUpdate::Error(message) => {
                (RemoteCommandReply::Error { message }, false)
            }
            TemporaryCommandUpdate::Stopped => (
                RemoteCommandReply::Error {
                    message: "command terminal stopped".into(),
                },
                true,
            ),
        }
    }

    pub(crate) async fn operation(
        &mut self,
        operation: RemoteCommandOperation,
        maximum: usize,
    ) -> Result<RemoteCommandReply> {
        use RemoteCommandOperation::*;
        let mut clipboard = None;
        let mut handled = true;
        match operation {
            Input(bytes) => self.0.input(bytes).await?,
            KeyInput(event) => self.0.key_input(event).await?,
            Paste(text) => self.0.paste(text).await?,
            Resize(size) => {
                size.validate_within(maximum)?;
                self.0.resize(size).await?;
            }
            FinishClipboard { request_id, copied } => {
                handled = self.0.finish_clipboard(request_id, copied).await?
            }
            CancelMouseCopy => self.0.cancel_mouse_copy().await,
            Mouse {
                kind,
                column,
                row,
                shift,
                control,
                alt,
                content: [x, y, width, height],
            } => {
                let mut modifiers = KeyModifiers::empty();
                modifiers.set(KeyModifiers::SHIFT, shift);
                modifiers.set(KeyModifiers::CONTROL, control);
                modifiers.set(KeyModifiers::ALT, alt);
                let event = MouseEvent {
                    kind: mouse_kind(kind),
                    column,
                    row,
                    modifiers,
                };
                clipboard = self
                    .0
                    .mouse(event, Rect::new(x, y, width, height))
                    .await?
                    .map(|copy| RemoteClipboardCopy {
                        request_id: copy.request_id,
                        text: copy.text,
                    });
            }
        }
        Ok(RemoteCommandReply::Ack { clipboard, handled })
    }
}

fn mouse_kind(kind: RemoteMouseKind) -> MouseEventKind {
    let button = |button| match button {
        DomainButton::Left => MouseButton::Left,
        DomainButton::Middle => MouseButton::Middle,
        DomainButton::Right => MouseButton::Right,
    };
    match kind {
        RemoteMouseKind::Down { button: b } => MouseEventKind::Down(button(b)),
        RemoteMouseKind::Up { button: b } => MouseEventKind::Up(button(b)),
        RemoteMouseKind::Drag { button: b } => MouseEventKind::Drag(button(b)),
        RemoteMouseKind::Moved => MouseEventKind::Moved,
        RemoteMouseKind::ScrollUp => MouseEventKind::ScrollUp,
        RemoteMouseKind::ScrollDown => MouseEventKind::ScrollDown,
        RemoteMouseKind::ScrollLeft => MouseEventKind::ScrollLeft,
        RemoteMouseKind::ScrollRight => MouseEventKind::ScrollRight,
    }
}

#[cfg(test)]
mod tests {
    use super::super::temporary_command::run_background_process;
    use super::*;
    use crate::{command::PopupSize, protocol::RemoteCommandIdentity};

    fn spec() -> RemoteCommandSpec {
        RemoteCommandSpec {
            title: "untrusted title".into(),
            program: "/client/only/program".into(),
            args: vec!["client arg".into()],
            execution: ExtensionCommandExecution::Background,
            extension: Some(RemoteCommandIdentity {
                id: "configured".into(),
                command: "launch".into(),
            }),
            extension_args_override: false,
        }
    }

    #[test]
    fn generic_home_relative_program_expands_on_the_execution_host() {
        let mut supplied = spec();
        supplied.extension = None;
        supplied.program = "~/bin/diff-popup".into();
        let command = resolve_command(supplied, &config::UiConfig::default()).unwrap();
        assert_eq!(
            command.program,
            PathBuf::from(std::env::var_os("HOME").unwrap()).join("bin/diff-popup")
        );
    }

    #[test]
    fn extension_identity_resolves_authoritatively_and_args_override_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("extension");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(crate::extensions::MANIFEST_FILE_NAME),
            "api_version = 1\nversion = '1.0.0'\nfut = '>=0.7.0, <1.0.0'\ncapabilities = ['commands']\nid = 'configured'\n[commands.launch]\ntitle = 'Remote title'\nargv = ['./launch', '--manifest']\nactivate_opened = true\n").unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, format!("extensions = [{:?}]\n[extension_commands.\"configured:launch\"]\nargs = ['--remote-config']\n", root.display().to_string())).unwrap();
        let ui = config::load_location(&config::ConfigLocation {
            path: Some(path),
            explicit: true,
            source: "test",
        })
        .unwrap()
        .ui;
        let resolved = resolve_command(spec(), &ui).unwrap();
        assert_eq!(
            resolved.program,
            root.canonicalize().unwrap().join("launch")
        );
        assert_eq!(resolved.title, "Remote title");
        assert_eq!(resolved.args, ["--remote-config"]);
        assert_eq!(
            resolved.execution,
            ExtensionCommandExecution::Interactive {
                size: PopupSize::default(),
                activate_opened: true
            }
        );
        assert_eq!(
            resolved.extension.unwrap().root,
            root.canonicalize().unwrap()
        );
        let mut overridden = spec();
        overridden.extension_args_override = true;
        overridden.args.clear();
        assert!(resolve_command(overridden, &ui).unwrap().args.is_empty());
        let mut missing = spec();
        missing.extension.as_mut().unwrap().command = "missing".into();
        assert!(resolve_command(missing, &ui).is_err());
    }

    #[tokio::test]
    async fn generic_background_preserves_remote_cwd_and_failure_status() {
        let temp = tempfile::tempdir().unwrap();
        let mut supplied = spec();
        supplied.extension = None;
        supplied.program = "/bin/sh".into();
        supplied.args = vec!["-c".into(), "printf '%s' \"$PWD\" >&2; exit 7".into()];
        let command = resolve_command(supplied, &config::UiConfig::default()).unwrap();
        let (code, failure) = run_background_process(
            &command,
            None,
            temp.path(),
            &temp.path().join("fut.sock"),
            None,
        )
        .await;
        assert_eq!(code, Some(7));
        assert!(failure.unwrap().contains(temp.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn cancelling_background_runner_kills_its_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let mut supplied = spec();
        supplied.extension = None;
        supplied.program = "/bin/sh".into();
        supplied.args = vec![
            "-c".into(),
            format!("sleep 60 & echo $! > '{}'; wait", pid_file.display()),
        ];
        let command = resolve_command(supplied, &config::UiConfig::default()).unwrap();
        let cwd = temp.path().to_owned();
        let task = tokio::spawn(async move {
            run_background_process(&command, None, &cwd, &cwd.join("fut.sock"), None).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !pid_file.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        task.abort();
        let _ = task.await;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn popup_operations_and_disconnect_close_the_process() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let mut supplied = spec();
        supplied.extension = None;
        supplied.program = "/bin/sh".into();
        supplied.args = vec![
            "-c".into(),
            format!("echo $$ > '{}'; exec cat", pid_file.display()),
        ];
        supplied.execution = ExtensionCommandExecution::Interactive {
            size: PopupSize::default(),
            activate_opened: false,
        };
        let command = resolve_command(supplied, &config::UiConfig::default()).unwrap();
        let surface = TemporaryCommandSurface::spawn(
            Default::default(),
            &command,
            std::process::id(),
            TerminalSize {
                columns: 30,
                rows: 5,
            },
            &temp.path().join("fut.sock"),
            None,
            None,
        )
        .await
        .unwrap();
        let mut surface = CommandSurface(surface);
        assert!(matches!(surface.ready(), RemoteCommandReply::Ready { .. }));
        assert_eq!(
            surface
                .operation(RemoteCommandOperation::Input(b"hello\n".to_vec()), 150_000)
                .await
                .unwrap(),
            RemoteCommandReply::Ack {
                clipboard: None,
                handled: true
            }
        );
        assert!(
            surface
                .operation(
                    RemoteCommandOperation::Resize(TerminalSize {
                        columns: 0,
                        rows: 5
                    }),
                    150_000
                )
                .await
                .is_err()
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !pid_file.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(surface);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
