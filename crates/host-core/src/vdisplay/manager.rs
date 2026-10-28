//! 虚拟屏生命周期（M3 · V1）：归属、认领、销毁。纯同步，时间由调用方传入（微秒）。

use super::{VdCreated, VdError};
use crate::{platform::DisplayInfo, session::roster::ConnKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub per_owner: usize,
    pub total: usize,
    pub appear_timeout_us: u64,
    pub vanish_grace_us: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            per_owner: 1,
            total: 2,
            appear_timeout_us: 10_000_000,
            vanish_grace_us: 3_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket(pub u64);

/// 与 pb::VirtualDisplayResult.Status 一一对应（转换由会话处理器完成，本模块不依赖 pb）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdStatus {
    Ok,
    Unsupported,
    NotAllowed,
    Limit,
    BadRequest,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begin {
    /// 需要调后端 create；结果用 created(ticket, …) 交回。
    Create(Ticket),
    /// 该会话已有一块活着的虚拟屏：直接回 OK。
    Existing {
        platform_id: u32,
    },
    Reject {
        status: VdStatus,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdEvent {
    /// 显示器已被认领：给 conn 回 OK；select 为真时随后切屏。
    Ready {
        conn: ConnKey,
        request_id: u32,
        select: bool,
        platform_id: u32,
    },
    /// 给 conn 回失败。
    Failed {
        conn: ConnKey,
        request_id: u32,
        status: VdStatus,
        message: String,
    },
    /// 调后端 destroy(token)。
    Destroy { token: u64 },
}

#[derive(Debug)]
struct Entry {
    owner: String,
    label: String,
    conn: ConnKey,
    request_id: u32,
    select: bool,
    state: EntryState,
}

#[derive(Debug)]
enum EntryState {
    Creating {
        ticket: Ticket,
        orphan: bool,
    },
    Appearing {
        token: u64,
        platform_id: Option<u32>,
        match_name: Option<String>,
        deadline_us: u64,
    },
    Active {
        token: u64,
        platform_id: u32,
        missing_since_us: Option<u64>,
    },
}

pub struct VdManager {
    limits: Limits,
    next_ticket: u64,
    entries: Vec<Entry>,
}

pub fn status_of(error: &VdError) -> VdStatus {
    match error {
        VdError::Unsupported(_) => VdStatus::Unsupported,
        VdError::Limit => VdStatus::Limit,
        VdError::Failed(_) => VdStatus::Failed,
    }
}

impl VdManager {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            next_ticket: 1,
            entries: Vec::new(),
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn begin(
        &mut self,
        owner: &str,
        label: &str,
        conn: ConnKey,
        request_id: u32,
        select: bool,
    ) -> Begin {
        for entry in &self.entries {
            if entry.owner != owner {
                continue;
            }
            match entry.state {
                EntryState::Active { platform_id, .. } => {
                    return Begin::Existing { platform_id };
                }
                EntryState::Creating { .. } | EntryState::Appearing { .. } => {
                    return Begin::Reject {
                        status: VdStatus::Limit,
                        message: "a virtual display is already being created".to_owned(),
                    };
                }
            }
        }

        let owner_count = self
            .entries
            .iter()
            .filter(|entry| {
                entry.owner == owner
                    && !matches!(entry.state, EntryState::Creating { orphan: true, .. })
            })
            .count();
        if owner_count >= self.limits.per_owner || self.entries.len() >= self.limits.total {
            return Begin::Reject {
                status: VdStatus::Limit,
                message: "virtual display limit reached".to_owned(),
            };
        }

        let ticket = Ticket(self.next_ticket);
        self.next_ticket = self.next_ticket.saturating_add(1);
        self.entries.push(Entry {
            owner: owner.to_owned(),
            label: label.to_owned(),
            conn,
            request_id,
            select,
            state: EntryState::Creating {
                ticket,
                orphan: false,
            },
        });
        Begin::Create(ticket)
    }

    pub fn created(
        &mut self,
        ticket: Ticket,
        result: Result<VdCreated, VdError>,
        current: &[DisplayInfo],
        now_us: u64,
    ) -> Vec<VdEvent> {
        let Some(index) = self.entries.iter().position(|entry| {
            matches!(entry.state, EntryState::Creating { ticket: entry_ticket, .. } if entry_ticket == ticket)
        }) else {
            return match result {
                Ok(created) => vec![VdEvent::Destroy {
                    token: created.token,
                }],
                Err(_) => Vec::new(),
            };
        };

        let entry = self.entries.get(index);
        let (conn, request_id, select, orphan) = match entry {
            Some(entry) => (
                entry.conn,
                entry.request_id,
                entry.select,
                matches!(entry.state, EntryState::Creating { orphan: true, .. }),
            ),
            None => return Vec::new(),
        };

        match result {
            Err(error) => {
                self.entries.remove(index);
                if orphan {
                    Vec::new()
                } else {
                    vec![VdEvent::Failed {
                        conn,
                        request_id,
                        status: status_of(&error),
                        message: error.to_string(),
                    }]
                }
            }
            Ok(created) if orphan => {
                self.entries.remove(index);
                vec![VdEvent::Destroy {
                    token: created.token,
                }]
            }
            Ok(created) if created.platform_id.is_none() && created.match_name.is_none() => {
                self.entries.remove(index);
                vec![
                    VdEvent::Destroy {
                        token: created.token,
                    },
                    VdEvent::Failed {
                        conn,
                        request_id,
                        status: VdStatus::Failed,
                        message: "backend returned no display identity".to_owned(),
                    },
                ]
            }
            Ok(created) => {
                let deadline_us = now_us.saturating_add(self.limits.appear_timeout_us);
                if let Some(entry) = self.entries.get_mut(index) {
                    entry.state = EntryState::Appearing {
                        token: created.token,
                        platform_id: created.platform_id,
                        match_name: created.match_name,
                        deadline_us,
                    };
                } else {
                    return vec![VdEvent::Destroy {
                        token: created.token,
                    }];
                }

                let Some(platform_id) = self.find_claim(index, current) else {
                    return Vec::new();
                };
                self.activate(index, platform_id);
                vec![VdEvent::Ready {
                    conn,
                    request_id,
                    select,
                    platform_id,
                }]
            }
        }
    }

    pub fn on_displays(&mut self, current: &[DisplayInfo], now_us: u64) -> Vec<VdEvent> {
        let mut events_by_entry = (0..self.entries.len())
            .map(|_| Vec::<VdEvent>::new())
            .collect::<Vec<_>>();

        // Claims happen before disappearance bookkeeping. This also means an
        // Active entry can free a name only on the next display update.
        let mut index = 0;
        while index < self.entries.len() {
            let Some(entry) = self.entries.get(index) else {
                index = index.saturating_add(1);
                continue;
            };
            if !matches!(entry.state, EntryState::Appearing { .. }) {
                index = index.saturating_add(1);
                continue;
            }
            let Some(platform_id) = self.find_claim(index, current) else {
                index = index.saturating_add(1);
                continue;
            };
            let conn = entry.conn;
            let request_id = entry.request_id;
            let select = entry.select;
            self.activate(index, platform_id);
            events_by_entry[index].push(VdEvent::Ready {
                conn,
                request_id,
                select,
                platform_id,
            });
            index = index.saturating_add(1);
        }

        let mut remove = vec![false; self.entries.len()];
        for (index, entry_events) in events_by_entry.iter_mut().enumerate() {
            let Some(entry) = self.entries.get_mut(index) else {
                continue;
            };
            let EntryState::Active {
                token,
                platform_id,
                missing_since_us,
            } = &mut entry.state
            else {
                continue;
            };
            if current.iter().any(|display| display.id == *platform_id) {
                *missing_since_us = None;
                continue;
            }
            if missing_since_us.is_none() {
                *missing_since_us = Some(now_us);
            }
            if missing_since_us
                .map(|missing| missing.saturating_add(self.limits.vanish_grace_us) <= now_us)
                .unwrap_or(false)
            {
                remove[index] = true;
                entry_events.push(VdEvent::Destroy { token: *token });
            }
        }

        self.remove_marked(remove, events_by_entry)
    }

    pub fn tick(&mut self, live: &dyn Fn(&str) -> bool, now_us: u64) -> Vec<VdEvent> {
        let mut events_by_entry = (0..self.entries.len())
            .map(|_| Vec::<VdEvent>::new())
            .collect::<Vec<_>>();
        let mut remove = vec![false; self.entries.len()];

        for index in 0..self.entries.len() {
            let Some(entry) = self.entries.get_mut(index) else {
                continue;
            };
            let owner_live = live(&entry.owner);
            match &mut entry.state {
                EntryState::Creating { orphan, .. } => {
                    if !owner_live {
                        *orphan = true;
                    }
                }
                EntryState::Appearing {
                    token, deadline_us, ..
                } => {
                    if !owner_live {
                        remove[index] = true;
                        events_by_entry[index].push(VdEvent::Destroy { token: *token });
                    } else if now_us >= *deadline_us {
                        remove[index] = true;
                        events_by_entry[index].extend([
                            VdEvent::Destroy { token: *token },
                            VdEvent::Failed {
                                conn: entry.conn,
                                request_id: entry.request_id,
                                status: VdStatus::Failed,
                                message: "virtual display did not appear in time".to_owned(),
                            },
                        ]);
                    }
                }
                EntryState::Active {
                    token,
                    missing_since_us,
                    ..
                } => {
                    if !owner_live
                        || missing_since_us
                            .map(|missing| {
                                missing.saturating_add(self.limits.vanish_grace_us) <= now_us
                            })
                            .unwrap_or(false)
                    {
                        remove[index] = true;
                        events_by_entry[index].push(VdEvent::Destroy { token: *token });
                    }
                }
            }
        }

        self.remove_marked(remove, events_by_entry)
    }

    pub fn release(&mut self, owner: &str, platform_id: u32) -> Result<Vec<VdEvent>, VdStatus> {
        let Some(index) = self.entries.iter().position(|entry| {
            matches!(entry.state, EntryState::Active { platform_id: id, .. } if id == platform_id)
        }) else {
            return Err(VdStatus::BadRequest);
        };
        if self.entries.get(index).map(|entry| entry.owner.as_str()) != Some(owner) {
            return Err(VdStatus::NotAllowed);
        }
        let token = match self.entries.get(index).map(|entry| &entry.state) {
            Some(EntryState::Active { token, .. }) => *token,
            _ => return Err(VdStatus::BadRequest),
        };
        self.entries.remove(index);
        Ok(vec![VdEvent::Destroy { token }])
    }

    /// 已认领（Active）的 (platform_id, label)，按 platform_id 升序。
    pub fn virtual_names(&self) -> Vec<(u32, String)> {
        let mut names = self
            .entries
            .iter()
            .filter_map(|entry| match entry.state {
                EntryState::Active { platform_id, .. } => Some((platform_id, entry.label.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        names.sort_by_key(|(platform_id, _)| *platform_id);
        names
    }

    pub fn owner_of(&self, platform_id: u32) -> Option<&str> {
        self.entries.iter().find_map(|entry| match entry.state {
            EntryState::Active {
                platform_id: id, ..
            } if id == platform_id => Some(entry.owner.as_str()),
            _ => None,
        })
    }

    /// Host 停止时移除所有已经拿到后端 token 的条目；尚未返回结果的
    /// Creating 条目标记为 orphan，稍后收到结果时再销毁。
    pub fn drain_all(&mut self) -> Vec<u64> {
        let mut tokens = Vec::new();
        let mut retained = Vec::with_capacity(self.entries.len());
        for mut entry in self.entries.drain(..) {
            match &mut entry.state {
                EntryState::Creating { orphan, .. } => {
                    *orphan = true;
                    retained.push(entry);
                }
                EntryState::Appearing { token, .. } | EntryState::Active { token, .. } => {
                    tokens.push(*token);
                }
            }
        }
        self.entries = retained;
        tokens
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn find_claim(&self, index: usize, current: &[DisplayInfo]) -> Option<u32> {
        let entry = self.entries.get(index)?;
        let EntryState::Appearing {
            platform_id,
            match_name,
            ..
        } = &entry.state
        else {
            return None;
        };
        if let Some(platform_id) = platform_id {
            return current
                .iter()
                .find(|display| display.id == *platform_id)
                .map(|display| display.id);
        }
        let match_name = match_name.as_deref()?;
        current
            .iter()
            .find(|display| {
                display.name == match_name
                    && !self.entries.iter().enumerate().any(|(other_index, other)| {
                        other_index != index
                            && matches!(
                                other.state,
                                EntryState::Active {
                                    platform_id,
                                    ..
                                } if platform_id == display.id
                            )
                    })
            })
            .map(|display| display.id)
    }

    fn activate(&mut self, index: usize, platform_id: u32) {
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        let token = match entry.state {
            EntryState::Appearing { token, .. } => token,
            _ => return,
        };
        entry.state = EntryState::Active {
            token,
            platform_id,
            missing_since_us: None,
        };
    }

    fn remove_marked(
        &mut self,
        remove: Vec<bool>,
        events_by_entry: Vec<Vec<VdEvent>>,
    ) -> Vec<VdEvent> {
        let mut events = Vec::new();
        let mut retained = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.drain(..).enumerate() {
            if let Some(entry_events) = events_by_entry.get(index) {
                events.extend(entry_events.iter().cloned());
            }
            if !remove.get(index).copied().unwrap_or(false) {
                retained.push(entry);
            }
        }
        self.entries = retained;
        events
    }
}
