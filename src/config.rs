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
    pub data_dir: String,
    pub bind_address: String,
    /// SSH ポート（None = SSH 無効、DEN_SSH_PORT で指定）
    pub ssh_port: Option<u16>,
}

impl Config {
    pub fn from_env() -> Self {
        let env = env::var("DEN_ENV")
            .ok()
            .and_then(|v| Environment::from_str(&v).ok())
            .unwrap_or(Environment::Development);

        let default_port = match env {
            Environment::Development => 3939,
            Environment::Production => 8080,
        };

        let port = env::var("DEN_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_port);

        let password = match env::var("DEN_PASSWORD") {
            Ok(p) if !p.is_empty() => p,
            _ => {
                eprintln!("ERROR: DEN_PASSWORD environment variable is required.");
                eprintln!("  Set it before starting Den: DEN_PASSWORD=your_password cargo run");
                std::process::exit(1);
            }
        };

        let shell = env::var("DEN_SHELL").unwrap_or_else(|_| {
            if cfg!(windows) {
                "powershell.exe".to_string()
            } else {
                env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            }
        });

        let default_log_level = match env {
            Environment::Development => "debug",
            Environment::Production => "info",
        };
        let log_level = env::var("DEN_LOG_LEVEL").unwrap_or_else(|_| default_log_level.to_string());

        let data_dir = env::var("DEN_DATA_DIR").unwrap_or_else(|_| "./data".to_string());

        let ssh_port = env::var("DEN_SSH_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|&p| p > 0);

        let default_bind = match env {
            Environment::Development => "127.0.0.1",
            Environment::Production => "0.0.0.0",
        };
        let bind_address =
            env::var("DEN_BIND_ADDRESS").unwrap_or_else(|_| default_bind.to_string());

        Self {
            port,
            password,
            shell,
            env,
            log_level,
            data_dir,
            bind_address,
            ssh_port,
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
            env::set_var("DEN_PASSWORD", "test_password");
            env::remove_var("DEN_SHELL");
            env::remove_var("DEN_LOG_LEVEL");
            env::remove_var("DEN_DATA_DIR");
            env::remove_var("DEN_BIND_ADDRESS");
            env::remove_var("DEN_SSH_PORT");
        }
    }

    #[test]
    #[serial]
    fn defaults_dev() {
        clear_env();
        let config = Config::from_env();
        assert_eq!(config.env, Environment::Development);
        assert_eq!(config.port, 3939);
        assert_eq!(config.password, "test_password");
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.bind_address, "127.0.0.1");
    }

    #[test]
    #[serial]
    fn defaults_prod() {
        clear_env();
        unsafe { env::set_var("DEN_ENV", "production") };
        let config = Config::from_env();
        assert_eq!(config.env, Environment::Production);
        assert_eq!(config.port, 8080);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.bind_address, "0.0.0.0");
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
        assert_eq!(config.port, 3939);
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
    #[serial]
    fn custom_bind_address() {
        clear_env();
        unsafe { env::set_var("DEN_BIND_ADDRESS", "192.168.1.100") };
        let config = Config::from_env();
        assert_eq!(config.bind_address, "192.168.1.100");
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
