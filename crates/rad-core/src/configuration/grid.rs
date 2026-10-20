use super::ConfigurationError;
/// Supported deck geometry. Additional device models require an explicit implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid;
impl Grid {
    pub const XL: Self = Self;
    pub fn new(columns: u8, rows: u8) -> Result<Self, ConfigurationError> {
        if (columns, rows) != (8, 4) {
            return Err(ConfigurationError::InvalidValue);
        }
        Ok(Self::XL)
    }
    pub const fn columns(self) -> u8 {
        8
    }
    pub const fn rows(self) -> u8 {
        4
    }
    pub const fn key_count(self) -> usize {
        self.columns() as usize * self.rows() as usize
    }
}
