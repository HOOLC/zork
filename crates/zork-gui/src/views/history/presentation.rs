//! Native composition of the shared reading-oriented history rows.
use super::*;
use zork_ui::history_page::Jump;

pub(in crate::views) use zork_ui::history_details::Presentation as DetailPresentation;

impl RootView {
    pub(in crate::views) fn refresh_history_sources(&mut self) {
        if !self.history.open {
            return;
        }
        let wanted: std::collections::HashSet<&str> = self
            .history
            .ui
            .entries
            .iter()
            .filter_map(|e| activity::input(e)?["request_id"].as_str())
            .collect();
        self.history.wanted_sources = wanted.iter().map(|id| (*id).to_owned()).collect();
        let mut sources = HashMap::new();
        if !wanted.is_empty() {
            let mut add = |line: &TranscriptLine| {
                let TranscriptLine::Message { role, metadata, .. } = line;
                let Some(id) = metadata.id.as_deref().filter(|id| wanted.contains(id)) else {
                    return;
                };
                let label = metadata
                    .author_name
                    .clone()
                    .or_else(|| metadata.author_agent_id.clone())
                    .unwrap_or_else(|| {
                        self.locale
                            .text(if *role == Role::User {
                                "history_user"
                            } else {
                                "history_source_unknown"
                            })
                            .into()
                    });
                sources.insert(id.to_owned(), (label, metadata.author_agent_id.clone()));
            };
            if self.transcript_lookup.len() == self.lines.len() {
                for id in &wanted {
                    if let Some(line) = self
                        .transcript_lookup
                        .index_of(id)
                        .and_then(|index| self.lines.get(index))
                    {
                        add(line);
                    }
                }
            } else {
                // Unbound preview fixtures have no core-built lookup.
                for line in self.lines.iter() {
                    add(line);
                }
            }
        }
        self.history.sources = sources;
    }

    pub(in crate::views) fn refresh_changed_message_sources(
        &mut self,
        edits: &[zork_client_core::observe::ListEdit<TranscriptLine>],
    ) {
        if !self.history.open {
            return;
        }
        let mut previous = self.lines.clone();
        for edit in edits {
            for line in previous.slice(edit.remove.clone()).iter() {
                let TranscriptLine::Message { metadata, .. } = line;
                if let Some(id) = &metadata.id {
                    self.history.sources.remove(id);
                }
            }
            for line in edit.insert.iter() {
                let TranscriptLine::Message { role, metadata, .. } = line;
                let Some(id) = metadata
                    .id
                    .as_ref()
                    .filter(|id| self.history.wanted_sources.contains(*id))
                else {
                    continue;
                };
                let label = metadata
                    .author_name
                    .clone()
                    .or_else(|| metadata.author_agent_id.clone())
                    .unwrap_or_else(|| {
                        self.locale
                            .text(if *role == Role::User {
                                "history_user"
                            } else {
                                "history_source_unknown"
                            })
                            .into()
                    });
                self.history
                    .sources
                    .insert(id.clone(), (label, metadata.author_agent_id.clone()));
            }
            edit.apply(&mut previous);
        }
    }

    pub(super) fn activity_subject(
        &self,
        a: &Activity,
        e: &Entry,
    ) -> (Option<String>, Option<Jump>) {
        if a.kind == Kind::Input && a.subject.is_none() {
            let receipt = activity::input(e).and_then(|input| input["request_id"].as_str());
            if receipt.is_some_and(|id| id.starts_with("assignment-") || id.starts_with("rework-"))
            {
                let session = self
                    .history
                    .session
                    .as_ref()
                    .or(self.selected_session.as_ref());
                if let Some((leader, _)) = self.tasks_by_leader.iter().find(|(_, tasks)| {
                    session.is_some_and(|session| {
                        tasks.iter().any(|t| t.session_id.as_ref() == Some(session))
                    })
                }) {
                    let agent = self.node_agents.iter().find(|a| a["id"] == *leader);
                    return (
                        Some(
                            agent
                                .and_then(|a| a["name"].as_str())
                                .unwrap_or(leader)
                                .to_owned(),
                        ),
                        agent.map(|_| Jump::Agent(leader.clone())),
                    );
                }
            }
            if let Some((label, agent)) = activity::input(e)
                .and_then(|input| input["request_id"].as_str())
                .and_then(|id| self.history.sources.get(id))
            {
                return (
                    Some(label.clone()),
                    agent
                        .as_ref()
                        .map(|id| Jump::Agent(id.clone()))
                        .or_else(|| {
                            self.history
                                .session
                                .as_ref()
                                .or(self.selected_session.as_ref())
                                .cloned()
                                .map(Jump::Conversation)
                        }),
                );
            }
            return (
                Some(self.locale.text("history_source_unknown").into()),
                None,
            );
        }
        match &a.subject {
            Some(Subject::User) => (Some(self.locale.text("history_user").into()), None),
            Some(Subject::Conversation) => {
                let session = self
                    .history
                    .session
                    .as_ref()
                    .or(self.selected_session.as_ref());
                let title = session
                    .and_then(|id| self.sessions.iter().find(|s| &s.session_id == id))
                    .and_then(|s| s.task.as_ref())
                    .map(|t| t.title.clone())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| self.locale.text("history_current_conversation").into());
                (Some(title), session.cloned().map(Jump::Conversation))
            }
            Some(Subject::Agent(id)) => {
                let agent = self.node_agents.iter().find(|agent| agent["id"] == *id);
                let participant = self.participants.iter().find(|p| &p.id == id);
                let name = agent
                    .and_then(|a| a["name"].as_str())
                    .map(str::to_owned)
                    .or_else(|| participant.map(|p| p.name.clone()))
                    .unwrap_or_else(|| id.clone());
                (
                    Some(name),
                    (agent.is_some() || participant.is_some()).then(|| Jump::Agent(id.clone())),
                )
            }
            Some(Subject::Task(id)) => {
                let task = self
                    .tasks_by_leader
                    .values()
                    .flatten()
                    .find(|t| &t.task_id == id);
                (
                    Some(task.map_or_else(|| id.clone(), |t| t.title.clone())),
                    task.and_then(|t| t.session_id.clone())
                        .map(Jump::Conversation),
                )
            }
            Some(Subject::Invocation(id)) => {
                let key = format!("tool:{id}");
                let entry = self.history.entries.iter().find(|e| e.id == key);
                (
                    Some(entry.map_or_else(|| id.clone(), |e| e.action.clone())),
                    entry.map(|e| Jump::Entry(e.id.clone())),
                )
            }
            Some(Subject::Slack { channel, thread }) => (
                Some(format!(
                    "{} / {}",
                    activity::preview(channel),
                    activity::preview(thread)
                )),
                None,
            ),
            Some(
                Subject::Source(label)
                | Subject::File(label)
                | Subject::Tool(label)
                | Subject::BrowserTab(label),
            ) => (Some(activity::preview(label)), None),
            None => (None, None),
        }
    }

    pub(super) fn jump_history_target(&mut self, jump: Jump, cx: &mut Context<Self>) {
        match jump {
            Jump::Conversation(session) => {
                self.history.detail = None;
                self.history.agent_detail = None;
                if self.selected_session.as_ref() == Some(&session) {
                    self.browser
                        .update(cx, |panel, cx| panel.close_native_page("history", cx));
                } else {
                    self.select_session(&session, cx);
                }
            }
            Jump::Agent(id) => {
                self.history.detail = None;
                self.history.agent_detail = Some(id);
            }
            Jump::Entry(id) => {
                let index = self.history.entries.iter().position(|e| e.id == id);
                if let Some(index) = index {
                    self.history_select(index, cx);
                }
            }
            Jump::File(id) => {
                self.history.detail = Some(id);
                self.history.agent_detail = None;
            }
        }
        zork_ui::components::region::invalidate_all(cx);
    }

    pub(in crate::views) fn history_detail_presentation(&self) -> Option<DetailPresentation> {
        if let Some(id) = &self.history.agent_detail {
            let agent = self.node_agents.iter().find(|a| a["id"] == *id);
            let participant = self.participants.iter().find(|p| p.id == *id);
            Some(DetailPresentation::Agent {
                id: id.clone(),
                name: agent
                    .and_then(|a| a["name"].as_str())
                    .or_else(|| participant.map(|p| p.name.as_str()))
                    .unwrap_or(id)
                    .to_owned(),
                avatar: agent
                    .and_then(|a| a["avatar"].as_str())
                    .or_else(|| participant.and_then(|p| p.avatar.as_deref()))
                    .map(str::to_owned),
                role: agent.and_then(|a| a["role"].as_str()).map(str::to_owned),
            })
        } else {
            self.history
                .detail
                .as_ref()
                .and_then(|id| self.history.entries.iter().find(|entry| &entry.id == id))
                .cloned()
                .map(DetailPresentation::Entry)
        }
    }

    pub(in crate::views) fn sync_history_details(&mut self, cx: &mut Context<Self>) {
        let presentation = self.history_detail_presentation();
        let resource = match &presentation {
            Some(DetailPresentation::Entry(entry)) => {
                zork_client_core::resources::history_target(entry).map(|target| {
                    let label = self
                        .locale
                        .text(
                            if matches!(
                                target.query,
                                zork_client_core::resources::Inspection::Mcp(_)
                            ) {
                                "tool_connections"
                            } else {
                                "device_services"
                            },
                        )
                        .to_owned();
                    let root = cx.entity().downgrade();
                    zork_ui::history_details::Resource {
                        label,
                        open: Rc::new(move |cx| {
                            let _ = root.update(cx, |_, cx| {
                                cx.emit(crate::views::InspectResource {
                                    target: target.clone(),
                                })
                            });
                        }),
                    }
                })
            }
            _ => None,
        };
        let locale = self.locale;
        self.history_details.update(cx, |view, cx| {
            view.configure(
                self.selected_session.clone().unwrap_or_default(),
                presentation,
                resource,
                zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into())),
                cx,
            )
        });
    }
}
