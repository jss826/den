use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct SshHost {
    pub name: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// ~/.ssh/config からホスト一覧を取得
pub fn list_ssh_hosts() -> Vec<SshHost> {
    let path = ssh_config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    parse_ssh_config(&content)
}

fn ssh_config_path() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    home.map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config")
}

fn parse_ssh_config(content: &str) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    let mut current: Option<SshHost> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.trim(), v.trim().trim_matches('"')),
            None => continue,
        };

        if key.eq_ignore_ascii_case("host") {
            if let Some(h) = current.take()
                && h.name != "*"
            {
                hosts.push(h);
            }
            current = Some(SshHost {
                name: value.to_string(),
                hostname: None,
                user: None,
                port: None,
            });
        } else if key.eq_ignore_ascii_case("hostname") {
            if let Some(ref mut h) = current {
                h.hostname = Some(value.to_string());
            }
        } else if key.eq_ignore_ascii_case("user") {
            if let Some(ref mut h) = current {
                h.user = Some(value.to_string());
            }
        } else if key.eq_ignore_ascii_case("port")
            && let Some(ref mut h) = current
        {
            h.port = value.parse().ok();
        }
    }

    if let Some(h) = current
        && h.name != "*"
    {
        hosts.push(h);
    }

    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_config() {
        let config = r#"
Host dev-server
    HostName 192.168.1.100
    User admin
    Port 2222

Host prod
    HostName example.com
    User deploy

Host *
    ServerAliveInterval 60
"#;
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "dev-server");
        assert_eq!(hosts[0].hostname.as_deref(), Some("192.168.1.100"));
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].name, "prod");
        assert_eq!(hosts[1].hostname.as_deref(), Some("example.com"));
        assert_eq!(hosts[1].port, None);
    }

    #[test]
    fn empty_config() {
        let hosts = parse_ssh_config("");
        assert!(hosts.is_empty());
    }

    #[test]
    fn comments_only() {
        let config = "# This is a comment\n# Another comment\n";
        let hosts = parse_ssh_config(config);
        assert!(hosts.is_empty());
    }

    #[test]
    fn wildcard_excluded() {
        let config = "Host *\n    ServerAliveInterval 60\n";
        let hosts = parse_ssh_config(config);
        assert!(hosts.is_empty());
    }

    #[test]
    fn equals_syntax() {
        let config = "Host=myhost\n    HostName=10.0.0.1\n    User=root\n    Port=22\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "myhost");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.1"));
        assert_eq!(hosts[0].user.as_deref(), Some("root"));
        assert_eq!(hosts[0].port, Some(22));
    }

    #[test]
    fn minimal_host() {
        let config = "Host jump\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "jump");
        assert!(hosts[0].hostname.is_none());
        assert!(hosts[0].user.is_none());
        assert!(hosts[0].port.is_none());
    }

    #[test]
    fn duplicate_host_names() {
        let config = "Host a\n    User u1\nHost a\n    User u2\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].user.as_deref(), Some("u1"));
        assert_eq!(hosts[1].user.as_deref(), Some("u2"));
    }

    #[test]
    fn invalid_port_ignored() {
        let config = "Host bad\n    Port abc\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 1);
        assert!(hosts[0].port.is_none());
    }

    #[test]
    fn case_insensitive_keys() {
        let config = "HOST myhost\n    HOSTNAME 1.2.3.4\n    USER admin\n    PORT 443\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname.as_deref(), Some("1.2.3.4"));
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
        assert_eq!(hosts[0].port, Some(443));
    }

    #[test]
    fn quoted_values() {
        let config = "Host quoted\n    HostName \"my.server.com\"\n    User \"admin\"\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname.as_deref(), Some("my.server.com"));
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
    }

    #[test]
    fn unknown_directives_ignored() {
        let config = "Host myhost\n    IdentityFile ~/.ssh/id_rsa\n    ForwardAgent yes\n    HostName 10.0.0.1\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.1"));
    }
}
