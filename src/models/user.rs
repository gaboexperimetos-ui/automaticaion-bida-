use serde::{Deserialize, Serialize};

/// A user in the system (currently only admin).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Password hash is NEVER serialized to responses
    #[serde(skip)]
    pub password_hash: String,
    pub created_at: String,
}

/// Request to register a new user.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub confirm: String,
}

/// Request to set a password (for admin setup or password change).
#[derive(Debug, Deserialize)]
pub struct SetPasswordRequest {
    pub password: String,
    pub confirm: String,
}

impl SetPasswordRequest {
    /// Validate that password matches confirmation and meets minimum requirements.
    pub fn validate(&self) -> Result<(), String> {
        if self.password.is_empty() {
            return Err("La contraseña no puede estar vacía".to_string());
        }
        if self.password.len() < 8 {
            return Err("La contraseña debe tener al menos 8 caracteres".to_string());
        }
        if self.password != self.confirm {
            return Err("Las contraseñas no coinciden".to_string());
        }
        Ok(())
    }
}
