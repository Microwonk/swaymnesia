//! Rules to determine which windows to save to the session.

use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use regex::Regex;

#[derive(Debug, Default)]
pub struct Config {
    ignore: Vec<Rule>,
    allow: Vec<Rule>,
}

impl Config {
    /// Reads the configuration. If it is not found, [`Default::default()`] is called.
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let path = path();
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()).into()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display()).into()),
        }
    }

    /// Whether a window should be ignored or allowed
    pub fn skips(&self, app_id: &str, title: &str, command: &str) -> bool {
        let matches = |rule: &Rule| rule.matches(app_id, title, command);
        self.ignore.iter().any(matches) && !self.allow.iter().any(matches)
    }

    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let mut config = Config::default();
        let mut open: Option<(Table, Rule, usize)> = None;

        for (index, raw) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(name) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
                let table = match name.trim() {
                    "ignore" => Table::Ignore,
                    "allow" => Table::Allow,
                    other => {
                        return Err(ParseError::at(
                            line_number,
                            format!("unknown table [[{other}]]"),
                        ));
                    }
                };
                config.close(open.take())?;
                open = Some((table, Rule::default(), line_number));
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(ParseError::at(
                    line_number,
                    "expected a key = \"value\" pair".into(),
                ));
            };
            let Some((_, rule, _)) = open.as_mut() else {
                return Err(ParseError::at(
                    line_number,
                    "a rule has to start with [[ignore]] or [[allow]]".into(),
                ));
            };

            let value = unquote(value.trim()).ok_or_else(|| {
                ParseError::at(line_number, "value is not a quoted string".into())
            })?;
            let pattern = Regex::new(&value)
                .map_err(|e| ParseError::at(line_number, format!("bad pattern: {e}")))?;

            let key = key.trim();
            let field = match key {
                "app_id" => &mut rule.app_id,
                "title" => &mut rule.title,
                "command" => &mut rule.command,
                other => {
                    return Err(ParseError::at(
                        line_number,
                        format!("unknown field '{other}'"),
                    ));
                }
            };
            if field.is_some() {
                return Err(ParseError::at(line_number, format!("'{key}' is set twice")));
            }
            *field = Some(pattern);
        }
        config.close(open)?;
        Ok(config)
    }

    fn close(&mut self, open: Option<(Table, Rule, usize)>) -> Result<(), ParseError> {
        let Some((table, rule, line_number)) = open else {
            return Ok(());
        };
        if rule.app_id.is_none() && rule.title.is_none() && rule.command.is_none() {
            return Err(ParseError::at(
                line_number,
                "rule has no app_id, title or command to match on".into(),
            ));
        }
        match table {
            Table::Ignore => self.ignore.push(rule),
            Table::Allow => self.allow.push(rule),
        }
        Ok(())
    }
}

enum Table {
    Ignore,
    Allow,
}

#[derive(Debug, Default)]
struct Rule {
    app_id: Option<Regex>,
    title: Option<Regex>,
    command: Option<Regex>,
}

impl Rule {
    fn matches(&self, app_id: &str, title: &str, command: &str) -> bool {
        self.app_id.as_ref().is_none_or(|r| r.is_match(app_id))
            && self.title.as_ref().is_none_or(|r| r.is_match(title))
            && self.command.as_ref().is_none_or(|r| r.is_match(command))
    }
}

#[derive(Debug)]
pub struct ParseError {
    line: usize,
    message: String,
}

impl ParseError {
    fn at(line: usize, message: String) -> Self {
        ParseError { line, message }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl Error for ParseError {}

fn path() -> PathBuf {
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config"));
    config_home.join("swaymnesia/config.toml")
}

/// Reads a TOML string
fn unquote(value: &str) -> Option<String> {
    if let Some(literal) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return Some(literal.to_string());
    }
    let basic = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = basic.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '"' => '"',
            '\\' => '\\',
            _ => return None,
        });
    }
    Some(out)
}
