//! What each engine method takes. Engine-owned types over `mklm_core` data: the engine does not
//! depend on the pipe protocol, and the helper maps each wire request onto one of these explicitly
//! (design review S5). Only the helper's fixed `--uninstall-restore` command line (M5) builds a
//! [`RestoreMode::Silent`] restore.

use mklm_core::{
    ApplyOptions, ConflictPolicy, ExpectedPlan, Layout, LayoutChoice, OpId, RestoreScope,
    ValueChoice,
};

/// Section D.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetLayoutParams {
    pub instance_id: String,
    pub layout: LayoutChoice,
    pub apply: ApplyOptions,
    pub expected: Option<ExpectedPlan>,
}

/// Section D.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateParams {
    pub standard: Layout,
    pub assignments: Vec<(String, LayoutChoice)>,
    pub expected: Option<ExpectedPlan>,
}

/// How a restore-to-baseline runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestoreMode {
    /// The user asked for it: same apply paths as `set` (countdown, reconnect or restart).
    Interactive,
    /// Uninstall custom action (M5): no confirmation, conflicts skipped and logged, ends
    /// `Confirmed` right after its writes, never rolled back.
    Silent,
}

/// Section D.5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreBaselineParams {
    pub scope: RestoreScope,
    pub on_conflict: ConflictPolicy,
    pub mode: RestoreMode,
    pub apply: ApplyOptions,
}

/// Section D.8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveParams {
    pub op_id: OpId,
    pub choices: Vec<ValueChoice>,
    pub apply: ApplyOptions,
}
