//! Pure automation permissions, not an OS security boundary.
//! Permission administration and invocation admission belong to future adapters.

/// A complete, valid Consulta/Modificar/Ejecutar set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    consult: bool,
    modify: bool,
    execute: bool,
}

/// Modificar or Ejecutar was requested without Consulta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPermissions;

impl Permissions {
    /// Rejects contradictory input rather than silently granting Consulta.
    pub fn new(consult: bool, modify: bool, execute: bool) -> Result<Self, InvalidPermissions> {
        if !consult && (modify || execute) {
            return Err(InvalidPermissions);
        }
        Ok(Self {
            consult,
            modify,
            execute,
        })
    }

    pub fn consult(self) -> bool {
        self.consult
    }
    pub fn modify(self) -> bool {
        self.modify
    }
    pub fn execute(self) -> bool {
        self.execute
    }

    /// Disabling Consulta clears both dependent permissions; enabling restores neither.
    pub fn set_consult(&mut self, enabled: bool) {
        self.consult = enabled;
        if !enabled {
            self.modify = false;
            self.execute = false;
        }
    }

    /// Enabling Modificar also enables Consulta; Ejecutar is unchanged.
    pub fn set_modify(&mut self, enabled: bool) {
        self.modify = enabled;
        if enabled {
            self.consult = true;
        }
    }

    /// Enabling Ejecutar also enables Consulta; Modificar is unchanged.
    pub fn set_execute(&mut self, enabled: bool) {
        self.execute = enabled;
        if enabled {
            self.consult = true;
        }
    }
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            consult: true,
            modify: true,
            execute: false,
        }
    }
}

/// Button/action configuration: inherits by default, or replaces the entire set.
/// Resolve a button against Global, then an action against that resolved button.
/// Replace with `Self::default()` to resume inheritance (no captured parent value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalPermissions(Option<Permissions>);

impl LocalPermissions {
    /// Explicit override, independent of the current parent permission value.
    pub fn own_value(self) -> Option<Permissions> {
        self.0
    }
    pub fn own(permissions: Permissions) -> Self {
        Self(Some(permissions))
    }

    /// Own permissions may expand or restrict the parent's complete set.
    pub fn resolve(self, parent: Permissions) -> Permissions {
        self.0.unwrap_or(parent)
    }
}
