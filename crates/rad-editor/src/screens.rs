//! Which screen fills the editor's window, and how Back leaves one (E4). Leaving never loses
//! unapplied changes silently: the screen stays and asks once, until they are applied or the
//! user discards them.

/// The parts of the settings screen, one at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    General,
    Backup,
    Plugins,
    Automation,
    Updates,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Keys,
    Settings(Section),
}

/// Back (the button or Esc) from a view that may hold unapplied changes.
#[derive(Debug, Default)]
pub struct Leave {
    asking: bool,
}
impl Leave {
    /// Whether the view may close now; with unapplied changes it asks instead.
    pub fn request(&mut self, unapplied: bool) -> bool {
        self.asking = unapplied;
        !unapplied
    }
    /// The question is on display while there is still something unapplied.
    pub fn asking(&self, unapplied: bool) -> bool {
        self.asking && unapplied
    }
    pub fn reset(&mut self) {
        self.asking = false;
    }
}

#[derive(Debug, Default)]
pub struct Navigation {
    screen: Screen,
    leave: Leave,
}
impl Navigation {
    pub fn screen(&self) -> Screen {
        self.screen
    }
    /// Shows a section of the settings, from the keys or from another section.
    pub fn open(&mut self, section: Section) {
        self.screen = Screen::Settings(section);
        self.leave.reset();
    }
    /// Back to the keys; says whether it left.
    pub fn back(&mut self, unapplied: bool) -> bool {
        if self.screen == Screen::Keys || !self.leave.request(unapplied) {
            return false;
        }
        self.screen = Screen::Keys;
        true
    }
    pub fn asking(&self, unapplied: bool) -> bool {
        self.leave.asking(unapplied)
    }
    /// The user chose to drop what was not applied: back to the keys.
    pub fn discard(&mut self) {
        self.screen = Screen::Keys;
        self.leave.reset();
    }
}
