#![cfg(windows)]

//! Synchronous INetFwPolicy2 adapter for [`crate::firewall_model`].
//!
//! The service must run as SYSTEM to have permission to change firewall rules.
//! All calls block; enumerating thousands of rules can take hundreds of
//! milliseconds, so the service should call this module from `spawn_blocking`.
//! COM apartments and interfaces are created and released on the calling
//! thread; `WinFirewall` retains only ordinary Rust data.
//!
//! Call `sync_intranet` once at service startup, whenever intranet settings
//! change, and on NIC address changes from the address watcher. Service
//! wiring belongs to the assembly layer.

use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use host_core::ipc::NetworkFirewallStatusResult;
use host_core::listen::ListenPolicy;
use host_core::platform::Nic;
use tracing::{debug, info, warn};
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, E_ACCESSDENIED, RPC_E_CHANGED_MODE, S_FALSE, S_OK,
};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NetFwPolicy2, NetFwRule, NET_FW_ACTION_ALLOW,
    NET_FW_MODIFY_STATE_OK, NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE,
    NET_FW_PROFILE2_PUBLIC, NET_FW_RULE_DIR_IN,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH};

use crate::firewall_model::{
    self, FwFacts, FwPlan, FwRuleSpec, ProfileState, INTRANET_PREFIX, RULE_GROUP, TAILNET_PREFIX,
};

#[derive(Debug, thiserror::Error)]
pub enum FwError {
    #[error("firewall access denied: {0}")]
    AccessDenied(String),
    #[error("firewall COM error: {0}")]
    Com(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub error: Option<String>,
    pub policy_locked: bool,
}

impl SyncReport {
    fn record_error(&mut self, error: FwError) {
        self.policy_locked |= matches!(error, FwError::AccessDenied(_));
        self.error.get_or_insert_with(|| error.to_string());
    }
}

fn com<T>(context: &str, result: windows::core::Result<T>) -> Result<T, FwError> {
    result.map_err(|error| {
        let hr = error.code().0 as u32;
        let message = format!("{context}: 0x{hr:08x}");
        if error.code() == E_ACCESSDENIED {
            FwError::AccessDenied(message)
        } else {
            FwError::Com(message)
        }
    })
}

struct ComGuard {
    initialized: bool,
    // An apartment must be uninitialized on the thread that initialized it.
    _thread_bound: PhantomData<Rc<()>>,
}

impl ComGuard {
    fn new() -> Result<Self, FwError> {
        // SAFETY: the reserved pointer is null. Successful initialization is
        // balanced by this thread-bound guard after its interfaces are dropped.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr != RPC_E_CHANGED_MODE {
            com("CoInitializeEx", hr.ok())?;
        }
        Ok(Self {
            initialized: hr == S_OK || hr == S_FALSE,
            _thread_bound: PhantomData,
        })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: only S_OK/S_FALSE own an initialization to balance. This
            // guard cannot leave its thread; all local interfaces are gone.
            unsafe { CoUninitialize() };
        }
    }
}

fn with_policy<T>(
    operation: impl FnOnce(&INetFwPolicy2) -> Result<T, FwError>,
) -> Result<T, FwError> {
    let _com = ComGuard::new()?;
    // SAFETY: COM is initialized on this thread (including an existing STA).
    // The local interface is released before the apartment guard.
    let policy = com("CoCreateInstance(NetFwPolicy2)", unsafe {
        CoCreateInstance::<_, INetFwPolicy2>(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
    })?;
    operation(&policy)
}

/// Read managed inbound allow rules, skipping individual unreadable entries.
pub fn read_rules() -> Result<Vec<FwRuleSpec>, FwError> {
    with_policy(|policy| {
        // SAFETY: the policy and returned interfaces remain on their initialized
        // COM thread and are dropped before with_policy's apartment guard.
        let enumerator = unsafe {
            let rules = com("INetFwPolicy2::Rules", policy.Rules())?;
            let unknown = com("INetFwRules::_NewEnum", rules._NewEnum())?;
            com(
                "IEnumVARIANT::QueryInterface",
                unknown.cast::<IEnumVARIANT>(),
            )?
        };
        let mut result = Vec::new();
        loop {
            let mut batch: [VARIANT; 32] = std::array::from_fn(|_| VARIANT::default());
            let mut fetched = 0;
            // SAFETY: Next receives initialized writable variants and a valid
            // fetched-count pointer. Each variant's Drop calls VariantClear.
            let hr = unsafe { enumerator.Next(&mut batch, &mut fetched) };
            com("IEnumVARIANT::Next", hr.ok())?;
            if hr == S_FALSE && fetched == 0 {
                break;
            }
            if fetched == 0 || fetched as usize > batch.len() {
                return Err(FwError::Com(format!(
                    "IEnumVARIANT::Next: invalid fetched count {fetched}"
                )));
            }
            for variant in batch.iter().take(fetched as usize) {
                match read_variant(variant) {
                    Ok(Some(rule)) => result.push(rule),
                    Ok(None) => {}
                    Err(error) => debug!(%error, "skipping unreadable firewall rule"),
                }
            }
        }
        Ok(result)
    })
}

fn read_variant(variant: &VARIANT) -> Result<Option<FwRuleSpec>, FwError> {
    // SAFETY: the type tag is always present in an initialized VARIANT. The
    // dispatch union member is accessed only after checking its exact tag.
    let dispatch = unsafe {
        let value = &variant.Anonymous.Anonymous;
        if value.vt != VT_DISPATCH {
            debug!(
                variant_type = value.vt.0,
                "skipping non-dispatch firewall entry"
            );
            return Ok(None);
        }
        // Clone the Option<IDispatch>, not its ManuallyDrop wrapper: the clone
        // owns one AddRef and releases it normally, independently of VARIANT.
        (*value.Anonymous.pdispVal).clone()
    };
    let Some(dispatch) = dispatch else {
        debug!("skipping null firewall dispatch entry");
        return Ok(None);
    };
    let rule = com("INetFwRule::QueryInterface", dispatch.cast::<INetFwRule>())?;
    // SAFETY: rule is a live interface on this initialized apartment. Getters
    // return owned strings and scalar values; no borrowed COM pointers escape.
    unsafe {
        let name = com("INetFwRule::Name", rule.Name())?.to_string();
        let group = com("INetFwRule::Grouping", rule.Grouping())?.to_string();
        if !name.starts_with("Remote Host") && group != RULE_GROUP {
            return Ok(None);
        }
        if com("INetFwRule::Direction", rule.Direction())? != NET_FW_RULE_DIR_IN
            || com("INetFwRule::Action", rule.Action())? != NET_FW_ACTION_ALLOW
        {
            return Ok(None);
        }
        let application = com("INetFwRule::ApplicationName", rule.ApplicationName())?.to_string();
        Ok(Some(FwRuleSpec {
            name,
            description: com("INetFwRule::Description", rule.Description())?.to_string(),
            group,
            protocol: com("INetFwRule::Protocol", rule.Protocol())?,
            local_ports: com("INetFwRule::LocalPorts", rule.LocalPorts())?.to_string(),
            local_addresses: com("INetFwRule::LocalAddresses", rule.LocalAddresses())?.to_string(),
            remote_addresses: com("INetFwRule::RemoteAddresses", rule.RemoteAddresses())?
                .to_string(),
            profiles: com("INetFwRule::Profiles", rule.Profiles())?,
            application: (!application.is_empty()).then_some(application),
            enabled: com("INetFwRule::Enabled", rule.Enabled())?.as_bool(),
        }))
    }
}

/// Fully configure a detached rule before adding it to the system collection.
pub fn add_rule(spec: &FwRuleSpec) -> Result<(), FwError> {
    with_policy(|policy| {
        // SAFETY: all interfaces stay on this COM thread; every BSTR argument
        // lives through its call. The new rule is published only after all
        // setters succeed, so setter failures leave no partial system rule.
        unsafe {
            let rules = com("INetFwPolicy2::Rules", policy.Rules())?;
            let rule = com(
                "CoCreateInstance(NetFwRule)",
                CoCreateInstance::<_, INetFwRule>(&NetFwRule, None, CLSCTX_INPROC_SERVER),
            )?;
            com("INetFwRule::SetName", rule.SetName(&BSTR::from(&spec.name)))?;
            com(
                "INetFwRule::SetDescription",
                rule.SetDescription(&BSTR::from(&spec.description)),
            )?;
            com("INetFwRule::SetProtocol", rule.SetProtocol(spec.protocol))?;
            com(
                "INetFwRule::SetLocalPorts",
                rule.SetLocalPorts(&BSTR::from(&spec.local_ports)),
            )?;
            if spec.local_addresses != "*" {
                com(
                    "INetFwRule::SetLocalAddresses",
                    rule.SetLocalAddresses(&BSTR::from(&spec.local_addresses)),
                )?;
            }
            if spec.remote_addresses != "*" {
                com(
                    "INetFwRule::SetRemoteAddresses",
                    rule.SetRemoteAddresses(&BSTR::from(&spec.remote_addresses)),
                )?;
            }
            com(
                "INetFwRule::SetDirection",
                rule.SetDirection(NET_FW_RULE_DIR_IN),
            )?;
            com("INetFwRule::SetAction", rule.SetAction(NET_FW_ACTION_ALLOW))?;
            com("INetFwRule::SetProfiles", rule.SetProfiles(spec.profiles))?;
            com(
                "INetFwRule::SetGrouping",
                rule.SetGrouping(&BSTR::from(&spec.group)),
            )?;
            if let Some(application) = &spec.application {
                com(
                    "INetFwRule::SetApplicationName",
                    rule.SetApplicationName(&BSTR::from(application)),
                )?;
            }
            com(
                "INetFwRule::SetEnabled",
                rule.SetEnabled(spec.enabled.into()),
            )?;
            com("INetFwRules::Add", rules.Add(&rule))
        }
    })
}

/// Delete all rules with this name, with a maximum of sixteen removals.
pub fn remove_rule(name: &str) -> Result<u32, FwError> {
    with_policy(|policy| {
        let name = BSTR::from(name);
        let mut removed = 0;
        // SAFETY: the collection remains on the initialized COM thread, and
        // the owned name BSTR outlives all Item/Remove calls.
        unsafe {
            let rules = com("INetFwPolicy2::Rules", policy.Rules())?;
            loop {
                match rules.Item(&name) {
                    Ok(_) => {}
                    Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                        return Ok(removed);
                    }
                    Err(error) => return com("INetFwRules::Item", Err(error)),
                }
                if removed == 16 {
                    return Err(FwError::Com("rule keeps reappearing".to_owned()));
                }
                com("INetFwRules::Remove", rules.Remove(&name))?;
                removed += 1;
            }
        }
    })
}

pub fn profile_state() -> Result<ProfileState, FwError> {
    with_policy(|policy| {
        // SAFETY: the live policy stays on its initialized COM thread, and all
        // profile constants are valid individual NET_FW_PROFILE_TYPE2 values.
        unsafe {
            Ok(ProfileState {
                domain: com(
                    "INetFwPolicy2::get_FirewallEnabled(domain)",
                    policy.get_FirewallEnabled(NET_FW_PROFILE2_DOMAIN),
                )?
                .as_bool(),
                private: com(
                    "INetFwPolicy2::get_FirewallEnabled(private)",
                    policy.get_FirewallEnabled(NET_FW_PROFILE2_PRIVATE),
                )?
                .as_bool(),
                public: com(
                    "INetFwPolicy2::get_FirewallEnabled(public)",
                    policy.get_FirewallEnabled(NET_FW_PROFILE2_PUBLIC),
                )?
                .as_bool(),
                current: com(
                    "INetFwPolicy2::CurrentProfileTypes",
                    policy.CurrentProfileTypes(),
                )?,
            })
        }
    })
}

pub fn local_policy_locked() -> Result<bool, FwError> {
    with_policy(|policy| {
        // SAFETY: the policy is live on this initialized COM thread.
        let state = com("INetFwPolicy2::LocalPolicyModifyState", unsafe {
            policy.LocalPolicyModifyState()
        })?;
        Ok(state != NET_FW_MODIFY_STATE_OK)
    })
}

/// Apply every removal before any addition; retain the first error and continue.
pub fn apply(plan: &FwPlan) -> SyncReport {
    let mut report = SyncReport::default();
    for name in &plan.remove_names {
        match remove_rule(name) {
            Ok(count) if count > 0 => report.removed.push(name.clone()),
            Ok(_) => {}
            Err(error) => {
                warn!(%name, %error, "firewall rule removal failed");
                report.record_error(error);
            }
        }
    }
    for spec in &plan.add {
        match add_rule(spec) {
            Ok(()) => report.added.push(spec.name.clone()),
            Err(error) => {
                warn!(name = %spec.name, %error, "firewall rule addition failed");
                report.record_error(error);
            }
        }
    }
    report
}

/// Reusable service adapter; no COM interfaces or apartment state are retained.
pub struct WinFirewall {
    exe: Option<String>,
    port: u16,
    last_error: Mutex<Option<String>>,
    last_denied: AtomicBool,
}

impl WinFirewall {
    pub fn new(exe: Option<String>, port: u16) -> Self {
        Self {
            exe,
            port,
            last_error: Mutex::new(None),
            last_denied: AtomicBool::new(false),
        }
    }

    pub fn for_current_exe(port: u16) -> Self {
        let exe = std::env::current_exe()
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        Self::new(exe, port)
    }

    pub fn facts(&self) -> FwFacts {
        self.read_facts().0
    }

    // Keep a fresh enumeration failure separate from cached synchronization
    // errors. A failed read must not look like an empty collection to a planner,
    // while a previous write failure must not prevent a later retry.
    fn read_facts(&self) -> (FwFacts, Option<FwError>) {
        let mut facts = FwFacts::default();
        match profile_state() {
            Ok(profiles) => facts.profiles = profiles,
            Err(error) => {
                facts.error.get_or_insert_with(|| error.to_string());
            }
        }
        match local_policy_locked() {
            Ok(locked) => facts.policy_locked = locked,
            Err(error) => {
                facts.error.get_or_insert_with(|| error.to_string());
            }
        }
        let rules_error = match read_rules() {
            Ok(existing) => {
                facts.existing = existing;
                None
            }
            Err(error) => {
                facts.error.get_or_insert_with(|| error.to_string());
                Some(error)
            }
        };
        let last_error = self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if facts.error.is_none() {
            facts.error.clone_from(&last_error);
        }
        facts.policy_locked |= self.last_denied.load(Ordering::Acquire);
        (facts, rules_error)
    }

    fn reconcile(&self, make_plan: impl FnOnce(&[FwRuleSpec]) -> FwPlan) -> SyncReport {
        let (facts, rules_error) = self.read_facts();
        let report = if let Some(error) = rules_error {
            warn!(%error, "firewall synchronization could not read rules");
            let mut report = SyncReport::default();
            report.record_error(error);
            report
        } else {
            apply(&make_plan(&facts.existing))
        };
        let mut last_error = self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last_error.clone_from(&report.error);
        self.last_denied
            .store(report.policy_locked, Ordering::Release);
        report
    }

    pub fn sync_intranet(&self, policy: &ListenPolicy, nics: &[Nic]) -> SyncReport {
        let report = self.reconcile(|existing| {
            let desired = firewall_model::intranet_rules(policy, nics, self.exe.as_deref());
            firewall_model::plan_intranet(&desired, existing)
        });
        info!(removed = ?report.removed, added = ?report.added, error = ?report.error,
            "intranet firewall synchronization completed");
        report
    }

    pub fn ensure_tailnet(&self) -> SyncReport {
        self.reconcile(|existing| {
            if firewall_model::tailnet_ok(existing, self.port) {
                FwPlan::default()
            } else {
                firewall_model::plan(
                    TAILNET_PREFIX,
                    &firewall_model::tailnet_rules(self.port, self.exe.as_deref()),
                    existing,
                )
            }
        })
    }

    pub fn remove_intranet(&self) -> SyncReport {
        self.reconcile(|existing| firewall_model::plan(INTRANET_PREFIX, &[], existing))
    }

    pub fn status(&self, policy: &ListenPolicy, nics: &[Nic]) -> NetworkFirewallStatusResult {
        firewall_model::status(
            &self.facts(),
            &firewall_model::intranet_rules(policy, nics, self.exe.as_deref()),
            self.port,
        )
    }

    pub fn it_note(&self, policy: &ListenPolicy, nics: &[Nic]) -> String {
        firewall_model::it_note(
            policy,
            nics,
            self.exe.as_deref(),
            self.facts().policy_locked,
        )
    }
}
