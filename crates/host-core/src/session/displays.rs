//! 显示器登记、线上 id、选择与 epoch。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rsp_core::pb;

use crate::platform::DisplayInfo;

/// A client request for the primary display rather than a concrete wire id.
pub const PRIMARY_ALIAS: u32 = 0;

/// Which parts of a display's description changed between two updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangeFlags {
    pub size: bool,
    pub scale: bool,
    pub rotation: bool,
    pub position: bool,
    pub primary: bool,
    pub refresh: bool,
    pub name: bool,
}

impl ChangeFlags {
    fn any(self) -> bool {
        self.size
            || self.scale
            || self.rotation
            || self.position
            || self.primary
            || self.refresh
            || self.name
    }
}

/// Changes between the previous and current display lists, keyed by wire id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayDiff {
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
    pub changed: Vec<(u32, ChangeFlags)>,
}

impl DisplayDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

#[derive(Debug)]
struct DisplayEntry {
    display: DisplayInfo,
    present: bool,
    disconnected_seq: Option<u64>,
}

/// Sticky platform-to-wire id assignments for the current host process.
#[derive(Debug, Default)]
pub struct DisplayRegistry {
    slots: HashMap<u8, DisplayEntry>,
    platform_wire: HashMap<u32, u8>,
    virtual_names: HashMap<u32, String>,
    next_disconnect_seq: u64,
}

impl DisplayRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the present display list and report the differences by wire id.
    pub fn update(&mut self, displays: Vec<DisplayInfo>) -> DisplayDiff {
        let mut seen = HashSet::new();
        let displays: Vec<DisplayInfo> = displays
            .into_iter()
            .filter(|display| seen.insert(display.id))
            .collect();
        let present: HashSet<u32> = displays.iter().map(|display| display.id).collect();
        let mut diff = DisplayDiff::default();

        let mut gone: Vec<u8> = self
            .slots
            .iter()
            .filter_map(|(&wire_id, entry)| {
                (entry.present && !present.contains(&entry.display.id)).then_some(wire_id)
            })
            .collect();
        gone.sort_unstable();
        for wire_id in gone {
            if let Some(entry) = self.slots.get_mut(&wire_id) {
                entry.present = false;
                entry.disconnected_seq = Some(self.next_disconnect_seq);
                self.next_disconnect_seq = self.next_disconnect_seq.wrapping_add(1);
                diff.removed.push(u32::from(wire_id));
            }
        }

        // Reinserted displays keep their id, so mark them present before a new
        // display can reclaim any disconnected slot.
        for display in &displays {
            let Some(&wire_id) = self.platform_wire.get(&display.id) else {
                continue;
            };
            let Some(entry) = self.slots.get_mut(&wire_id) else {
                continue;
            };
            if entry.present {
                let flags = changed_flags(&entry.display, display);
                if flags.any() {
                    diff.changed.push((u32::from(wire_id), flags));
                }
            } else {
                diff.added.push(u32::from(wire_id));
            }
            entry.display = display.clone();
            entry.present = true;
            entry.disconnected_seq = None;
        }

        let mut fresh: Vec<&DisplayInfo> = displays
            .iter()
            .filter(|display| !self.platform_wire.contains_key(&display.id))
            .collect();
        fresh.sort_by(|left, right| display_order(left, right));
        for display in fresh {
            let Some(wire_id) = self.allocate_wire_id() else {
                // A host cannot represent more than 255 simultaneous displays.
                continue;
            };
            self.platform_wire.insert(display.id, wire_id);
            self.slots.insert(
                wire_id,
                DisplayEntry {
                    display: display.clone(),
                    present: true,
                    disconnected_seq: None,
                },
            );
            diff.added.push(u32::from(wire_id));
        }

        diff.added.sort_unstable();
        diff.removed.sort_unstable();
        diff.changed.sort_unstable_by_key(|(wire_id, _)| *wire_id);
        diff
    }

    pub fn list(&self) -> pb::DisplayList {
        pb::DisplayList {
            displays: self.displays_pb(),
        }
    }

    /// Set the virtual display annotations keyed by platform display id.
    ///
    /// The annotations are kept separately from the platform-provided display
    /// metadata so a watch update can preserve the platform name while the
    /// wire representation exposes the host-owned virtual label.
    pub fn set_virtual(&mut self, entries: Vec<(u32, String)>) -> bool {
        let next = entries.into_iter().collect::<HashMap<_, _>>();
        if self.virtual_names == next {
            return false;
        }
        self.virtual_names = next;
        true
    }

    pub fn displays_pb(&self) -> Vec<pb::Display> {
        self.ordered()
            .into_iter()
            .map(|(wire_id, display)| {
                let virtual_name = self.virtual_names.get(&display.id);
                pb::Display {
                    id: wire_id,
                    name: virtual_name
                        .cloned()
                        .unwrap_or_else(|| display.name.clone()),
                    width_px: display.width_px,
                    height_px: display.height_px,
                    scale: display.scale,
                    x: display.x,
                    y: display.y,
                    refresh_hz: display.refresh_hz,
                    primary: display.primary,
                    rotation: display.rotation,
                    is_virtual: virtual_name.is_some(),
                }
            })
            .collect()
    }

    pub fn get(&self, wire_id: u32) -> Option<&DisplayInfo> {
        let wire_id = u8::try_from(wire_id).ok()?;
        let entry = self.slots.get(&wire_id)?;
        entry.present.then_some(&entry.display)
    }

    pub fn wire_id(&self, platform_id: u32) -> Option<u8> {
        self.platform_wire.get(&platform_id).copied()
    }

    pub fn platform_id(&self, wire_id: u32) -> Option<u32> {
        let wire_id = u8::try_from(wire_id).ok()?;
        self.slots.get(&wire_id).map(|entry| entry.display.id)
    }

    pub fn primary(&self) -> Option<u32> {
        self.ordered().first().map(|(wire_id, _)| *wire_id)
    }

    pub fn resolve(&self, requested: u32) -> Option<u32> {
        if requested == PRIMARY_ALIAS {
            return self.primary();
        }
        self.get(requested).map(|_| requested)
    }

    pub fn is_empty(&self) -> bool {
        !self.slots.values().any(|entry| entry.present)
    }

    fn ordered(&self) -> Vec<(u32, &DisplayInfo)> {
        let mut displays: Vec<(u32, &DisplayInfo)> = self
            .slots
            .iter()
            .filter_map(|(&wire_id, entry)| {
                entry
                    .present
                    .then_some((u32::from(wire_id), &entry.display))
            })
            .collect();
        displays.sort_by(|(left_wire, left), (right_wire, right)| {
            display_order(left, right).then_with(|| left_wire.cmp(right_wire))
        });
        displays
    }

    fn allocate_wire_id(&mut self) -> Option<u8> {
        if let Some(wire_id) = (1..=u8::MAX).find(|wire_id| !self.slots.contains_key(wire_id)) {
            return Some(wire_id);
        }
        self.reclaim_disconnected_wire_id()
    }

    fn reclaim_disconnected_wire_id(&mut self) -> Option<u8> {
        let wire_id = self
            .slots
            .iter()
            .filter(|(_, entry)| !entry.present)
            .min_by_key(|(&wire_id, entry)| (entry.disconnected_seq.unwrap_or(u64::MAX), wire_id))
            .map(|(&wire_id, _)| wire_id)?;
        let entry = self.slots.remove(&wire_id)?;
        self.platform_wire.remove(&entry.display.id);
        Some(wire_id)
    }
}

fn display_order(left: &DisplayInfo, right: &DisplayInfo) -> Ordering {
    right
        .primary
        .cmp(&left.primary)
        .then_with(|| left.x.cmp(&right.x))
        .then_with(|| left.y.cmp(&right.y))
        .then_with(|| left.id.cmp(&right.id))
}

fn changed_flags(previous: &DisplayInfo, current: &DisplayInfo) -> ChangeFlags {
    ChangeFlags {
        size: previous.width_px != current.width_px || previous.height_px != current.height_px,
        scale: previous.scale != current.scale,
        rotation: previous.rotation != current.rotation,
        position: previous.x != current.x || previous.y != current.y,
        primary: previous.primary != current.primary,
        refresh: previous.refresh_hz != current.refresh_hz,
        name: previous.name != current.name,
    }
}

/// Why the encoder must be rebuilt for the selected display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    Start,
    Selected,
    Resized,
    Rotated,
    Removed,
    Reappeared,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisplayAction {
    None,
    Rebuild {
        wire_id: u32,
        platform_id: u32,
        epoch: u32,
        display: DisplayInfo,
        reason: RebuildReason,
    },
    Suspend {
        epoch: u32,
    },
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SelectError {
    #[error("unknown display {0}")]
    UnknownDisplay(u32),
    #[error("no displays are available")]
    NoDisplays,
}

/// Per-connection display selection and stream epoch.
#[derive(Debug, Default)]
pub struct StreamDisplay {
    selected: Option<u32>,
    epoch: u32,
    suspended: bool,
    last_geometry: Option<DisplayGeometry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayGeometry {
    width_px: u32,
    height_px: u32,
    rotation: u32,
}

impl DisplayGeometry {
    fn new(display: &DisplayInfo) -> Self {
        Self {
            width_px: display.width_px,
            height_px: display.height_px,
            rotation: display.rotation,
        }
    }
}

impl StreamDisplay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(
        &mut self,
        registry: &DisplayRegistry,
        requested: u32,
    ) -> Result<DisplayAction, SelectError> {
        let (wire_id, platform_id, display) = resolve_selection(registry, requested)?;
        Ok(self.rebuild(wire_id, platform_id, display, RebuildReason::Start))
    }

    pub fn select(
        &mut self,
        registry: &DisplayRegistry,
        requested: u32,
    ) -> Result<DisplayAction, SelectError> {
        if self.selected.is_none() {
            return Ok(DisplayAction::None);
        }
        if registry.is_empty() {
            return Err(SelectError::NoDisplays);
        }
        let wire_id = match registry.resolve(requested) {
            Some(wire_id) => wire_id,
            None => return Err(SelectError::UnknownDisplay(requested)),
        };
        if self.selected == Some(wire_id) {
            return Ok(DisplayAction::None);
        }
        let platform_id = registry
            .platform_id(wire_id)
            .ok_or(SelectError::UnknownDisplay(requested))?;
        let display = registry
            .get(wire_id)
            .cloned()
            .ok_or(SelectError::UnknownDisplay(requested))?;
        Ok(self.rebuild(wire_id, platform_id, display, RebuildReason::Selected))
    }

    pub fn on_registry_change(
        &mut self,
        registry: &DisplayRegistry,
        diff: &DisplayDiff,
    ) -> DisplayAction {
        let Some(selected) = self.selected else {
            return DisplayAction::None;
        };

        if self.suspended {
            if registry.is_empty() || diff.is_empty() {
                return DisplayAction::None;
            }
            return match registry.primary() {
                Some(wire_id) => {
                    self.rebuild_from_registry(registry, wire_id, RebuildReason::Reappeared)
                }
                None => DisplayAction::None,
            };
        }

        if diff.removed.contains(&selected) || registry.get(selected).is_none() {
            return match registry.primary() {
                Some(wire_id) => {
                    self.rebuild_from_registry(registry, wire_id, RebuildReason::Removed)
                }
                None => {
                    self.suspended = true;
                    DisplayAction::Suspend { epoch: self.epoch }
                }
            };
        }

        let flags = diff
            .changed
            .iter()
            .find_map(|(wire_id, flags)| (*wire_id == selected).then_some(*flags))
            .unwrap_or_default();
        let geometry = registry.get(selected).map(DisplayGeometry::new);
        let geometry_changed = match (self.last_geometry, geometry) {
            (Some(previous), Some(current)) => (
                previous.width_px != current.width_px || previous.height_px != current.height_px,
                previous.rotation != current.rotation,
            ),
            _ => (false, false),
        };
        if flags.rotation || geometry_changed.1 {
            self.rebuild_from_registry(registry, selected, RebuildReason::Rotated)
        } else if flags.size || geometry_changed.0 {
            self.rebuild_from_registry(registry, selected, RebuildReason::Resized)
        } else {
            DisplayAction::None
        }
    }

    pub fn selected(&self) -> Option<u32> {
        self.selected
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    fn rebuild_from_registry(
        &mut self,
        registry: &DisplayRegistry,
        wire_id: u32,
        reason: RebuildReason,
    ) -> DisplayAction {
        let Some(platform_id) = registry.platform_id(wire_id) else {
            return DisplayAction::None;
        };
        let Some(display) = registry.get(wire_id).cloned() else {
            return DisplayAction::None;
        };
        self.rebuild(wire_id, platform_id, display, reason)
    }

    fn rebuild(
        &mut self,
        wire_id: u32,
        platform_id: u32,
        display: DisplayInfo,
        reason: RebuildReason,
    ) -> DisplayAction {
        self.epoch = self.epoch.wrapping_add(1);
        self.selected = Some(wire_id);
        self.suspended = false;
        self.last_geometry = Some(DisplayGeometry::new(&display));
        DisplayAction::Rebuild {
            wire_id,
            platform_id,
            epoch: self.epoch,
            display,
            reason,
        }
    }
}

fn resolve_selection(
    registry: &DisplayRegistry,
    requested: u32,
) -> Result<(u32, u32, DisplayInfo), SelectError> {
    if registry.is_empty() {
        return Err(SelectError::NoDisplays);
    }
    let wire_id = registry
        .resolve(requested)
        .ok_or(SelectError::UnknownDisplay(requested))?;
    let platform_id = registry
        .platform_id(wire_id)
        .ok_or(SelectError::UnknownDisplay(requested))?;
    let display = registry
        .get(wire_id)
        .cloned()
        .ok_or(SelectError::UnknownDisplay(requested))?;
    Ok((wire_id, platform_id, display))
}

/// Fit a display within client limits while treating the longer limit as the long edge.
pub fn fit_stream_size(display: &DisplayInfo, max_width: u32, max_height: u32) -> (u32, u32) {
    let long_edge = display.width_px.max(display.height_px);
    let short_edge = display.width_px.min(display.height_px);
    let long_limit = max_width.max(max_height);
    let short_limit = max_width.min(max_height);
    let ratio = limit_ratio(long_limit, long_edge)
        .min(limit_ratio(short_limit, short_edge))
        .min(1.0);
    (
        scaled_even(display.width_px, ratio),
        scaled_even(display.height_px, ratio),
    )
}

fn limit_ratio(limit: u32, edge: u32) -> f64 {
    if limit == 0 || edge == 0 {
        f64::INFINITY
    } else {
        f64::from(limit) / f64::from(edge)
    }
}

fn scaled_even(edge: u32, ratio: f64) -> u32 {
    let scaled = (f64::from(edge) * ratio).round() as u32;
    (scaled & !1).max(2)
}

pub fn epoch_byte(epoch: u32) -> u8 {
    epoch as u8
}
