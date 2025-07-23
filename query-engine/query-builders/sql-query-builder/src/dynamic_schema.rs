/**
 * Changed by @vetching-corporation
 * Author: nfl1ryxditimo12@gmail.com
 * Date: 2025-06-16
 * Note: Add `DynamicSchema` struct to support dynamic schema
 */
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct DynamicSchema(HashMap<String, String>);

impl DynamicSchema {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn from_str(s: Option<String>) -> Self {
        if s.is_none() {
            return Self::new();
        }
        Self(serde_json::from_str(&s.unwrap()).unwrap_or_default())
    }
}

// HashMap의 모든 메서드를 사용할 수 있게 함
impl std::ops::Deref for DynamicSchema {
    type Target = HashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for DynamicSchema {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
