use std::env;

pub struct Config {
    pub port: u16,
    pub password: String,
    pub shell: String,
}

impl Config {
    pub fn from_env() -> Self {
        let port = env::var("DEN_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);

        let password = env::var("DEN_PASSWORD").unwrap_or_else(|_| "den".to_string());

        let shell = env::var("DEN_SHELL").unwrap_or_else(|_| {
            if cfg!(windows) {
                env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
            } else {
                env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            }
        });

        Self {
            port,
            password,
            shell,
        }
    }
}
