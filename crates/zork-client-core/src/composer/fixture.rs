//! In-memory story host using the production desktop composer capabilities.
//! Events model accepted intents only; no message is delivered or file uploaded.
use super::*;
use zork_client_types::composer::{Capabilities, Controller, File, Intent, Member, Snapshot};

pub struct Fixture {
    session: DesktopSession,
    state: Snapshot,
    stopping: bool,
    preparing: usize,
    next_file: u64,
}
impl Default for Fixture {
    fn default() -> Self {
        Self {
            session: DesktopSession {
                editable: true,
                can_stop: true,
                running: false,
            },
            state: Snapshot {
                members: vec![Member {
                    id: "fox".into(),
                    avatar: "fox".into(),
                    label: "准备就绪".into(),
                    active: false,
                }],
                ..Default::default()
            },
            stopping: false,
            preparing: 0,
            next_file: 0,
        }
    }
}
impl Fixture {
    fn state(&mut self) -> Snapshot {
        let c = desktop_state(
            Some(self.session),
            &self.state.text,
            self.state.files.len(),
            self.stopping,
            self.preparing,
        );
        self.state.capabilities = Capabilities {
            editable: c.editable,
            stop: c.stop,
            enabled: c.enabled,
        };
        self.state.clone()
    }
    fn submit(&mut self) {
        if self.session.editable
            && self.preparing == 0
            && has_content(&self.state.text, self.state.files.len())
        {
            self.state.events.push(format!(
                "send:{}:{}",
                self.state.text,
                self.state.files.len()
            ));
            self.state.text.clear();
            self.state.files.clear();
        }
    }
}
impl Controller for Fixture {
    fn apply(&mut self, intent: Intent) -> Snapshot {
        match intent {
            Intent::Inspect => {}
            Intent::Edit(text) => {
                if self.session.editable {
                    self.state.text = text;
                }
            }
            Intent::Submit => self.submit(),
            Intent::Primary => {
                let c = self.state().capabilities;
                if c.enabled {
                    if c.stop {
                        self.state.events.push("stop".into());
                        self.session.running = false;
                        self.stopping = false;
                        for member in &mut self.state.members {
                            member.active = false;
                            member.label = "准备就绪".into();
                        }
                    } else {
                        self.submit();
                    }
                }
            }
            Intent::Files(names) => {
                if self.session.editable {
                    for name in names {
                        self.next_file += 1;
                        self.state.files.push(File {
                            id: self.next_file,
                            name,
                        });
                    }
                }
            }
            Intent::RemoveFile(id) => self.state.files.retain(|file| file.id != id),
            Intent::Scenario(index) => {
                *self = Self::default();
                match index {
                    1 => self.state.text = "把这些组件整理成可复用的 Rust 实现。".into(),
                    2 | 3 | 6 => {
                        self.session.running = true;
                        self.state.members[0].active = true;
                        self.state.members[0].label = "正在检查共享组件".into();
                        self.stopping = index == 3;
                        if index == 6 {
                            for (id, label) in
                                [("panda", "正在整理验证结果"), ("octopus", "正在审阅交互")]
                            {
                                self.state.members.push(Member {
                                    id: id.into(),
                                    avatar: id.into(),
                                    label: label.into(),
                                    active: true,
                                });
                            }
                        }
                    }
                    4 => self.session.editable = false,
                    5 => {
                        self.preparing = 1;
                        self.state.text = "附件准备中…".into();
                    }
                    7 => {
                        self.session.running = true;
                        self.session.can_stop = false;
                        self.state.text = "补充任务评论".into();
                    }
                    _ => {}
                }
            }
        }
        self.state()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_uses_desktop_policy_for_content_running_tasks_and_enter() {
        let mut f = Fixture::default();
        assert!(!f.apply(Intent::Inspect).capabilities.enabled);
        assert!(
            f.apply(Intent::Files(vec!["note.md".into()]))
                .capabilities
                .enabled
        );
        assert!(f.apply(Intent::Primary).files.is_empty());
        assert!(f.apply(Intent::Scenario(2)).capabilities.stop);
        f.apply(Intent::Edit("追加消息".into()));
        let s = f.apply(Intent::Submit);
        assert!(s.capabilities.stop);
        assert_eq!(s.events, vec!["send:追加消息:0"]);
        assert!(!f.apply(Intent::Primary).capabilities.stop);
        assert!(!f.apply(Intent::Scenario(3)).capabilities.enabled);
        assert!(!f.apply(Intent::Scenario(4)).capabilities.editable);
        assert!(!f.apply(Intent::Scenario(5)).capabilities.enabled);
        let task = f.apply(Intent::Scenario(7));
        assert!(!task.capabilities.stop && task.capabilities.enabled);
    }
}
