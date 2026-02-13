use std::env;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum Environment {
    Development,
    Production,
}

impl FromStr for Environment {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "development" | "dev" => Ok(Environment::Development),
            "production" | "prod" => Ok(Environment::Production),
            _ => Err(format!("Unknown environment: {}", s)),
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Environment::Development => write!(f, "development"),
            Environment::Production => write!(f, "production"),
        }
    }
}

pub struct Config {
    pub port: u16,
    pub password: String,
    pub shell: String,
    pub env: Environment,
    pub log_level: String,
}

impl Config {
    pub fn from_env() -> Self {
        let env = env::var("DEN_ENV")
            .ok()
            .and_then(|v| Environment::from_str(&v).ok())
            .unwrap_or(Environment::Development);

        let default_port = match env {
            Environment::Development => 8080,
            Environment::Production => 3000,
        };

        let port = env::var("DEN_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_port);

        let password = env::var("DEN_PASSWORD").unwrap_or_else(|_| "den".to_string());

        let shell = env::var("DEN_SHELL").unwrap_or_else(|_| {
            if cfg!(windows) {
                env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
            } else {
                env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            }
        });

        let default_log_level = match env {
            Environment::Development => "debug",
            Environment::Production => "info",
        };
        let log_level = env::var("DEN_LOG_LEVEL").unwrap_or_else(|_| default_log_level.to_string());

        Self {
            port,
            password,
            shell,
            env,
            log_level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // SAFETY: serial_test ensures these tests run sequentially,
    // so concurrent env mutation is not a concern.
    fn clear_env() {
        unsafe {
            env::remove_var("DEN_ENV");
            env::remove_var("DEN_PORT");
            env::remove_var("DEN_PASSWORD");
            env::remove_var("DEN_SHELL");
            env::remove_var("DEN_LOG_LEVEL");
        }
    }

    #[test]
    #[serial]
    fn defaults_dev() {
        clear_env();
        let config = Config::from_env();
        assert_eq!(config.env, Environment::Development);
        assert_eq!(config.port, 8080);
        assert_eq!(config.password, "den");
        assert_eq!(config.log_level, "debug");
    }

    #[test]
    #[serial]
    fn defaults_prod() {
        clear_env();
        unsafe { env::set_var("DEN_ENV", "production") };
        let config = Config::from_env();
        assert_eq!(config.env, Environment::Production);
        assert_eq!(config.port, 3000);
        assert_eq!(config.log_level, "info");
        clear_env();
    }

    #[test]
    #[serial]
    fn custom_port() {
        clear_env();
        unsafe { env::set_var("DEN_PORT", "9090") };
        let config = Config::from_env();
        assert_eq!(config.port, 9090);
        clear_env();
    }

    #[test]
    #[serial]
    fn invalid_port_uses_default() {
        clear_env();
        unsafe { env::set_var("DEN_PORT", "not_a_number") };
        let config = Config::from_env();
        assert_eq!(config.port, 8080);
        clear_env();
    }

    #[test]
    #[serial]
    fn custom_password() {
        clear_env();
        unsafe { env::set_var("DEN_PASSWORD", "secret123") };
        let config = Config::from_env();
        assert_eq!(config.password, "secret123");
        clear_env();
    }

    #[test]
    fn environment_from_str() {
        assert_eq!(
            Environment::from_str("development").unwrap(),
            Environment::Development
        );
        assert_eq!(
            Environment::from_str("dev").unwrap(),
            Environment::Development
        );
        assert_eq!(
            Environment::from_str("production").unwrap(),
            Environment::Production
        );
        assert_eq!(
            Environment::from_str("prod").unwrap(),
            Environment::Production
        );
        assert_eq!(
            Environment::from_str("PRODUCTION").unwrap(),
            Environment::Production
        );
        assert!(Environment::from_str("staging").is_err());
    }
}
