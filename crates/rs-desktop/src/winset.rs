//! 会话窗口表：一台 Host 至多一个会话窗口；记录聚焦的窗口，菜单命令发给它。

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenDecision {
    FocusExisting(u64),
    OpenNew,
}

pub struct WindowSet<W: Clone> {
    windows: BTreeMap<String, (u64, W)>,
    focused: Option<u64>,
}

impl<W: Clone> WindowSet<W> {
    pub fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            focused: None,
        }
    }

    pub fn decide(&self, host_id: &str) -> OpenDecision {
        self.windows
            .get(host_id)
            .map_or(OpenDecision::OpenNew, |(session_id, _)| {
                OpenDecision::FocusExisting(*session_id)
            })
    }

    /// 同一 host_id 已存在时返回 Err(已有的 session_id)，不覆盖。
    pub fn insert(&mut self, host_id: &str, session_id: u64, window: W) -> Result<(), u64> {
        if let Some((existing, _)) = self.windows.get(host_id) {
            return Err(*existing);
        }
        self.windows
            .insert(host_id.to_owned(), (session_id, window));
        Ok(())
    }

    pub fn remove_session(&mut self, session_id: u64) -> Option<W> {
        let host = self
            .windows
            .iter()
            .find_map(|(host, (id, _))| (*id == session_id).then(|| host.clone()))?;
        let (_, window) = self.windows.remove(&host)?;
        if self.focused == Some(session_id) {
            self.focused = None;
        }
        Some(window)
    }

    pub fn get_by_session(&self, session_id: u64) -> Option<W> {
        self.windows
            .values()
            .find_map(|(id, window)| (*id == session_id).then(|| window.clone()))
    }

    pub fn session_for_host(&self, host_id: &str) -> Option<u64> {
        self.windows.get(host_id).map(|(id, _)| *id)
    }

    /// 窗口回报 Focus(true) 时设置，Focus(false) 时只在它是当前聚焦窗口时清空。
    pub fn set_focused(&mut self, session_id: u64, focused: bool) {
        if focused {
            if self.windows.values().any(|(id, _)| *id == session_id) {
                self.focused = Some(session_id);
            }
        } else if self.focused == Some(session_id) {
            self.focused = None;
        }
    }

    pub fn focused(&self) -> Option<(u64, W)> {
        let session_id = self.focused?;
        self.get_by_session(session_id)
            .map(|window| (session_id, window))
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn sessions(&self) -> Vec<u64> {
        self.windows.values().map(|(id, _)| *id).collect()
    }
}

impl<W: Clone> Default for WindowSet<W> {
    fn default() -> Self {
        Self::new()
    }
}
