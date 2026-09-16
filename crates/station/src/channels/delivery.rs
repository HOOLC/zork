//! Domain adapters for zork-notify and Mesh Feed; no channel-specific poll loop.
use super::*;
use crate::db::chats::{Notice, NoticePage, Topic};
use std::collections::HashMap;
use zork_mesh::feed::{Event as MeshEvent, Watch};
use zork_notify::{
    stream::{Event, Page, Source},
    Task,
};

struct NoticeSource {
    state: AppState,
    recipient: String,
    epoch: Option<String>,
    after: i64,
}
impl Source for NoticeSource {
    type Item = NoticePage;
    type Error = anyhow::Error;
    fn check_access(&self) -> Result<()> {
        if self.recipient != "local" {
            access(&self.state, &self.recipient)?;
        }
        Ok(())
    }
    async fn read(&mut self) -> Result<Page<NoticePage>> {
        self.check_access()?;
        let page =
            self.state
                .db
                .chat_notice_page(&self.recipient, self.epoch.as_deref(), self.after)?;
        if page.items.is_empty() {
            Ok(Page::snapshot(page))
        } else {
            let more = page.more;
            Ok(Page::chunk(page, more, false))
        }
    }
    fn delivered(&mut self, page: &NoticePage) {
        self.after = page.through;
        self.epoch = Some(page.epoch.clone());
    }
}
pub fn subscribe(
    state: AppState,
    recipient: String,
    epoch: Option<String>,
    after: i64,
) -> Result<tokio::sync::mpsc::Receiver<Value>> {
    ensure!(after >= 0, "invalid_mesh_cursor");
    let changes = state
        .db
        .chat_topics
        .subscribe([Topic::Recipient(recipient.clone())])
        .merge(state.db.realtime.listen(crate::realtime::MESH));
    let source = NoticeSource {
        state,
        recipient,
        epoch,
        after,
    };
    source.check_access()?;
    Ok(zork_notify::stream::spawn(
        source,
        changes,
        8,
        Some(zork_mesh::feed::HEARTBEAT),
        |event| {
            Some(match event {
                Event::Data(value) => json!({"v":1,"ok":true,"data":value}),
                Event::Heartbeat => json!({"v":1,"ok":true,"heartbeat":true}),
                Event::Error(error) => json!({"v":1,"ok":false,"error":super::error(&error)}),
            })
        },
    ))
}

pub fn start(state: AppState) -> Result<Task<()>> {
    state.db.record_chat_source("*", "local")?;
    Ok(Task(tokio::spawn(async move {
        let _mailbox = Task(tokio::spawn(dispatch_mailbox(state.clone())));
        let mut changes = state
            .db
            .chat_topics
            .subscribe([Topic::Sources])
            .merge(state.db.realtime.listen(crate::realtime::MESH));
        let mut sources: HashMap<String, Task<()>> = HashMap::new();
        loop {
            changes.checkpoint();
            match state.db.chat_sources() {
                Ok(rows) => {
                    let targets = rows
                        .iter()
                        .filter(|(_, target, _, _)| {
                            target == "local"
                                || (state.mesh.get().is_some() && access(&state, target).is_ok())
                        })
                        .map(|(_, target, _, _)| target.clone())
                        .collect::<Vec<_>>();
                    sources.retain(|target, _| targets.contains(target));
                    for (_, target, epoch, after) in rows {
                        if !targets.contains(&target)
                            || sources.get(&target).is_some_and(|task| !task.is_finished())
                        {
                            continue;
                        }
                        let state = state.clone();
                        let selected = target.clone();
                        sources.insert(target,Task(tokio::spawn(async move{
                            if let Err(error)=receive(state,&selected,epoch,after).await{tracing::warn!(target=%selected,error=%super::error(&error),"Channel source stopped");}
                        })));
                    }
                }
                Err(error) => {
                    tracing::warn!(error=%super::error(&error),"Channel source catalog unavailable")
                }
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    })))
}

async fn receive(state: AppState, target: &str, epoch: Option<String>, after: i64) -> Result<()> {
    if target == "local" {
        let mut source = subscribe(state.clone(), "local".into(), epoch, after)?;
        while let Some(value) = source.recv().await {
            ensure!(value["ok"] == true, "chat_source_failed");
            if value["heartbeat"] == true {
                continue;
            }
            let page: NoticePage = serde_json::from_value(value["data"].clone())?;
            state
                .db
                .accept_chat_notices(&node_access::identity(&state), target, &page)?;
        }
        return Ok(());
    }
    let service = state.mesh.get().context("node_starting")?;
    let mut source = service.follow_channel_messages(target, epoch, after);
    while let Some(event) = source.next().await {
        match event {
            MeshEvent::Data(value) => {
                access(&state, target)?;
                let page: NoticePage = serde_json::from_value(value)?;
                state
                    .db
                    .accept_chat_notices(&node_access::identity(&state), target, &page)?;
                // This is the receiver's committed cursor, not the UI applied
                // version and not the publisher's transport queue cursor.
                source.resume_with(Watch::AgentMessages {
                    epoch: Some(page.epoch),
                    after: page.through,
                })?;
            }
            MeshEvent::Disconnected { error, terminal } => {
                if terminal {
                    anyhow::bail!(error)
                }
                tracing::debug!(target, error, "Channel feed reconnecting");
            }
        }
    }
    Ok(())
}

async fn dispatch_mailbox(state: AppState) {
    let mut changes = state.db.chat_topics.subscribe([Topic::Mailbox]);
    let mut running = tokio::task::JoinSet::new();
    let mut active = HashMap::new();
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        changes.checkpoint();
        match state.db.pending_chat_agents() {
            Ok(agents) => {
                for agent in agents {
                    if active.contains_key(&agent) {
                        continue;
                    }
                    if active.len() >= 64 {
                        break;
                    }
                    let state = state.clone();
                    let slots = slots.clone();
                    let id = agent.clone();
                    let handle = running.spawn(async move {
                        dispatch_agent(state, &id, slots).await;
                        id
                    });
                    active.insert(agent, handle);
                }
            }
            Err(error) => {
                tracing::warn!(error=%super::error(&error),"Agent input catalog unavailable");
                tokio::select! {_=retry.wait()=>{},_=changes.changed()=>{}}
                continue;
            }
        }
        tokio::select! {
            changed=changes.changed()=>if changed.is_err(){return},
            result=running.join_next(),if !running.is_empty()=>match result{
                Some(Ok(agent))=>{active.remove(&agent);retry.reset();},
                Some(Err(error))=>{
                    active.retain(|_,handle|!handle.is_finished());
                    tracing::warn!(%error,"Agent input worker stopped");retry.wait().await;
                },
                None=>{}
            }
        }
    }
}

async fn attempt(
    slots: &tokio::sync::Semaphore,
    work: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let _permit = slots.acquire().await.context("channel_delivery_closed")?;
        work.await
    })
    .await
    .context("channel_delivery_timeout")?
}

async fn dispatch_agent(
    state: AppState,
    agent: &str,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
) {
    let mut changes =
        state
            .db
            .chat_topics
            .subscribe([Topic::AgentInput(agent.into())])
            .merge(state.db.realtime.listen(
                crate::realtime::AGENTS | crate::realtime::MESH | crate::realtime::PROFILES,
            ));
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        changes.checkpoint();
        let mut failed = false;
        let mut progress = false;
        match state.db.direct_agent_inputs(agent) {
            Ok(inputs) => {
                for (sequence, agent, author, content, epoch) in inputs {
                    let result = attempt(&slots, async {
                        let author: Subject = serde_json::from_str(&author)?;
                        if author.origin != "local" {
                            access(&state, &author.origin)?;
                        }
                        let session = agents::ensure_runtime(&state, &agent).await?;
                        state
                            .agent
                            .append_ordered_mailbox(
                                session,
                                format!("agent-direct-{epoch}"),
                                sequence.try_into()?,
                                json!({"source":"agent","author":author,"text":content})
                                    .to_string(),
                                true,
                            )
                            .await?;
                        state.db.finish_direct_agent_input(sequence)?;
                        Ok::<_, anyhow::Error>(())
                    })
                    .await;
                    match result {
                        Ok(()) => progress = true,
                        Err(error) => {
                            failed = true;
                            tracing::debug!(agent,error=%super::error(&error),"Direct Agent input awaits retry");
                        }
                    }
                }
            }
            Err(error) => {
                failed = true;
                tracing::warn!(error=%super::error(&error),"Direct Agent mailbox unavailable");
            }
        }
        match state.db.pending_chat_inputs_for(Some(agent)) {
            Ok(inputs) => {
                for (id, agent, target, notice) in inputs {
                    match attempt(&slots, deliver(&state, &agent, &target, &notice)).await {
                        Ok(()) => match state.db.finish_chat_input(&id) {
                            Ok(()) => progress = true,
                            Err(error) => {
                                failed = true;
                                tracing::warn!(error=%super::error(&error),"Channel receipt persistence failed");
                            }
                        },
                        Err(error) => {
                            failed = true;
                            tracing::debug!(agent,target,error=%super::error(&error),"Channel input awaits retry");
                        }
                    }
                }
            }
            Err(error) => {
                failed = true;
                tracing::warn!(error=%super::error(&error),"Channel mailbox unavailable");
            }
        }
        if progress {
            retry.reset();
            tokio::task::yield_now().await;
            continue;
        }
        if failed {
            tokio::select! {changed=changes.changed()=>if changed.is_err(){return},_=retry.wait()=>{}}
        } else {
            // The manager observes worker completion as well as new commits,
            // so input racing with this idle exit cannot be lost.
            return;
        }
    }
}

async fn deliver(state: &AppState, agent: &str, target: &str, notice: &Notice) -> Result<()> {
    if target != "local" {
        access(state, target)?;
    }
    if !state.db.accepts_chat_input(agent, target, notice)? {
        return Ok(());
    }
    // The initial work request is durably delivered by the assignment path.
    // A channel notice must never enqueue the same goal as a second turn.
    if notice.work.as_ref().is_some_and(|work| work.initial) {
        return Ok(());
    }
    let session = agents::ensure_chat_runtime(state, agent, target, notice).await?;
    if !state.db.accepts_chat_input(agent, target, notice)? {
        return Ok(());
    }
    let content = json!({"source":"chat","target":target,"message":notice.message});
    let source = format!("chat-{}-{}", &fingerprint(&target)?[..24], notice.epoch);
    state
        .agent
        .append_ordered_mailbox(
            session,
            source,
            notice
                .sequence
                .try_into()
                .context("invalid_chat_sequence")?,
            content.to_string(),
            notice.delivery == zork_client_types::chat::DeliveryMode::Immediate,
        )
        .await?;
    Ok(())
}
