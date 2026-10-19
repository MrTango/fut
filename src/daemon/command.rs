//! A CommandStart dedicates its control connection to one command. EOF drops
//! the popup terminal, or cancels the background runner's kill-on-drop child.
use super::*;
use crate::{
    client::command_server::PreparedCommand,
    protocol::{RemoteCommandReply, RemoteCommandSpec},
};

pub(super) async fn reply(
    connection: &mut ClientConnection,
    request_id: Option<uuid::Uuid>,
    reply: RemoteCommandReply,
) -> Result<()> {
    send(
        connection,
        request_id,
        ServerMessage::CommandReply { reply },
    )
    .await
}

pub(super) async fn serve(
    connection: &mut ClientConnection,
    shared: &Shared,
    request_id: Option<uuid::Uuid>,
    command: RemoteCommandSpec,
    target: TerminalId,
    size: TerminalSize,
    form: Option<std::collections::BTreeMap<String, String>>,
) -> Result<()> {
    let result = serve_inner(connection, shared, request_id, command, target, size, form).await;
    if let Err(error) = result {
        reply(
            connection,
            request_id,
            RemoteCommandReply::Error {
                message: format!("{error:#}"),
            },
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn defaults(
    connection: &mut ClientConnection,
    shared: &Shared,
    request_id: Option<uuid::Uuid>,
    command: RemoteCommandSpec,
    target: TerminalId,
) -> Result<()> {
    let result = async {
        anyhow::ensure!(
            request_id.is_some(),
            "command defaults require a request ID"
        );
        prepare(shared, command, target, None).await?.defaults()
    }
    .await;
    let response = match result {
        Ok(values) => RemoteCommandReply::Defaults { values },
        Err(error) => RemoteCommandReply::Error {
            message: format!("{error:#}"),
        },
    };
    reply(connection, request_id, response).await
}

async fn serve_inner(
    connection: &mut ClientConnection,
    shared: &Shared,
    request_id: Option<uuid::Uuid>,
    command: RemoteCommandSpec,
    target: TerminalId,
    size: TerminalSize,
    form: Option<std::collections::BTreeMap<String, String>>,
) -> Result<()> {
    anyhow::ensure!(request_id.is_some(), "command start requires a request ID");
    size.validate_within(connection.max_visible_cells())?;
    let terminal = shared.lock().await.terminal_config.clone();
    let prepared = prepare(shared, command, target, form).await?;
    run(connection, request_id, prepared, terminal, size).await
}

async fn prepare(
    shared: &Shared,
    command: RemoteCommandSpec,
    target: TerminalId,
    form: Option<std::collections::BTreeMap<String, String>>,
) -> Result<PreparedCommand> {
    let (location, catalog, focused, root, project, socket) = {
        let state = shared.lock().await;
        anyhow::ensure!(state.accepting, "daemon is shutting down");
        let snapshot = state.resources.snapshot();
        let path = snapshot.live_terminal_path(target)?;
        let runtime = state
            .runtimes
            .get(&target)
            .context("target terminal runtime not found")?;
        anyhow::ensure!(
            !matches!(
                *runtime.handle.subscribe_lifecycle().borrow(),
                TerminalLifecycle::Exited { .. }
            ),
            "target terminal has exited"
        );
        let focused = selected_target(path, &runtime.handle);
        let root = state
            .resources
            .workspace_root(path.workspace_id)?
            .to_owned();
        let project = snapshot
            .sessions
            .iter()
            .find(|s| s.id == path.session_id)
            .and_then(|s| s.trusted_project_config.clone());
        let socket = PathBuf::from(
            state
                .child_env
                .get(std::ffi::OsStr::new("FUT_SOCKET"))
                .context("daemon socket path is missing")?,
        );
        let catalog = state.extension_catalog.borrow().clone();
        (
            state.config_location.clone(),
            catalog,
            focused,
            root,
            project,
            socket,
        )
    };
    PreparedCommand::prepare(
        command, location, catalog, focused, root, project, socket, form,
    )
    .await
}

async fn run(
    connection: &mut ClientConnection,
    request_id: Option<uuid::Uuid>,
    prepared: PreparedCommand,
    terminal: crate::terminal::TerminalConfig,
    size: TerminalSize,
) -> Result<()> {
    if prepared.background() {
        let run = prepared.run_background();
        tokio::pin!(run);
        loop {
            tokio::select! {
                done = &mut run => return reply(connection, request_id, done).await,
                frame = connection.next() => {
                    let Some(frame) = frame else { return Ok(()) };
                    let envelope: Envelope<ClientMessage> = decode_payload(&frame?)?;
                    if reject_remote_method(connection, &envelope).await? { continue; }
                    if matches!(envelope.message, ClientMessage::Detach) { return Ok(()); }
                    reply(connection, envelope.request_id, RemoteCommandReply::Error {
                        message: "background command does not accept operations".into(),
                    }).await?;
                }
            }
        }
    }
    let mut surface = prepared.spawn(terminal, size).await?;
    reply(connection, request_id, surface.ready()).await?;
    loop {
        tokio::select! {
            (update, done) = surface.update() => {
                reply(connection, None, update).await?;
                if done { return Ok(()); }
            }
            frame = connection.next() => {
                let Some(frame) = frame else { return Ok(()) };
                let envelope: Envelope<ClientMessage> = decode_payload(&frame?)?;
                if reject_remote_method(connection, &envelope).await? { continue; }
                if matches!(envelope.message, ClientMessage::Detach) { return Ok(()); }
                let mut screen_changed = false;
                let response = if envelope.request_id.is_none() {
                    RemoteCommandReply::Error { message: "command operation requires a request ID".into() }
                } else if let ClientMessage::CommandOperation { operation } = envelope.message {
                    use crate::protocol::RemoteCommandOperation;
                    screen_changed = matches!(&operation,
                        RemoteCommandOperation::Mouse { .. }
                        | RemoteCommandOperation::FinishClipboard { .. }
                        | RemoteCommandOperation::CancelMouseCopy);
                    match surface.operation(operation, connection.max_visible_cells()).await {
                        Ok(ack) => ack,
                        Err(error) => RemoteCommandReply::Error { message: format!("{error:#}") },
                    }
                } else {
                    RemoteCommandReply::Error { message: "connection is dedicated to a configured command".into() }
                };
                reply(connection, envelope.request_id, response).await?;
                // Mouse/copy actions can update the viewport without changing
                // the canonical terminal snapshot watched by update().
                if screen_changed {
                    reply(connection, None, surface.screen()).await?;
                }
            }
        }
    }
}
